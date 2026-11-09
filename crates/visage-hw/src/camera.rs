//! V4L2 camera capture via the `v4l` crate.

use crate::frame::{self, Frame};
use std::ops::ControlFlow;
use std::path::Path;
use thiserror::Error;
use v4l::buffer::Type as BufType;
use v4l::io::traits::CaptureStream;
use v4l::prelude::*;
use v4l::video::Capture;
use v4l::FourCC;

#[derive(Error, Debug)]
pub enum CameraError {
    #[error("device not found: {0}")]
    DeviceNotFound(String),
    #[error("capture failed: {0}")]
    CaptureFailed(String),
    #[error("device busy")]
    DeviceBusy,
    #[error("format negotiation failed: {0}")]
    FormatNegotiationFailed(String),
    #[error("streaming not supported")]
    StreamingNotSupported,
}

/// Info about a discovered V4L2 device.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub path: String,
    pub name: String,
    pub driver: String,
    pub bus: String,
}

/// Negotiated pixel format for the camera.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// YUYV 4:2:2 packed (2 bytes/pixel, extract Y channel).
    Yuyv,
    /// 8-bit grayscale (1 byte/pixel, native IR camera output).
    Grey,
    /// 16-bit little-endian grayscale (2 bytes/pixel, common IR camera format).
    Y16,
}

/// V4L2 camera device handle.
pub struct Camera {
    device: Device,
    pub width: u32,
    pub height: u32,
    pub device_path: String,
    pub fourcc: FourCC,
    /// Negotiated pixel format.
    pixel_format: PixelFormat,
}

/// What ends a capture loop.
///
/// Counted captures want N usable frames and accept a bounded number of
/// attempts to get them. The framing phase wants a predictable slice of the
/// user's attention and does not care how many frames arrive.
#[derive(Debug, Clone, Copy)]
enum Budget {
    /// Stop after this many non-dark frames, or `3x` that many attempts.
    Frames(usize),
    /// Stop at this instant, however many frames arrived.
    Until(std::time::Instant),
    /// Stop only when the observer says so.
    Open,
}

/// Adapt an observer that never stops the loop to the loop's signature.
fn continue_after<F>(mut observe: F) -> impl FnMut(&Frame) -> ControlFlow<()>
where
    F: FnMut(&Frame),
{
    move |frame| {
        observe(frame);
        ControlFlow::Continue(())
    }
}

impl Camera {
    /// Open a V4L2 camera device by path (e.g., "/dev/video2").
    pub fn open(device_path: &str) -> Result<Self, CameraError> {
        if !Path::new(device_path).exists() {
            return Err(CameraError::DeviceNotFound(device_path.to_string()));
        }

        let device = Device::with_path(device_path).map_err(|e| {
            if e.to_string().contains("busy") || e.to_string().contains("EBUSY") {
                CameraError::DeviceBusy
            } else {
                CameraError::DeviceNotFound(format!("{device_path}: {e}"))
            }
        })?;

        // Query capabilities
        let caps = device.query_caps().map_err(|e| {
            CameraError::CaptureFailed(format!("failed to query capabilities: {e}"))
        })?;

        tracing::info!(
            device = device_path,
            driver = %caps.driver,
            card = %caps.card,
            "opened camera"
        );

        // Check required capabilities
        let cap_flags = caps.capabilities;
        if !cap_flags.contains(v4l::capability::Flags::VIDEO_CAPTURE) {
            return Err(CameraError::StreamingNotSupported);
        }

        // Request format at 640x360 (common IR camera resolution).
        // Try YUYV first; if the driver negotiates GREY (common for IR cameras), accept it.
        let mut fmt = device.format().map_err(|e| {
            CameraError::FormatNegotiationFailed(format!("failed to get format: {e}"))
        })?;

        fmt.fourcc = FourCC::new(b"YUYV");
        fmt.width = 640;
        fmt.height = 360;

        let negotiated = device.set_format(&fmt).map_err(|e| {
            CameraError::FormatNegotiationFailed(format!("failed to set format: {e}"))
        })?;

        let fourcc = negotiated.fourcc;
        let pixel_format = if fourcc == FourCC::new(b"GREY") {
            PixelFormat::Grey
        } else if fourcc == FourCC::new(b"YUYV") {
            PixelFormat::Yuyv
        } else if fourcc == FourCC::new(b"Y16 ") || fourcc == FourCC::new(b"Y16\0") {
            PixelFormat::Y16
        } else {
            return Err(CameraError::FormatNegotiationFailed(format!(
                "unsupported pixel format: {fourcc:?} (need YUYV, GREY, or Y16)"
            )));
        };

        tracing::info!(
            width = negotiated.width,
            height = negotiated.height,
            fourcc = ?fourcc,
            "negotiated format"
        );

        Ok(Self {
            device,
            width: negotiated.width,
            height: negotiated.height,
            device_path: device_path.to_string(),
            fourcc,
            pixel_format,
        })
    }

