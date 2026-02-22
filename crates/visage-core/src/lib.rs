//! visage-core — Face detection and recognition engine.
//!
//! Uses SCRFD for face detection and ArcFace for face recognition,
//! both running via ONNX Runtime for CPU inference.

pub mod alignment;
pub mod detector;
pub mod recognizer;
pub mod types;

pub use detector::FaceDetector;
pub use recognizer::FaceRecognizer;
pub use types::{BoundingBox, CosineMatcher, Embedding, FaceModel, MatchResult, Matcher};

/// Default model directory (XDG data home).
pub fn default_model_dir() -> std::path::PathBuf {
    let base = std::env::var("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            std::path::PathBuf::from(home).join(".local/share")
        });
    base.join("visage/models")
}
