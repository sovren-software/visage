//! SCRFD face detector via ONNX Runtime.

use crate::types::BoundingBox;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum DetectorError {
    #[error("model not loaded")]
    ModelNotLoaded,
    #[error("inference failed: {0}")]
    InferenceFailed(String),
    #[error("no face detected")]
    NoFaceDetected,
}

/// SCRFD-based face detector.
pub struct FaceDetector {
    // TODO: ort::Session for SCRFD model
    _initialized: bool,
}

impl FaceDetector {
    /// Load the SCRFD ONNX model from the given path.
    pub fn load(_model_path: &str) -> Result<Self, DetectorError> {
        // TODO: Initialize ONNX Runtime session
        Ok(Self {
            _initialized: false,
        })
    }

    /// Detect faces in a frame, returning bounding boxes sorted by confidence.
    pub fn detect(&self, _frame: &[u8], _width: u32, _height: u32) -> Result<Vec<BoundingBox>, DetectorError> {
        // TODO: Preprocess frame, run inference, postprocess NMS
        Err(DetectorError::ModelNotLoaded)
    }
}
