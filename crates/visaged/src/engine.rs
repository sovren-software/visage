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
    #[error("liveness check failed: landmark displacement {displacement:.3} px < threshold {threshold:.3} px")]
    LivenessCheckFailed { displacement: f32, threshold: f32 },
    #[error("verification timed out")]
    VerifyTimeout,
    #[error("engine thread exited")]
    ChannelClosed,
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
        reply: oneshot::Sender<Result<EnrollResult, EngineError>>,
    },
    Verify {
        gallery: Vec<FaceModel>,
        threshold: f32,
        frames_count: usize,
        timeout: std::time::Duration,
        liveness_enabled: bool,
        liveness_min_displacement: f32,
        reply: oneshot::Sender<Result<VerifyResult, EngineError>>,
    },
}

/// Clone-safe handle to the engine thread.
#[derive(Clone)]
pub struct EngineHandle {
    tx: mpsc::Sender<EngineRequest>,
}

impl EngineHandle {
    /// Request enrollment: capture frames, detect best face, extract embedding.
    pub async fn enroll(&self, frames_count: usize) -> Result<EnrollResult, EngineError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(EngineRequest::Enroll {
                frames_count,
                reply: reply_tx,
            })
            .await
            .map_err(|_| EngineError::ChannelClosed)?;
        reply_rx.await.map_err(|_| EngineError::ChannelClosed)?
    }

    /// Request verification: capture frames, detect, extract, compare against gallery.
    pub async fn verify(
        &self,
        gallery: Vec<FaceModel>,
        threshold: f32,
        frames_count: usize,
        timeout: std::time::Duration,
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

    let mut detector = visage_core::FaceDetector::load(scrfd_path)?;
    tracing::info!(path = scrfd_path, "SCRFD detector loaded");

    let mut recognizer = visage_core::FaceRecognizer::load(arcface_path)?;
    tracing::info!(path = arcface_path, "ArcFace recognizer loaded");

    // Probe for IR emitter quirk
    let emitter: Option<IrEmitter> = if emitter_enabled {
        match IrEmitter::for_device(camera_device) {
            Some(e) => {
                tracing::info!(name = %e.name(), device = %e.device_path(), "IR emitter found");
                Some(e)
            }
            None => {
                tracing::warn!(
                    device = camera_device,
                    "no IR emitter quirk for device; proceeding without illumination"
                );
                None
            }
        }
    } else {
        tracing::info!("IR emitter disabled via VISAGE_EMITTER_ENABLED=0");
        None
    };

    // Discard warmup frames for camera AGC/AE stabilization
    if warmup_frames > 0 {
        tracing::info!(count = warmup_frames, "discarding warmup frames");
        for _ in 0..warmup_frames {
            let _ = camera.capture_frame();
        }
    }

    let (tx, mut rx) = mpsc::channel::<EngineRequest>(4);

    std::thread::Builder::new()
        .name("visage-engine".into())
        .spawn(move || {
            tracing::info!("engine thread started");
            while let Some(req) = rx.blocking_recv() {
                match req {
                    EngineRequest::Enroll {
                        frames_count,
                        reply,
                    } => {
                        let result = run_enroll(
                            &camera,
                            &emitter,
                            &mut detector,
                            &mut recognizer,
                            frames_count,
                        );
                        let _ = reply.send(result);
                    }
                    EngineRequest::Verify {
                        gallery,
                        threshold,
                        frames_count,
                        timeout,
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
                            liveness_enabled,
                            liveness_min_displacement,
                        );
                        let _ = reply.send(result);
                    }
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
) -> Result<EnrollResult, EngineError> {
    activate_emitter(emitter);
    let capture_result = camera.capture_frames(frames_count);
    deactivate_emitter(emitter);

    let (frames, dark_skipped) = capture_result?;
    tracing::debug!(
        captured = frames.len(),
        dark_skipped,
        "enroll: captured frames"
    );

    if frames.is_empty() {
        return Err(EngineError::NoFaceDetected);
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
    liveness_enabled: bool,
    liveness_min_displacement: f32,
) -> Result<VerifyResult, EngineError> {
    if std::time::Instant::now() > deadline {
        return Err(EngineError::VerifyTimeout);
    }

    activate_emitter(emitter);
    let capture_result = camera.capture_frames(frames_count);
    deactivate_emitter(emitter);

    if std::time::Instant::now() > deadline {
        return Err(EngineError::VerifyTimeout);
    }

    let (frames, dark_skipped) = capture_result?;
    tracing::debug!(
        captured = frames.len(),
        dark_skipped,
        "verify: captured frames"
    );

    if frames.is_empty() {
        return Err(EngineError::NoFaceDetected);
    }

    let matcher = CosineMatcher;
    let mut best_result: Option<MatchResult> = None;
    let mut best_quality = 0.0f32;
    let mut any_face_detected = false;
    let mut landmark_sequence: Vec<[(f32, f32); 5]> = Vec::new();

    for frame in &frames {
        let faces = detector.detect(&frame.data, frame.width, frame.height)?;
        let Some(face) = faces.first() else {
            continue;
        };
        any_face_detected = true;

        // Collect landmarks for liveness check
        if let Some(landmarks) = face.landmarks {
            landmark_sequence.push(landmarks);
        }

        let embedding = recognizer.extract(&frame.data, frame.width, frame.height, face)?;
        let result = matcher.compare(&embedding, gallery, threshold);

        let is_better = match &best_result {
            None => true,
            Some(prev) => result.similarity > prev.similarity,
        };
        if is_better {
            best_quality = face.confidence;
            best_result = Some(result);
        }
    }

    if !any_face_detected {
        return Err(EngineError::NoFaceDetected);
    }

    // If no match result at all, return a non-match
    let result = best_result.unwrap_or(MatchResult {
        matched: false,
        similarity: 0.0,
        model_id: None,
        model_label: None,
    });

    // --- Passive liveness check ---
    // Run after detection loop so we always have full landmark data.
    // Only gates the result when a match would otherwise succeed.
    if liveness_enabled && result.matched {
        let liveness =
            check_landmark_stability(&landmark_sequence, Some(liveness_min_displacement));

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
        best_quality,
    })
}