    /// Re-assert visage's negotiated capture format on the (possibly shared) device.
    ///
    /// The daemon holds one persistent fd but negotiates the format only once, at
    /// [`Camera::open`]. On a regular webcam shared with other applications (e.g. a
    /// video-conferencing app), another process can open the same node, change the
    /// streaming format via `VIDIOC_S_FMT`, and close it — leaving the device in a
    /// format that no longer matches our cached `(fourcc, width, height)`. Our next
    /// capture would then stream at the other app's format and hand back buffers we
    /// misinterpret through the stale cache, which the detector reads as "no face"
    /// until a manual restart (issue #48).
    ///
    /// Cheap: one `VIDIOC_G_FMT`; `VIDIOC_S_FMT` fires only when the device drifted,
    /// so this is a no-op in the common, uncontended case. Runs before the
    /// `MmapStream` is created (before `REQBUFS`/`STREAMON`), where `S_FMT` is legal.
    fn reassert_format(&self) -> Result<(), CameraError> {
        let current = self.device.format().map_err(|e| {
            CameraError::CaptureFailed(format!("failed to query current format: {e}"))
        })?;

        // Fast path: device is still in our negotiated format.
        if current.fourcc == self.fourcc
            && current.width == self.width
            && current.height == self.height
        {
            return Ok(());
        }

        tracing::warn!(
            got_fourcc = ?current.fourcc,
            got_width = current.width,
            got_height = current.height,
            want_fourcc = ?self.fourcc,
            want_width = self.width,
            want_height = self.height,
            "device format drifted (another application changed it); re-asserting"
        );

        let mut fmt = current;
        fmt.fourcc = self.fourcc;
        fmt.width = self.width;
        fmt.height = self.height;

        let negotiated = self.device.set_format(&fmt).map_err(|e| {
            // Another app is actively streaming (owns the device): surface as busy,
            // not as a bogus format error.
            if e.to_string().contains("busy") || e.to_string().contains("EBUSY") {
                CameraError::DeviceBusy
            } else {
                CameraError::FormatNegotiationFailed(format!("failed to re-assert format: {e}"))
            }
        })?;

        if negotiated.fourcc != self.fourcc
            || negotiated.width != self.width
            || negotiated.height != self.height
        {
            return Err(CameraError::FormatNegotiationFailed(format!(
                "re-assert negotiated {:?} {}x{}, expected {:?} {}x{}",
                negotiated.fourcc,
                negotiated.width,
                negotiated.height,
                self.fourcc,
                self.width,
                self.height
            )));
        }
        Ok(())
    }

    /// Capture a single frame, converting to grayscale if needed.
    pub fn capture_frame(&self) -> Result<Frame, CameraError> {
        self.reassert_format()?;
        let mut stream =
            MmapStream::with_buffers(&self.device, BufType::VideoCapture, 4).map_err(|e| {
                CameraError::CaptureFailed(format!("failed to create mmap stream: {e}"))
            })?;

        let (buf, meta) = stream
            .next()
            .map_err(|e| CameraError::CaptureFailed(format!("failed to dequeue buffer: {e}")))?;

        let gray = self.buf_to_grayscale(buf)?;
        let is_dark = frame::is_dark_frame(&gray, 0.95);

        Ok(Frame {
            data: gray,
            width: self.width,
            height: self.height,
            timestamp: std::time::Instant::now(),
            sequence: meta.sequence,
            is_dark,
        })
    }

    /// Convert a raw buffer to grayscale based on the negotiated format.
    fn buf_to_grayscale(&self, buf: &[u8]) -> Result<Vec<u8>, CameraError> {
        let pixels = (self.width * self.height) as usize;

        match self.pixel_format {
            PixelFormat::Grey => {
                if buf.len() < pixels {
                    return Err(CameraError::CaptureFailed(format!(
                        "GREY buffer too short: expected {pixels}, got {}",
                        buf.len()
                    )));
                }
                Ok(buf[..pixels].to_vec())
            }
            PixelFormat::Y16 => {
                let expected_bytes = pixels * 2;
                if buf.len() < expected_bytes {
                    return Err(CameraError::CaptureFailed(format!(
                        "Y16 buffer too short: expected {expected_bytes}, got {}",
                        buf.len()
                    )));
                }
                // Y16: 16-bit little-endian per pixel, downscale to 8-bit
                let mut gray = Vec::with_capacity(pixels);
                for idx in 0..pixels {
                    let low = buf[idx * 2] as u16;
                    let high = buf[idx * 2 + 1] as u16;
                    let value = (high << 8) | low;
                    gray.push((value >> 8) as u8);
                }
                Ok(gray)
            }
            PixelFormat::Yuyv => frame::yuyv_to_grayscale(buf, self.width, self.height)
                .map_err(|e| CameraError::CaptureFailed(format!("YUYV conversion failed: {e}"))),
        }
    }

