use std::ops::ControlFlow;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use visage_core::{
    check_landmark_stability, CosineMatcher, Embedding, FaceModel, MatchResult, Matcher,
};
use visage_hw::{Camera, IrEmitter};

#[derive(Error, Debug)]
pub enum EngineError {
    #[error("camera error: {0}")]
    Camera(#[from] visage_hw::CameraError),
    #[error("detector error: {0}")]
    Detector(#[from] visage_core::detector::DetectorError),
    #[error("recognizer error: {0}")]
    Recognizer(#[from] visage_core::recognizer::RecognizerError),
    #[error("no face detected in any captured frame")]
    NoFaceDetected,
    #[error("no usable frames captured (camera returned only dark or unreadable frames)")]
    NoUsableFrames,
    #[error("liveness check failed: landmark displacement {displacement:.3} px < threshold {threshold:.3} px")]
    LivenessCheckFailed { displacement: f32, threshold: f32 },
    #[error("verification timed out")]
    VerifyTimeout,
    #[error("engine thread exited")]
    ChannelClosed,
}

/// Consecutive "camera-broken" captures before the engine re-opens the device.
const MAX_CONSECUTIVE_CAPTURE_FAILURES: u32 = 3;

/// True only when a result indicates the *camera* is broken — dark/unreadable
/// frames or a capture error — never an absent/unrecognised user, a verify
/// timeout, or a liveness rejection. Only these arm the self-heal re-open (#48).
fn capture_looks_broken<T>(result: &Result<T, EngineError>) -> bool {
    matches!(
        result,
        Err(EngineError::NoUsableFrames) | Err(EngineError::Camera(_))
    )
}

/// Result of an enrollment operation.
pub struct EnrollResult {
    pub embedding: Embedding,
    pub quality_score: f32,
}

/// Result of a verification operation.
pub struct VerifyResult {
    pub result: MatchResult,
    /// Reserved for v3: surface capture quality metadata to callers without a schema change.
    #[allow(dead_code)]
    pub best_quality: f32,
}

/// Messages sent from D-Bus handlers to the engine thread.
enum EngineRequest {
    Enroll {
        frames_count: usize,
        /// Where to send preview frames as they are captured, if the caller
        /// wants them. `None` for callers that do not — the capture path is
        /// then byte-identical to what it was before previews existed.
        preview: Option<mpsc::Sender<PreviewFrame>>,
        /// How long to stream framing frames before capturing. Zero skips it.
        framing: std::time::Duration,
        reply: oneshot::Sender<Result<EnrollResult, EngineError>>,
    },
    Verify {
        gallery: Vec<FaceModel>,
        threshold: f32,
        frames_count: usize,
        timeout: std::time::Duration,
        /// How long to keep looking while no face has been seen. Zero means
        /// one group of frames. See `Config::face_wait`.
        face_wait: std::time::Duration,
        liveness_enabled: bool,
        liveness_min_displacement: f32,
        reply: oneshot::Sender<Result<VerifyResult, EngineError>>,
    },
}

/// Longest edge of an emitted preview frame, in pixels.
///
/// Deliberately small. It is enough to see whether a face is centred, lit and
/// in frame, which is all the preview is for — and it is deliberately not
/// enough to be a useful biometric capture. Full-resolution frames never leave
/// the engine thread: downscaling happens here, before the frame crosses a
/// channel, so there is no path on which a full-size frame reaches the bus.
const PREVIEW_MAX_EDGE: usize = 160;

/// A downscaled grayscale frame, emitted during enrollment so the client can
/// show the user what the camera is seeing.
#[derive(Debug, Clone)]
pub struct PreviewFrame {
    pub width: u32,
    pub height: u32,
    /// 8-bit grayscale, row-major, `width * height` bytes.
    pub data: Vec<u8>,
    /// The capture loop judged this frame too dark to use. It is emitted
    /// anyway: "too dark" is the most actionable thing a user can be told.
    pub is_dark: bool,
}

/// Nearest-neighbour downscale to `PREVIEW_MAX_EDGE` on the longest edge.
///
/// Nearest-neighbour rather than anything smoother on purpose — it is cheap
/// enough to run between buffer dequeues without stalling capture, and a
/// preview does not need to be pretty.
fn to_preview(frame: &visage_hw::Frame) -> PreviewFrame {
    let (w, h) = (frame.width as usize, frame.height as usize);
    let longest = w.max(h);
    let scale = if longest > PREVIEW_MAX_EDGE {
        longest.div_ceil(PREVIEW_MAX_EDGE)
    } else {
        1
    };
    let (ow, oh) = (w / scale, h / scale);
    let mut data = Vec::with_capacity(ow * oh);
    for y in 0..oh {
        let src_row = (y * scale) * w;
        for x in 0..ow {
            data.push(frame.data[src_row + x * scale]);
        }
    }
    PreviewFrame {
        width: ow as u32,
        height: oh as u32,
        data,
        is_dark: frame.is_dark,
    }
}

/// Clone-safe handle to the engine thread.
#[derive(Clone)]
pub struct EngineHandle {
    tx: mpsc::Sender<EngineRequest>,
}

impl EngineHandle {
    /// Request enrollment: capture frames, detect best face, extract embedding.
    ///
    /// `preview`, when given, receives downscaled frames as they are captured.
    /// Sends are non-blocking and dropped under backpressure: a slow or absent
    /// preview consumer must never slow down or fail an enrollment.
    pub async fn enroll(
        &self,
        frames_count: usize,
        preview: Option<mpsc::Sender<PreviewFrame>>,
        framing: std::time::Duration,
    ) -> Result<EnrollResult, EngineError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(EngineRequest::Enroll {
                frames_count,
                preview,
                framing,
                reply: reply_tx,
            })
            .await
            .map_err(|_| EngineError::ChannelClosed)?;
        reply_rx.await.map_err(|_| EngineError::ChannelClosed)?
    }

    /// Request verification: capture frames, detect, extract, compare against gallery.
    #[allow(clippy::too_many_arguments)]
    pub async fn verify(
        &self,
        gallery: Vec<FaceModel>,
        threshold: f32,
        frames_count: usize,
        timeout: std::time::Duration,
        face_wait: std::time::Duration,
        liveness_enabled: bool,
        liveness_min_displacement: f32,
    ) -> Result<VerifyResult, EngineError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(EngineRequest::Verify {
                gallery,
                threshold,
                frames_count,
                timeout,
                face_wait,
                liveness_enabled,
                liveness_min_displacement,
                reply: reply_tx,
            })
            .await
            .map_err(|_| EngineError::ChannelClosed)?;
        reply_rx.await.map_err(|_| EngineError::ChannelClosed)?
    }
}

/// Spawn the engine on a dedicated OS thread.
///
/// Opens the camera, loads both ONNX models, discards warmup frames,
/// then enters a request loop. Fails fast at startup if any resource
/// is unavailable.
pub fn spawn_engine(
    camera_device: &str,
    scrfd_path: &str,
    arcface_path: &str,
    inference: &visage_core::InferenceOptions,
    warmup_frames: usize,
    emitter_enabled: bool,
) -> Result<EngineHandle, EngineError> {
    // Open camera and load models synchronously (fail-fast)
    let camera = Camera::open(camera_device)?;
    tracing::info!(
        device = camera_device,
        width = camera.width,
        height = camera.height,
        fourcc = ?camera.fourcc,
        "camera opened"
    );

    let mut detector = visage_core::FaceDetector::load(scrfd_path, inference)?;
    tracing::info!(path = scrfd_path, device = ?detector.device(), "SCRFD detector loaded");

    let mut recognizer = visage_core::FaceRecognizer::load(arcface_path, inference)?;
    tracing::info!(path = arcface_path, device = ?recognizer.device(), "ArcFace recognizer loaded");

    // Probe for IR emitter quirk
    let emitter: Option<IrEmitter> = if emitter_enabled {
        match IrEmitter::for_device(camera_device) {
            Some(e) => {
                tracing::info!(name = %e.name(), device = %e.device_path(), "IR emitter found");
                Some(e)
            }
            None => {
                // Says what Visage will do, not what the hardware does.
                //
                // This read "proceeding without illumination", which asserts a
                // fact about the sensor that the daemon cannot know and which is
                // false on at least one shipped module: Shinetech 3277:0055
                // strobes its emitter lit/unlit every frame by firmware default
                // with no quirk present, measured at 10 good / 9 dark frames and
                // mean brightness 54.8. Two separate documents in this repo had
                // to carry a correction for this one log line, and it still sent
                // a reader chasing a missing quirk as the cause of a capture
                // failure that had nothing to do with it.
                tracing::warn!(
                    device = camera_device,
                    "no IR emitter quirk for device; Visage will not control the \
                     emitter. Some modules illuminate by firmware default — run \
                     `visage test` and check frame brightness before assuming the \
                     sensor is dark"
                );
                None
            }
        }
    } else {
        tracing::info!("IR emitter disabled via VISAGE_EMITTER_ENABLED=0");
        None
    };

    // Discard warmup frames for camera AGC/AE stabilization — with the emitter
    // active, in one continuous stream. Auto-gain only adapts while streaming;
    // warming up against ambient light leaves it high, and the first lit
    // capture after start is then saturated white and fails detection.
    if warmup_frames > 0 {
        tracing::info!(count = warmup_frames, "discarding warmup frames");
        activate_emitter(&emitter);
        let _ = camera.capture_frames(warmup_frames);
        deactivate_emitter(&emitter);
    }

    let (tx, mut rx) = mpsc::channel::<EngineRequest>(4);

    std::thread::Builder::new()
        .name("visage-engine".into())
        .spawn(move || {
            // `camera` must be reassignable so the engine can re-open the device
            // in-process (self-heal) rather than requiring a daemon restart (#48).
            let mut camera = camera;
            let device_path = camera.device_path.clone();
            let mut consecutive_failures: u32 = 0;

            tracing::info!("engine thread started");
            while let Some(req) = rx.blocking_recv() {
                let broken = match req {
                    EngineRequest::Enroll {
                        frames_count,
                        preview,
                        framing,
                        reply,
                    } => {
                        let result = run_enroll(
                            &camera,
                            &emitter,
                            &mut detector,
                            &mut recognizer,
                            frames_count,
                            preview.as_ref(),
                            framing,
                        );
                        let broken = capture_looks_broken(&result);
                        let _ = reply.send(result);
                        broken
                    }
                    EngineRequest::Verify {
                        gallery,
                        threshold,
                        frames_count,
                        timeout,
                        face_wait,
                        liveness_enabled,
                        liveness_min_displacement,
                        reply,
                    } => {
                        let deadline = std::time::Instant::now() + timeout;
                        let result = run_verify(
                            &camera,
                            &emitter,
                            &mut detector,
                            &mut recognizer,
                            &gallery,
                            threshold,
                            frames_count,
                            deadline,
                            face_wait,
                            liveness_enabled,
                            liveness_min_displacement,
                        );
                        let broken = capture_looks_broken(&result);
                        let _ = reply.send(result);
                        broken
                    }
                };

                // --- Self-heal: re-open the camera after repeated broken captures ---
                // This replicates what a manual `systemctl restart` does — re-run
                // `Camera::open` (fresh fd + `S_FMT`) — catching any residual desync
                // that per-capture format re-assertion alone does not reset.
                if broken {
                    consecutive_failures += 1;
                    if consecutive_failures >= MAX_CONSECUTIVE_CAPTURE_FAILURES {
                        tracing::warn!(
                            consecutive_failures,
                            "repeated camera-broken captures — re-initializing camera (self-heal)"
                        );
                        match Camera::open(&device_path) {
                            Ok(fresh) => {
                                camera = fresh;
                                consecutive_failures = 0;
                                tracing::info!(device = %device_path, "camera re-opened after failures");
                            }
                            Err(e) => {
                                // Keep the old handle and retry on the next failure;
                                // never let the engine thread die.
                                tracing::error!(error = %e, "camera re-open failed; will retry");
                            }
                        }
                    }
                } else {
                    consecutive_failures = 0;
                }
            }
            tracing::info!("engine thread exiting");
        })
        .expect("failed to spawn engine thread");

    Ok(EngineHandle { tx })
}

/// Activate the IR emitter and sleep briefly for AGC stabilisation.
/// Logs a warning on failure but never propagates the error — capture
/// continues with ambient light.
fn activate_emitter(emitter: &Option<IrEmitter>) {
    if let Some(e) = emitter {
        if let Err(err) = e.activate() {
            tracing::warn!(error = %err, "IR emitter activate failed; continuing without illumination");
        } else {
            // Allow AGC (auto gain control) to stabilise before capture.
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}

/// Deactivate the IR emitter. Logs a warning on failure.
fn deactivate_emitter(emitter: &Option<IrEmitter>) {
    if let Some(e) = emitter {
        if let Err(err) = e.deactivate() {
            tracing::warn!(error = %err, "IR emitter deactivate failed");
        }
    }
}

/// Capture frames, extract embeddings from all detected faces, and return
/// a confidence-weighted average embedding (L2-normalized).
fn run_enroll(
    camera: &Camera,
    emitter: &Option<IrEmitter>,
    detector: &mut visage_core::FaceDetector,
    recognizer: &mut visage_core::FaceRecognizer,
    frames_count: usize,
    preview: Option<&mpsc::Sender<PreviewFrame>>,
    framing: std::time::Duration,
) -> Result<EnrollResult, EngineError> {
    activate_emitter(emitter);

    // Framing phase: give the user a moment to see themselves and get centred
    // before anything is captured. Frames go to the client and are discarded.
    //
    // Only worth doing when someone is actually watching — with no preview
    // channel this is pure latency, so it is skipped. The emitter is already
    // active, so framing frames are lit the way the real capture will be;
    // showing an unlit preview of a capture that will be lit would be worse
    // than showing nothing.
    if let Some(tx) = preview {
        if !framing.is_zero() {
            match camera.stream_frames_for(framing, |frame| {
                let _ = tx.try_send(to_preview(frame));
            }) {
                Ok(dark) => tracing::debug!(dark, "enroll: framing phase complete"),
                // A framing failure is not an enrollment failure. The capture
                // below is what matters; losing the preview should never cost
                // the user their enrollment.
                Err(e) => {
                    tracing::warn!(error = %e, "enroll: framing phase failed, capturing anyway")
                }
            }
        }
    }

    // `try_send` rather than `send`: this closure runs between buffer dequeues
    // on the engine thread, so blocking here would stall the capture. A full
    // channel means the client is not keeping up, and the right response is to
    // drop the frame, not to slow the enrollment down.
    let capture_result = camera.capture_frames_observed(frames_count, |frame| {
        if let Some(tx) = preview {
            let _ = tx.try_send(to_preview(frame));
        }
    });
    deactivate_emitter(emitter);

    let (frames, dark_skipped) = capture_result?;
    tracing::debug!(
        captured = frames.len(),
        dark_skipped,
        "enroll: captured frames"
    );

    if frames.is_empty() {
        return Err(EngineError::NoUsableFrames);
    }

    let mut embeddings: Vec<(Embedding, f32)> = Vec::new();
    let mut best_confidence = 0.0f32;
    let mut best_frame_idx = 0usize;

    for (i, frame) in frames.iter().enumerate() {
        let faces = detector.detect(&frame.data, frame.width, frame.height)?;
        let Some(face) = faces.first() else {
            continue;
        };

        let embedding = match recognizer.extract(&frame.data, frame.width, frame.height, face) {
            Ok(embedding) => embedding,
            Err(visage_core::recognizer::RecognizerError::NoLandmarks) => continue,
            Err(e) => return Err(e.into()),
        };

        let weight = face.confidence.max(0.0);
        if weight > best_confidence {
            best_confidence = weight;
            best_frame_idx = i;
        }

        embeddings.push((embedding, weight));
    }

    if embeddings.is_empty() {
        return Err(EngineError::NoFaceDetected);
    }

    tracing::info!(
        confidence = best_confidence,
        frame = best_frame_idx,
        "enroll: best face selected"
    );

    let dim = embeddings[0].0.values.len();

    let total_weight: f32 = embeddings.iter().map(|(_, w)| *w).sum();
    let (denom, use_weighted) = if total_weight > 0.0 {
        (total_weight, true)
    } else {
        (embeddings.len() as f32, false)
    };

    let mut avg = vec![0.0f32; dim];
    for (emb, w) in &embeddings {
        let w = if use_weighted { *w } else { 1.0 };
        for (a, v) in avg.iter_mut().zip(emb.values.iter()) {
            *a += v * w;
        }
    }
    for v in &mut avg {
        *v /= denom;
    }

    // L2-normalize the averaged embedding
    let norm: f32 = avg.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for v in &mut avg {
            *v /= norm;
        }
    }

    let embedding = Embedding {
        values: avg,
        model_version: embeddings[0].0.model_version.clone(),
    };

    Ok(EnrollResult {
        embedding,
        quality_score: best_confidence,
    })
}

/// Capture frames, detect faces, extract embeddings, compare against gallery.
/// Uses the best match across all captured frames.
///
/// Time not attributable to any stage this struct measures.
///
/// Reported rather than dropped. The whole point of profiling an unattributed
/// 2.3s is to find where it goes, and an instrument that silently absorbs its
/// own blind spot would hand back a tidy breakdown that adds up to less than the
/// clock. If `other_ms` is large, the next stage to instrument is whatever is
/// hiding in it.
fn unaccounted(total: std::time::Duration, parts: &[std::time::Duration]) -> std::time::Duration {
    let summed: std::time::Duration = parts.iter().sum();
    total.saturating_sub(summed)
}

/// Per-stage timing for one verify, emitted on EVERY exit path.
///
/// `run_verify` has five early returns — two timeouts, no-usable-frames,
/// no-face, and a liveness rejection. A `tracing` call at the bottom of the
/// function would be skipped by all of them, and a FAILED authentication is
/// precisely when the breakdown matters: "it took 2.3s and then rejected you"
/// is the complaint, not the success case. A drop guard also cannot be bypassed
/// by an early return somebody adds later.
///
/// Emitted at INFO so it lands in the journal under the shipped
/// `RUST_LOG=visaged=info`, which makes every real `sudo` a profiling sample
/// under real conditions — better evidence than a synthetic benchmark that
/// warms differently.
struct VerifyTiming {
    start: std::time::Instant,
    capture: std::time::Duration,
    detect: std::time::Duration,
    detect_n: usize,
    extract: std::time::Duration,
    extract_n: usize,
    frames: usize,
    dark_skipped: usize,
    /// Groups of frames looked at. More than one only when the face-wait
    /// window kept the verify looking.
    bursts: usize,
}

impl VerifyTiming {
    fn start() -> Self {
        Self {
            start: std::time::Instant::now(),
            capture: std::time::Duration::ZERO,
            detect: std::time::Duration::ZERO,
            detect_n: 0,
            extract: std::time::Duration::ZERO,
            extract_n: 0,
            frames: 0,
            dark_skipped: 0,
            bursts: 0,
        }
    }
}

impl Drop for VerifyTiming {
    fn drop(&mut self) {
        let total = self.start.elapsed();
        let other = unaccounted(total, &[self.capture, self.detect, self.extract]);
        tracing::info!(
            total_ms = total.as_millis() as u64,
            capture_ms = self.capture.as_millis() as u64,
            detect_ms = self.detect.as_millis() as u64,
            detect_n = self.detect_n,
            extract_ms = self.extract.as_millis() as u64,
            extract_n = self.extract_n,
            other_ms = other.as_millis() as u64,
            frames = self.frames,
            dark_skipped = self.dark_skipped,
            bursts = self.bursts,
            "verify timing"
        );
    }
}

/// How much of a frame the verify loop still needs.
#[derive(Debug, PartialEq, Eq)]
enum FrameWork {
    /// Detect, extract and compare.
    Full,
    /// Detect only: the liveness check needs this frame's landmarks, but the
    /// identity decision is already made.
    LandmarksOnly,
    /// Nothing: the decision is made and nothing else reads the frame.
    None,
}

/// Once one frame has matched, the verify's outcome is a match whatever the
/// remaining frames hold: the result is matched if any frame matched. Their
/// ArcFace extractions can only change the reported similarity, never the
/// decision, so they are skipped. That was two of three extractions per
/// successful verify at the default `frames_per_verify=3`.
///
/// The liveness check is the exception for detection only: it needs landmarks
/// from every frame, and starving it of frames would make it fail closed.
fn frame_work(matched: bool, liveness_enabled: bool) -> FrameWork {
    match (matched, liveness_enabled) {
        (false, _) => FrameWork::Full,
        (true, true) => FrameWork::LandmarksOnly,
        (true, false) => FrameWork::None,
    }
}

/// Brightness of one frame the dark filter rejected, kept for the log line a
/// verify writes when it saw nothing else.
#[derive(Debug, Clone, Copy, PartialEq)]
struct DarkFrame {
    sequence: u32,
    /// Mean pixel value, 0-255.
    mean: f32,
    /// 99th-percentile pixel value, 0-255.
    p99: u8,
}

/// Most dark frames a verify keeps for that log line.
const DARK_FRAMES_LOGGED: usize = 32;

fn dark_frame_stats(sequence: u32, data: &[u8]) -> DarkFrame {
    let mut histogram = [0usize; 256];
    for &px in data {
        histogram[px as usize] += 1;
    }
    let n = data.len().max(1);
    let sum: usize = histogram.iter().enumerate().map(|(v, c)| v * c).sum();
    let rank = (n * 99).div_ceil(100);
    let mut seen = 0;
    let mut p99 = 0u8;
    for (value, count) in histogram.iter().enumerate() {
        seen += count;
        if seen >= rank {
            p99 = value as u8;
            break;
        }
    }
    DarkFrame {
        sequence,
        mean: sum as f32 / n as f32,
        p99,
    }
}

/// `seq 41: 1.0/2, seq 42: 1.1/2` — sequence, mean / 99th percentile.
fn describe_dark_frames(frames: &[DarkFrame]) -> String {
    frames
        .iter()
        .map(|f| format!("seq {}: {:.1}/{}", f.sequence, f.mean, f.p99))
        .collect::<Vec<_>>()
        .join(", ")
}

/// What one frame showed, as far as the verify decision is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameSeen {
    /// Too dark to use.
    Dark,
    /// Usable, but no face in it.
    NoFace,
    /// A face. `matched` is whether any frame of the verify has matched the
    /// gallery so far.
    Face { matched: bool },
}

/// What the verify loop does after a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Read the next frame.
    Continue,
    /// The verify has its answer, matched or not.
    Decide,
    /// The frames ran out without a face.
    NoFace,
    /// The frames ran out and every one was dark.
    NoUsableFrames,
}

