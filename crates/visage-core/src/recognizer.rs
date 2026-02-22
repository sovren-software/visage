//! ArcFace face recognizer via ONNX Runtime.

use crate::types::{BoundingBox, Embedding};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum RecognizerError {
    #[error("model not loaded")]
    ModelNotLoaded,
    #[error("inference failed: {0}")]
    InferenceFailed(String),
}

/// ArcFace-based face recognizer.
pub struct FaceRecognizer {
    // TODO: ort::Session for ArcFace model
    _initialized: bool,
}

impl FaceRecognizer {
    /// Load the ArcFace ONNX model from the given path.
    pub fn load(_model_path: &str) -> Result<Self, RecognizerError> {
        // TODO: Initialize ONNX Runtime session
        Ok(Self {
            _initialized: false,
        })
    }

    /// Extract a face embedding from a cropped face region.
    pub fn extract(
        &self,
        _frame: &[u8],
        _width: u32,
        _height: u32,
        _face: &BoundingBox,
    ) -> Result<Embedding, RecognizerError> {
        // TODO: Crop, align, preprocess, run inference
        Err(RecognizerError::ModelNotLoaded)
    }
}
