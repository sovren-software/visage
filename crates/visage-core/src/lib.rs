//! visage-core — Face detection and recognition engine.
//!
//! Uses SCRFD for face detection and ArcFace for face recognition,
//! both running via ONNX Runtime for CPU inference.

pub mod detector;
pub mod recognizer;
pub mod types;

pub use types::{BoundingBox, Embedding, FaceModel};