/// Counts a verify's frames the way a capture burst does.
///
/// A verify reads `frames_per_verify` usable frames, or three times that many
/// attempts when frames come out dark: the bounds of
/// [`Camera::capture_frames`], which the verify used before. The difference is
/// that each frame is examined as it arrives, on one stream, so the verify can
/// stop as soon as its answer is known instead of waiting for the burst.
///
/// The outcomes are the burst's:
/// - Once the frames are used up, a verify that saw a face decides, matched or
///   not.
/// - One that saw no face fails with [`Step::NoFace`], or with
///   [`Step::NoUsableFrames`] when every frame was dark (which the engine
///   treats as a broken camera). Inside the face-wait window the verify
///   instead [`FrameBudget::restart`]s and keeps reading the same stream; a
///   face that was seen, matched or not, always ends it.
/// - A match with liveness off decides at once: nothing else can change the
///   outcome (see [`frame_work`]). With liveness on the verify reads all its
///   frames, because the landmark check needs every one.
struct FrameBudget {
    frames_per_verify: usize,
    liveness_enabled: bool,
    usable: usize,
    attempts: usize,
    face_seen: bool,
    matched: bool,
    /// Whether any frame of the whole verify, not just since the last
    /// restart, was usable.
    any_usable: bool,
    /// With a face-wait window, the first face of the verify starts a fresh
    /// group at its frame, so the liveness check gets `frames_per_verify`
    /// frames counted from the face. Without a window, the one group is
    /// counted from the start of the verify, as a capture burst was.
    regroup_at_first_face: bool,
}