    /// Capture multiple frames with dark-frame filtering and CLAHE enhancement.
    ///
    /// Attempts up to `count * 3` raw captures to find `count` non-dark frames.
    /// Each non-dark frame gets CLAHE contrast enhancement applied.
    pub fn capture_frames(&self, count: usize) -> Result<(Vec<Frame>, usize), CameraError> {
        self.capture_frames_observed(count, |_| {})
    }

    /// `capture_frames`, with every dequeued frame handed to `observe` as it
    /// arrives — including the dark ones that are skipped.
    ///
    /// This exists so enrollment can show the user what the camera is seeing
    /// while it captures. Blind enrollment is the single biggest gap in the
    /// first-run experience: the user stares at a lens, the capture either
    /// works or does not, and nothing says whether they were too dark, too
    /// close, or out of frame.
    ///
    /// The dark frames matter most and are exactly the ones the capture loop
    /// throws away, so `observe` is called for those too, with `is_dark` set
    /// and no contrast enhancement applied — the client can then say "too
    /// dark" rather than silently showing nothing.
    ///
    /// `capture_frames` delegates here with a no-op closure, so the
    /// authentication path runs this identical code and cannot diverge from
    /// the enrollment path. `observe` must not block: it runs between buffer
    /// dequeues, and stalling it stalls the capture.
    pub fn capture_frames_observed<F>(
        &self,
        count: usize,
        observe: F,
    ) -> Result<(Vec<Frame>, usize), CameraError>
    where
        F: FnMut(&Frame),
    {
        self.capture_inner(Budget::Frames(count), true, true, continue_after(observe))
    }

    /// Stream frames to `observe` for up to `budget`, retaining none of them.
    ///
    /// This is the framing phase: the frames are for the human, not the model.
    /// The user needs a moment to see themselves and get centred before a
    /// capture commits, and every frame in that window is discarded.
    ///
    /// ⚠️ It is bounded by TIME, not by a frame count, and that is deliberate.
    /// The counted path only credits *non-dark* frames toward its target, so on
    /// hardware where most frames read dark — an unquirked emitter, which is
    /// exactly where a preview helps most — asking for N good frames can block
    /// for `N * 3` dequeues. A framing phase must occupy a predictable slice of
    /// the user's attention, so it takes a deadline.
    ///
    /// ⚠️ This is not free of side effects on the capture that follows. Sensor
    /// auto-gain only adapts while streaming, so holding the stream open here
    /// acts as an extended warmup and leaves AGC in a different state than a
    /// bare capture would. That is very likely an improvement — it is the same
    /// mechanism #104 relied on — but it is a real change to capture
    /// conditions, not a no-op.
    ///
    /// Returns the number of frames skipped as too dark.
    pub fn stream_frames_for<F>(
        &self,
        budget: std::time::Duration,
        observe: F,
    ) -> Result<usize, CameraError>
    where
        F: FnMut(&Frame),
    {
        self.capture_inner(
            Budget::Until(std::time::Instant::now() + budget),
            false,
            true,
            continue_after(observe),
        )
        .map(|(_, dark)| dark)
    }

    /// Stream frames to `on_frame` until it returns [`ControlFlow::Break`].
    ///
    /// For callers that decide frame by frame, like verification: it can stop
    /// at the first frame that settles the question instead of waiting for a
    /// fixed count, and it can keep looking without paying for a new stream.
    /// Starting a stream is the expensive part of a capture on UVC IR modules:
    /// measured at ~320 ms from `STREAMON` to the first frame on a Syntek
    /// 174f:118f, against ~35 ms between frames once it runs.
    ///
    /// Every frame is delivered, dark ones included (`is_dark` set, no CLAHE),
    /// so the caller can bound its own attempts. Lit frames get CLAHE as in
    /// [`Camera::capture_frames`]. Nothing is retained.
    ///
    /// The caller must eventually break: there is no frame or time limit here.
    pub fn stream_frames<F>(&self, on_frame: F) -> Result<(), CameraError>
    where
        F: FnMut(&Frame) -> ControlFlow<()>,
    {
        self.capture_inner(Budget::Open, false, true, on_frame)
            .map(|_| ())
    }

