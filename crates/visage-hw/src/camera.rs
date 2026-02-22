//! V4L2 camera capture via the `v4l` crate.

use crate::frame::{self, Frame};
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

    /// Capture a single frame, converting to grayscale if needed.
    pub fn capture_frame(&self) -> Result<Frame, CameraError> {
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
            PixelFormat::Yuyv => {
                frame::yuyv_to_grayscale(buf, self.width, self.height)
                    .map_err(|e| CameraError::CaptureFailed(format!("YUYV conversion failed: {e}")))
            }
        }
    }

    /// Capture multiple frames with dark-frame filtering and CLAHE enhancement.
    ///
    /// Attempts up to `count * 3` raw captures to find `count` non-dark frames.
    /// Each non-dark frame gets CLAHE contrast enhancement applied.
    pub fn capture_frames(&self, count: usize) -> Result<(Vec<Frame>, usize), CameraError> {
        let max_attempts = count * 3;
        let mut good_frames = Vec::with_capacity(count);
        let mut dark_count = 0usize;

        let mut stream =
            MmapStream::with_buffers(&self.device, BufType::VideoCapture, 4).map_err(|e| {
                CameraError::CaptureFailed(format!("failed to create mmap stream: {e}"))
            })?;

        for _ in 0..max_attempts {
            if good_frames.len() >= count {
                break;
            }

            let (buf, meta) = stream.next().map_err(|e| {
                CameraError::CaptureFailed(format!("failed to dequeue buffer: {e}"))
            })?;

            let mut gray = self.buf_to_grayscale(buf)?;

            if frame::is_dark_frame(&gray, 0.95) {
                dark_count += 1;
                tracing::debug!(seq = meta.sequence, "skipping dark frame");
                continue;
            }

            // Apply CLAHE contrast enhancement
            frame::clahe_enhance(&mut gray, self.width, self.height, 8, 0.02);

            good_frames.push(Frame {
                data: gray,
                width: self.width,
                height: self.height,
                timestamp: std::time::Instant::now(),
                sequence: meta.sequence,
                is_dark: false,
            });
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
            if !caps.capabilities.contains(v4l::capability::Flags::VIDEO_CAPTURE) {
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