impl FrameBudget {
    /// `frames_per_verify` must be at least 1; `run_verify` handles 0.
    fn new(frames_per_verify: usize, liveness_enabled: bool, regroup_at_first_face: bool) -> Self {
        Self {
            frames_per_verify,
            liveness_enabled,
            usable: 0,
            attempts: 0,
            face_seen: false,
            matched: false,
            any_usable: false,
            regroup_at_first_face,
        }
    }

    /// Start another group of frames after one that saw no face, for the
    /// face-wait window. Only valid after [`Step::NoFace`] or
    /// [`Step::NoUsableFrames`]: a face that was seen decides the verify and
    /// never earns another group, or one request would become several
    /// attempts against the gallery that the rate limiter cannot see.
    fn restart(&mut self) {
        debug_assert!(!self.face_seen);
        self.usable = 0;
        self.attempts = 0;
    }

    /// How much of the next usable frame the verify still needs.
    fn work(&self) -> FrameWork {
        frame_work(self.matched, self.liveness_enabled)
    }

    fn record(&mut self, seen: FrameSeen) -> Step {
        if self.regroup_at_first_face && !self.face_seen && matches!(seen, FrameSeen::Face { .. }) {
            // A face that first appears late in a group would otherwise leave
            // the liveness check with too few landmark frames, and it fails
            // closed: start the group at this frame instead.
            self.usable = 0;
            self.attempts = 0;
        }
        self.attempts += 1;
        match seen {
            FrameSeen::Dark => {}
            FrameSeen::NoFace => {
                self.usable += 1;
                self.any_usable = true;
            }
            FrameSeen::Face { matched } => {
                self.usable += 1;
                self.any_usable = true;
                self.face_seen = true;
                self.matched |= matched;
            }
        }

        if self.matched && !self.liveness_enabled {
            return Step::Decide;
        }
        let used_up = self.usable >= self.frames_per_verify
            || self.attempts >= self.frames_per_verify.saturating_mul(3);
        if !used_up {
            Step::Continue
        } else if self.face_seen {
            Step::Decide
        } else if !self.any_usable {
            // Only a verify in which every frame was dark looks like a broken
            // camera; one that saw usable frames and then went dark (lens
            // covered) found no face.
            Step::NoUsableFrames
        } else {
            Step::NoFace
        }
    }
}