    /// Stream frames for `budget` with **no CLAHE**, retaining none of them.
    ///
    /// For measurement, not for recognition. The other entry points equalise
    /// contrast on lit frames and leave dark ones raw, which is right for a
    /// preview — both look to the user like what the camera sees — and wrong for
    /// physics. Comparing brightness across the IR emitter's lit/unlit strobe
    /// requires both halves to have had the same thing done to them, which is
    /// nothing.
    ///
    /// One stream for the whole window, so consecutive frames are genuinely
    /// adjacent. `capture_frame` cannot serve this: it builds a new `MmapStream`
    /// per call, so a loop over it samples disconnected points in the strobe.
    ///
    /// Returns the number of dark frames seen.
    pub fn stream_raw_frames_for<F>(
        &self,
        budget: std::time::Duration,
        observe: F,
    ) -> Result<usize, CameraError>
    where
        F: FnMut(&Frame),
    {
        self.capture_inner(
            Budget::Until(std::time::Instant::now() + budget),
            false,
            false,
            continue_after(observe),
        )
        .map(|(_, dark)| dark)
    }

    /// The one capture loop. Every public entry point above funnels through it,
    /// so the authentication path and the enrollment path cannot diverge.
    ///
    /// `observe` sees every frame and may end the loop early by returning
    /// [`ControlFlow::Break`]; the counted and timed entry points never do.
    fn capture_inner<F>(
        &self,
        budget: Budget,
        keep: bool,
        clahe: bool,
        mut observe: F,
    ) -> Result<(Vec<Frame>, usize), CameraError>
    where
        F: FnMut(&Frame) -> ControlFlow<()>,
    {
        self.reassert_format()?;
        let wanted = match budget {
            Budget::Frames(n) => n,
            Budget::Until(_) | Budget::Open => 0,
        };
        let max_attempts = wanted.saturating_mul(3);
        let mut good_frames = if keep {
            Vec::with_capacity(wanted)
        } else {
            Vec::new()
        };
        let mut good = 0usize;
        let mut attempts = 0usize;
        let mut dark_count = 0usize;

        let mut stream =
            MmapStream::with_buffers(&self.device, BufType::VideoCapture, 4).map_err(|e| {
                CameraError::CaptureFailed(format!("failed to create mmap stream: {e}"))
            })?;

        loop {
            match budget {
                Budget::Frames(_) => {
                    if good >= wanted || attempts >= max_attempts {
                        break;
                    }
                }
                Budget::Until(deadline) => {
                    if std::time::Instant::now() >= deadline {
                        break;
                    }
                }
                Budget::Open => {}
            }
            attempts += 1;

            let (buf, meta) = stream.next().map_err(|e| {
                CameraError::CaptureFailed(format!("failed to dequeue buffer: {e}"))
            })?;

            let mut gray = self.buf_to_grayscale(buf)?;

            if frame::is_dark_frame(&gray, 0.95) {
                dark_count += 1;
                tracing::debug!(seq = meta.sequence, "skipping dark frame");
                // Surface it anyway: "too dark" is the most useful thing the
                // user can be told, and it is only knowable here.
                let flow = observe(&Frame {
                    data: gray,
                    width: self.width,
                    height: self.height,
                    timestamp: std::time::Instant::now(),
                    sequence: meta.sequence,
                    is_dark: true,
                });
                if flow.is_break() {
                    break;
                }
                continue;
            }

            // Apply CLAHE contrast enhancement.
            //
            // Skipped for measurement callers. CLAHE equalises contrast, and
            // dark frames never receive it because they return above this line
            // — so a CLAHE'd lit frame compared against a raw dark one measures
            // the enhancement, not the sensor. Anything reasoning about the IR
            // emitter's physical effect needs both halves raw.
            if clahe {
                frame::clahe_enhance(&mut gray, self.width, self.height, 8, 0.02);
            }

            let frame = Frame {
                data: gray,
                width: self.width,
                height: self.height,
                timestamp: std::time::Instant::now(),
                sequence: meta.sequence,
                is_dark: false,
            };
            let flow = observe(&frame);
            good += 1;
            if keep {
                good_frames.push(frame);
            }
            if flow.is_break() {
                break;
            }
        }

        Ok((good_frames, dark_count))
    }

    /// List available V4L2 video capture devices.
    pub fn list_devices() -> Vec<DeviceInfo> {
        let mut devices = Vec::new();

        for i in 0..16 {
            let path = format!("/dev/video{i}");
            if !Path::new(&path).exists() {
                continue;
            }
            let Ok(dev) = Device::with_path(&path) else {
                continue;
            };
            let Ok(caps) = dev.query_caps() else {
                continue;
            };
            if !caps
                .capabilities
                .contains(v4l::capability::Flags::VIDEO_CAPTURE)
            {
                continue;
            }
            devices.push(DeviceInfo {
                path,
                name: caps.card.clone(),
                driver: caps.driver.clone(),
                bus: caps.bus.clone(),
            });
        }

        devices
    }
}
