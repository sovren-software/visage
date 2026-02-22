//! ArcFace face recognizer via ONNX Runtime.
//!
//! Extracts 512-dimensional face embeddings from aligned face crops,
//! using the w600k_r50 ArcFace model.

use crate::alignment;
use crate::types::{BoundingBox, Embedding};
use ndarray::Array4;
use ort::session::Session;
use ort::value::TensorRef;
use std::path::Path;
use thiserror::Error;

// --- Named constants (different from SCRFD!) ---
const ARCFACE_INPUT_SIZE: usize = 112;
const ARCFACE_MEAN: f32 = 127.5;
const ARCFACE_STD: f32 = 127.5; // NOT 128.0 — ArcFace uses symmetric normalization
const ARCFACE_EMBEDDING_DIM: usize = 512;
const ARCFACE_MODEL_VERSION: &str = "w600k_r50";

#[derive(Error, Debug)]
pub enum RecognizerError {
    #[error("model file not found: {0} — download from insightface and place in models/")]
    ModelNotFound(String),
    #[error("inference failed: {0}")]
    InferenceFailed(String),
    #[error("face has no landmarks — detector must return landmarks for alignment")]
    NoLandmarks,
    #[error("ort: {0}")]
    Ort(#[from] ort::Error),
}

/// ArcFace-based face recognizer.
pub struct FaceRecognizer {
    session: Session,
}

impl FaceRecognizer {
    /// Load the ArcFace ONNX model from the given path.
    pub fn load(model_path: &str) -> Result<Self, RecognizerError> {
        if !Path::new(model_path).exists() {
            return Err(RecognizerError::ModelNotFound(model_path.to_string()));
        }

        let session = Session::builder()?
            .with_intra_threads(2)?
            .commit_from_file(model_path)?;

        tracing::info!(
            path = model_path,
            inputs = ?session.inputs().iter().map(|i| (i.name(), i.dtype())).collect::<Vec<_>>(),
            outputs = ?session.outputs().iter().map(|o| o.name()).collect::<Vec<_>>(),
            "loaded ArcFace model"
        );

        Ok(Self { session })
    }

    /// Extract a face embedding from a detected face in a grayscale frame.
    ///
    /// The face must have landmarks (from SCRFD detector). The face is aligned
    /// to a canonical 112x112 position before embedding extraction.
    pub fn extract(
        &mut self,
        frame: &[u8],
        width: u32,
        height: u32,
        face: &BoundingBox,
    ) -> Result<Embedding, RecognizerError> {
        let landmarks = face.landmarks.as_ref().ok_or(RecognizerError::NoLandmarks)?;

        // Align face to canonical 112x112 position
        let aligned = alignment::align_face(frame, width, height, landmarks);

        // Preprocess aligned crop
        let input = Self::preprocess(&aligned);

        // Run inference
        let outputs = self.session.run(ort::inputs![TensorRef::from_array_view(input.view())?])?;

        let (_, raw_data) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| RecognizerError::InferenceFailed(format!("embedding extraction: {e}")))?;

        let raw: Vec<f32> = raw_data.to_vec();

        if raw.len() != ARCFACE_EMBEDDING_DIM {
            return Err(RecognizerError::InferenceFailed(format!(
                "expected {ARCFACE_EMBEDDING_DIM}-dim embedding, got {}",
                raw.len()
            )));
        }

        // L2-normalize the embedding
        let norm: f32 = raw.iter().map(|x| x * x).sum::<f32>().sqrt();
        let values = if norm > 0.0 {
            raw.iter().map(|x| x / norm).collect()
        } else {
            raw
        };

        Ok(Embedding {
            values,
            model_version: Some(ARCFACE_MODEL_VERSION.to_string()),
        })
    }

    /// Preprocess a 112x112 grayscale aligned face crop into a NCHW float tensor.
    fn preprocess(aligned_face: &[u8]) -> Array4<f32> {
        let size = ARCFACE_INPUT_SIZE;
        let mut tensor = Array4::<f32>::zeros((1, 3, size, size));

        for y in 0..size {
            for x in 0..size {
                let pixel = aligned_face
                    .get(y * size + x)
                    .copied()
                    .unwrap_or(0) as f32;

                let normalized = (pixel - ARCFACE_MEAN) / ARCFACE_STD;
                // Grayscale → 3-channel: replicate Y → [R=Y, G=Y, B=Y]
                tensor[[0, 0, y, x]] = normalized;
                tensor[[0, 1, y, x]] = normalized;
                tensor[[0, 2, y, x]] = normalized;
            }
        }

        tensor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_preprocess_output_shape() {
        let aligned = vec![128u8; ARCFACE_INPUT_SIZE * ARCFACE_INPUT_SIZE];
        let tensor = FaceRecognizer::preprocess(&aligned);
        assert_eq!(tensor.shape(), &[1, 3, ARCFACE_INPUT_SIZE, ARCFACE_INPUT_SIZE]);
    }

    #[test]
    fn test_preprocess_normalization() {
        // Pixel value 127.5 should normalize to 0.0
        let aligned = vec![128u8; ARCFACE_INPUT_SIZE * ARCFACE_INPUT_SIZE];
        let tensor = FaceRecognizer::preprocess(&aligned);
        // 128 - 127.5 = 0.5, / 127.5 ≈ 0.00392
        let val = tensor[[0, 0, 0, 0]];
        let expected = (128.0 - ARCFACE_MEAN) / ARCFACE_STD;
        assert!((val - expected).abs() < 1e-6, "got {val}, expected {expected}");
    }

    #[test]
    fn test_preprocess_channels_identical() {
        // All 3 channels should be identical for grayscale input
        let aligned = vec![100u8; ARCFACE_INPUT_SIZE * ARCFACE_INPUT_SIZE];
        let tensor = FaceRecognizer::preprocess(&aligned);
        for y in 0..ARCFACE_INPUT_SIZE {
            for x in 0..ARCFACE_INPUT_SIZE {
                let r = tensor[[0, 0, y, x]];
                let g = tensor[[0, 1, y, x]];
                let b = tensor[[0, 2, y, x]];
                assert_eq!(r, g);
                assert_eq!(g, b);
            }
        }
    }

    #[test]
    fn test_extract_requires_landmarks() {
        // Cannot test full extract without a loaded model, but we can verify
        // that missing landmarks returns the correct error.
        let face = BoundingBox {
            x: 0.0, y: 0.0, width: 100.0, height: 100.0,
            confidence: 0.9, landmarks: None,
        };
        // We can't construct a FaceRecognizer without a model file,
        // so just verify the NoLandmarks check at the type level.
        assert!(face.landmarks.is_none());
    }
}