/// What the verify's frames added up to.
#[derive(Default)]
struct Evidence {
    best: Option<MatchResult>,
    best_quality: f32,
    landmark_sequence: Vec<[(f32, f32); 5]>,
}

impl Evidence {
    fn matched(&self) -> bool {
        self.best.as_ref().is_some_and(|r| r.matched)
    }
}

/// Detect, and when `work` asks for it extract and compare, one usable frame.
#[allow(clippy::too_many_arguments)]
fn examine_frame(
    frame: &visage_hw::Frame,
    work: FrameWork,
    detector: &mut visage_core::FaceDetector,
    recognizer: &mut visage_core::FaceRecognizer,
    gallery: &[FaceModel],
    threshold: f32,
    evidence: &mut Evidence,
    timing: &mut VerifyTiming,
) -> Result<FrameSeen, EngineError> {
    let t_detect = std::time::Instant::now();
    let faces = detector.detect(&frame.data, frame.width, frame.height)?;
    timing.detect += t_detect.elapsed();
    timing.detect_n += 1;
    let Some(face) = faces.first() else {
        return Ok(FrameSeen::NoFace);
    };

    // Collect landmarks for liveness check
    if let Some(landmarks) = face.landmarks {
        evidence.landmark_sequence.push(landmarks);
    }

    if work == FrameWork::Full {
        let t_extract = std::time::Instant::now();
        let embedding = recognizer.extract(&frame.data, frame.width, frame.height, face)?;
        timing.extract += t_extract.elapsed();
        timing.extract_n += 1;
        let result = CosineMatcher.compare(&embedding, gallery, threshold);

        let is_better = evidence
            .best
            .as_ref()
            .is_none_or(|prev| result.similarity > prev.similarity);
        if is_better {
            evidence.best_quality = face.confidence;
            evidence.best = Some(result);
        }
    }

    Ok(FrameSeen::Face {
        matched: evidence.matched(),
    })
}

/// Stream frames, examining each as it arrives, until [`FrameBudget`] has an
/// answer; then apply the liveness gate.
///
/// When `liveness_enabled` is true, collects eye landmarks across all frames
/// and runs a passive stability check before accepting a match. Static images
/// (photographs) produce near-identical landmarks and are rejected.
#[allow(clippy::too_many_arguments)]
fn run_verify(
    camera: &Camera,
    emitter: &Option<IrEmitter>,
    detector: &mut visage_core::FaceDetector,
    recognizer: &mut visage_core::FaceRecognizer,
    gallery: &[FaceModel],
    threshold: f32,
    frames_count: usize,
    deadline: std::time::Instant,
    face_wait: std::time::Duration,
    liveness_enabled: bool,
    liveness_min_displacement: f32,
) -> Result<VerifyResult, EngineError> {
    let mut timing = VerifyTiming::start();

    if std::time::Instant::now() > deadline {
        return Err(EngineError::VerifyTimeout);
    }
    // A burst of zero frames captured nothing; an open stream would read one.
    if frames_count == 0 {
        return Err(EngineError::NoUsableFrames);
    }

    let face_deadline = std::cmp::min(timing.start + face_wait, deadline);
    let mut budget = FrameBudget::new(frames_count, liveness_enabled, !face_wait.is_zero());
    let mut evidence = Evidence::default();
    let mut outcome: Option<Result<(), EngineError>> = None;
    let mut dark_frames: Vec<DarkFrame> = Vec::new();
    timing.bursts = 1;

    activate_emitter(emitter);
    let t_stream = std::time::Instant::now();
    let streamed = camera.stream_frames(|frame| {
        if std::time::Instant::now() > deadline {
            outcome = Some(Err(EngineError::VerifyTimeout));
            return ControlFlow::Break(());
        }

        let seen = if frame.is_dark {
            timing.dark_skipped += 1;
            if dark_frames.len() < DARK_FRAMES_LOGGED {
                dark_frames.push(dark_frame_stats(frame.sequence, &frame.data));
            }
            FrameSeen::Dark
        } else {
            timing.frames += 1;
            match examine_frame(
                frame,
                budget.work(),
                detector,
                recognizer,
                gallery,
                threshold,
                &mut evidence,
                &mut timing,
            ) {
                Ok(seen) => seen,
                Err(e) => {
                    outcome = Some(Err(e));
                    return ControlFlow::Break(());
                }
            }
        };

        let step = budget.record(seen);
        if matches!(step, Step::NoFace | Step::NoUsableFrames)
            && std::time::Instant::now() < face_deadline
        {
            // Nothing seen yet: keep looking on the same stream.
            budget.restart();
            timing.bursts += 1;
            return ControlFlow::Continue(());
        }
        outcome = match step {
            Step::Continue => return ControlFlow::Continue(()),
            Step::Decide => Some(Ok(())),
            Step::NoFace => Some(Err(EngineError::NoFaceDetected)),
            Step::NoUsableFrames => Some(Err(EngineError::NoUsableFrames)),
        };
        ControlFlow::Break(())
    });
    deactivate_emitter(emitter);
    // Frames are examined as they arrive, so the stream's wall time includes
    // the inference; capture is what is left.
    timing.capture = unaccounted(t_stream.elapsed(), &[timing.detect, timing.extract]);

    streamed?;
    tracing::debug!(
        usable = timing.frames,
        dark_skipped = timing.dark_skipped,
        "verify: streamed frames"
    );
    if matches!(outcome, Some(Err(EngineError::NoUsableFrames))) {
        // The cause of all-dark verifies is not known yet (a covered lens or
        // shutter, an emitter that did not light, a threshold problem). These
        // numbers tell them apart: values at the sensor's black level on every
        // frame point at something over the lens; a p99 well above it means
        // the sensor sees a scene that the emitter did not light.
        tracing::warn!(
            frames = %describe_dark_frames(&dark_frames),
            "verify saw only dark frames (mean/p99 brightness, 0-255)"
        );
    }
    // The stream only ends through the closure, which always records why.
    outcome.unwrap_or(Err(EngineError::NoUsableFrames))?;

    let result = evidence.best.unwrap_or(MatchResult {
        matched: false,
        similarity: 0.0,
        model_id: None,
        model_label: None,
    });

    // --- Passive liveness check ---
    // Only gates the result when a match would otherwise succeed. The check
    // fails closed: fewer than 2 landmark frames yields `is_live = false`
    // (rejected), so a spoof that produces only a single detectable landmark
    // frame cannot slip past liveness by starving it of evidence.
    if liveness_enabled && result.matched {
        let liveness =
            check_landmark_stability(&evidence.landmark_sequence, Some(liveness_min_displacement));

        tracing::debug!(
            is_live = liveness.is_live,
            mean_eye_displacement = liveness.mean_eye_displacement,
            frame_pairs = liveness.frame_pairs_analysed,
            threshold = liveness_min_displacement,
            "liveness check"
        );

        if !liveness.is_live {
            tracing::warn!(
                similarity = result.similarity,
                displacement = liveness.mean_eye_displacement,
                "liveness rejected a face that matched identity — possible spoof attempt"
            );
            return Err(EngineError::LivenessCheckFailed {
                displacement: liveness.mean_eye_displacement,
                threshold: liveness_min_displacement,
            });
        }
    }

    Ok(VerifyResult {
        result,
        best_quality: evidence.best_quality,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unattributed time must be REPORTED, not absorbed.
    ///
    /// The point of profiling a 2.3s verify is to find where it goes. An
    /// instrument whose stages quietly sum to less than the clock hands back a
    /// tidy breakdown and hides its own blind spot.
    #[test]
    fn unaccounted_time_is_reported_and_never_underflows() {
        use std::time::Duration;

        let total = Duration::from_millis(2273);
        let parts = [Duration::from_millis(200), Duration::from_millis(900)];
        assert_eq!(
            unaccounted(total, &parts),
            Duration::from_millis(1173),
            "the gap between the clock and the measured stages is the finding"
        );

        // Saturating, not panicking. Stage timers are taken with separate
        // Instants, so rounding or a re-entrant call could make the parts
        // exceed the total — and `Duration` subtraction underflow PANICS.
        // A profiler that can crash the authentication path is worse than no
        // profiler.
        assert_eq!(
            unaccounted(Duration::from_millis(10), &[Duration::from_millis(50)]),
            Duration::ZERO,
            "overlong parts must clamp to zero, never underflow"
        );

        // Degenerate: nothing measured means everything is unattributed.
        assert_eq!(unaccounted(total, &[]), total);
    }

    /// The self-heal re-open must arm ONLY on camera-broken outcomes — never on a
    /// genuine no-face / unknown-user, a verify timeout, a liveness rejection, or a
    /// success. Guards the false-positive property in CI (no hardware needed).
    #[test]
    fn self_heal_only_arms_on_camera_broken() {
        // Camera-broken → arm.
        assert!(capture_looks_broken::<()>(&Err(
            EngineError::NoUsableFrames
        )));
        assert!(capture_looks_broken::<()>(&Err(EngineError::Camera(
            visage_hw::CameraError::DeviceBusy
        ))));
        // Everything else → do NOT re-open.
        assert!(!capture_looks_broken::<()>(&Err(
            EngineError::NoFaceDetected
        )));
        assert!(!capture_looks_broken::<()>(&Err(
            EngineError::VerifyTimeout
        )));
        assert!(!capture_looks_broken::<()>(&Err(
            EngineError::LivenessCheckFailed {
                displacement: 0.0,
                threshold: 1.0,
            }
        )));
        assert!(!capture_looks_broken::<()>(&Ok(())));
    }

    /// Skipping work after a match must never skip work the decision or the
    /// liveness gate depends on.
    #[test]
    fn frames_after_a_match_skip_only_what_cannot_change_the_outcome() {
        // No match yet: every frame gets the full pipeline, with or without liveness.
        assert_eq!(frame_work(false, false), FrameWork::Full);
        assert_eq!(frame_work(false, true), FrameWork::Full);
        // Matched with liveness on: keep detecting, the check needs every
        // frame's landmarks and fails closed without them.
        assert_eq!(frame_work(true, true), FrameWork::LandmarksOnly);
        // Matched with liveness off: nothing reads the remaining frames.
        assert_eq!(frame_work(true, false), FrameWork::None);
    }

    /// A match with liveness off decides on that frame: the remaining frames
    /// cannot change the outcome.
    #[test]
    fn a_match_without_liveness_decides_at_once() {
        let mut b = FrameBudget::new(3, false, false);
        assert_eq!(b.record(FrameSeen::NoFace), Step::Continue);
        assert_eq!(b.record(FrameSeen::Face { matched: true }), Step::Decide);
    }

    /// With liveness on, the landmark check needs every frame, so a match
    /// keeps the verify reading, detection only, to the end of its frames.
    #[test]
    fn a_match_with_liveness_reads_all_its_frames() {
        let mut b = FrameBudget::new(3, true, false);
        assert_eq!(b.record(FrameSeen::Face { matched: true }), Step::Continue);
        assert_eq!(b.work(), FrameWork::LandmarksOnly);
        assert_eq!(b.record(FrameSeen::Face { matched: true }), Step::Continue);
        assert_eq!(b.record(FrameSeen::NoFace), Step::Decide);
    }

    /// A face that was seen and rejected decides once the frames are used up,
    /// as it did after a burst: a later frame may still match.
    #[test]
    fn a_rejected_face_decides_when_the_frames_run_out() {
        let mut b = FrameBudget::new(3, false, false);
        assert_eq!(b.record(FrameSeen::Face { matched: false }), Step::Continue);
        assert_eq!(b.work(), FrameWork::Full);
        assert_eq!(b.record(FrameSeen::NoFace), Step::Continue);
        assert_eq!(b.record(FrameSeen::NoFace), Step::Decide);
    }

    #[test]
    fn no_face_in_any_frame_fails_as_no_face() {
        let mut b = FrameBudget::new(3, false, false);
        assert_eq!(b.record(FrameSeen::NoFace), Step::Continue);
        assert_eq!(b.record(FrameSeen::NoFace), Step::Continue);
        assert_eq!(b.record(FrameSeen::NoFace), Step::NoFace);
    }

    /// The face-wait window restarts a group that saw no face; the next
    /// group gets the full budget again. A face in it starts a group of its
    /// own, which decides once its frames are used up, matched or not.
    #[test]
    fn a_restart_after_no_face_reads_a_fresh_group() {
        let mut b = FrameBudget::new(3, false, true);
        for _ in 0..2 {
            assert_eq!(b.record(FrameSeen::NoFace), Step::Continue);
        }
        assert_eq!(b.record(FrameSeen::NoFace), Step::NoFace);
        b.restart();
        assert_eq!(b.record(FrameSeen::NoFace), Step::Continue);
        assert_eq!(b.record(FrameSeen::NoFace), Step::Continue);
        assert_eq!(b.record(FrameSeen::Face { matched: false }), Step::Continue);
        assert_eq!(b.record(FrameSeen::NoFace), Step::Continue);
        assert_eq!(b.record(FrameSeen::NoFace), Step::Decide);
    }

    /// With a window, a face that first appears on the last frame of a group
    /// must not decide on that one frame: with liveness on, the check needs
    /// `frames_per_verify` landmark frames and fails closed below two, which
    /// would turn a late but genuine face into a non-match.
    #[test]
    fn with_a_window_the_first_face_starts_a_full_group() {
        let mut b = FrameBudget::new(3, true, true);
        for _ in 0..2 {
            assert_eq!(b.record(FrameSeen::NoFace), Step::Continue);
        }
        assert_eq!(b.record(FrameSeen::NoFace), Step::NoFace);
        b.restart();
        assert_eq!(b.record(FrameSeen::NoFace), Step::Continue);
        assert_eq!(b.record(FrameSeen::NoFace), Step::Continue);
        assert_eq!(b.record(FrameSeen::Face { matched: true }), Step::Continue);
        assert_eq!(b.work(), FrameWork::LandmarksOnly);
        assert_eq!(b.record(FrameSeen::Face { matched: true }), Step::Continue);
        assert_eq!(b.record(FrameSeen::Face { matched: true }), Step::Decide);
    }

    /// Without a window nothing changes: the one group is counted from the
    /// start of the verify, exactly like a capture burst.
    #[test]
    fn without_a_window_a_late_face_decides_with_the_group() {
        let mut b = FrameBudget::new(3, true, false);
        assert_eq!(b.record(FrameSeen::NoFace), Step::Continue);
        assert_eq!(b.record(FrameSeen::NoFace), Step::Continue);
        assert_eq!(b.record(FrameSeen::Face { matched: true }), Step::Decide);
    }

    /// A verify that saw usable frames and then went dark (lens covered, lid
    /// closing) found no face. It must not be reported as a broken camera,
    /// which three times in a row makes the engine re-open the device.
    #[test]
    fn no_usable_frames_means_the_whole_verify_was_dark() {
        let mut b = FrameBudget::new(3, false, true);
        for _ in 0..2 {
            assert_eq!(b.record(FrameSeen::NoFace), Step::Continue);
        }
        assert_eq!(b.record(FrameSeen::NoFace), Step::NoFace);
        b.restart();
        for _ in 0..8 {
            assert_eq!(b.record(FrameSeen::Dark), Step::Continue);
        }
        assert_eq!(b.record(FrameSeen::Dark), Step::NoFace);
    }

    /// Dark frames count as attempts, bounded at three per wanted frame, the
    /// same budget `Camera::capture_frames` has. Only a verify in which every
    /// frame was dark reports no usable frames, which the engine treats as a
    /// broken camera.
    #[test]
    fn dark_frames_are_bounded_like_a_capture_burst() {
        let mut b = FrameBudget::new(3, false, false);
        for _ in 0..8 {
            assert_eq!(b.record(FrameSeen::Dark), Step::Continue);
        }
        assert_eq!(b.record(FrameSeen::Dark), Step::NoUsableFrames);

        let mut b = FrameBudget::new(3, false, false);
        for _ in 0..7 {
            assert_eq!(b.record(FrameSeen::Dark), Step::Continue);
        }
        assert_eq!(b.record(FrameSeen::NoFace), Step::Continue);
        // Ninth attempt: usable frames, but no face.
        assert_eq!(b.record(FrameSeen::NoFace), Step::NoFace);
    }

    /// A lens covered by a shutter reads flat at the black level.
    #[test]
    fn dark_frame_stats_of_a_flat_frame() {
        let f = dark_frame_stats(41, &[1u8; 1000]);
        assert_eq!(
            f,
            DarkFrame {
                sequence: 41,
                mean: 1.0,
                p99: 1
            }
        );
    }

    /// A small bright patch (2% of pixels) moves p99 but barely moves the
    /// mean, so p99 is what separates "sensor sees a scene" from "covered".
    #[test]
    fn dark_frame_stats_see_a_small_bright_patch() {
        let mut data = vec![2u8; 980];
        data.extend(std::iter::repeat_n(120u8, 20));
        let f = dark_frame_stats(7, &data);
        assert_eq!(f.p99, 120);
        assert!((f.mean - 4.36).abs() < 1e-4, "mean {}", f.mean);
    }

    #[test]
    fn dark_frames_are_described_in_order() {
        let frames = [
            DarkFrame {
                sequence: 41,
                mean: 1.0,
                p99: 2,
            },
            DarkFrame {
                sequence: 42,
                mean: 4.36,
                p99: 120,
            },
        ];
        assert_eq!(
            describe_dark_frames(&frames),
            "seq 41: 1.0/2, seq 42: 4.4/120"
        );
        assert_eq!(describe_dark_frames(&[]), "");
    }
}

#[cfg(test)]
mod preview_tests {
    use super::*;

    fn frame(w: u32, h: u32, is_dark: bool) -> visage_hw::Frame {
        // A horizontal gradient, so a downscale that samples nothing (or the
        // same pixel repeatedly) is distinguishable from one that works.
        let data = (0..(w as usize * h as usize))
            .map(|i| (i % w as usize) as u8)
            .collect();
        visage_hw::Frame {
            data,
            width: w,
            height: h,
            timestamp: std::time::Instant::now(),
            sequence: 0,
            is_dark,
        }
    }

    /// Pin the constant itself, with a literal.
    ///
    /// The test below asserts the downscaler honours `PREVIEW_MAX_EDGE`. It
    /// cannot notice `PREVIEW_MAX_EDGE` being raised, because it compares
    /// against that same constant — so on its own it would pass just as well
    /// at 2000px, and the "too small to be a useful biometric capture"
    /// argument would have quietly evaporated.
    ///
    /// Raising this is a security decision about how much biometric detail
    /// leaves the daemon, not a tuning knob. Changing the literal here is the
    /// deliberate act that says someone thought about it.
    #[test]
    fn the_preview_size_cap_is_what_the_security_argument_assumes() {
        assert_eq!(
            PREVIEW_MAX_EDGE, 160,
            "PREVIEW_MAX_EDGE changed. The preview is allowed out of the daemon \
             because it is too coarse to be a useful capture; raising it needs a \
             threat-model review, not just a passing test suite."
        );
    }

    /// That the downscaler honours whatever the cap currently is.
    #[test]
    fn preview_never_exceeds_the_declared_maximum_edge() {
        for (w, h) in [
            (640, 480),
            (640, 360),
            (1280, 720),
            (1920, 1080),
            (320, 240),
        ] {
            let p = to_preview(&frame(w, h, false));
            let longest = p.width.max(p.height) as usize;
            assert!(
                longest <= PREVIEW_MAX_EDGE,
                "{w}x{h} produced a {}x{} preview, longest edge {longest} > {PREVIEW_MAX_EDGE}",
                p.width,
                p.height
            );
        }
    }

    #[test]
    fn data_length_matches_the_declared_dimensions() {
        // A client reading `width * height` bytes must not run off the end, and
        // must not silently render a truncated frame as if it were whole.
        for (w, h) in [(640, 480), (640, 360), (100, 80), (161, 161)] {
            let p = to_preview(&frame(w, h, false));
            assert_eq!(
                p.data.len(),
                p.width as usize * p.height as usize,
                "{w}x{h}: data is {} bytes for a {}x{} frame",
                p.data.len(),
                p.width,
                p.height
            );
        }
    }

    #[test]
    fn a_frame_already_small_enough_is_left_alone() {
        let src = frame(100, 80, false);
        let p = to_preview(&src);
        assert_eq!((p.width, p.height), (100, 80));
        assert_eq!(p.data, src.data, "a no-op downscale altered the pixels");
    }

    /// Without this the tests above would pass on a downscaler that returned a
    /// correctly-sized block of zeros.
    #[test]
    fn downscaling_actually_samples_the_source() {
        let p = to_preview(&frame(640, 480, false));
        let distinct: std::collections::HashSet<u8> = p.data.iter().copied().collect();
        assert!(
            distinct.len() > 1,
            "every pixel is identical ({distinct:?}) — the downscale is not reading the source"
        );
    }

    #[test]
    fn the_dark_verdict_survives_downscaling() {
        // The client renders "too dark" from this flag; losing it here would
        // turn the most useful feedback into a silently normal-looking frame.
        assert!(to_preview(&frame(640, 480, true)).is_dark);
        assert!(!to_preview(&frame(640, 480, false)).is_dark);
    }
}
