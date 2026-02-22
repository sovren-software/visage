//! V4L2 camera capture.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum CameraError {
    #[error("device not found: {0}")]
    DeviceNotFound(String),
    #[error("capture failed: {0}")]
    CaptureFailed(String),
    #[error("device busy")]
    DeviceBusy,
}

/// Camera device handle.
pub struct Camera {
    pub device_path: String,
    pub width: u32,
    pub height: u32,
    // TODO: V4L2 file descriptor
}

impl Camera {
    /// Open a V4L2 camera device.
    pub fn open(device_path: &str) -> Result<Self, CameraError> {
        // TODO: Open device, negotiate format, start streaming
        tracing::info!(device_path, "opening camera");
        Ok(Self {
            device_path: device_path.to_string(),
            width: 640,
            height: 360,
        })
    }

    /// Capture a single frame.
    pub fn capture_frame(&self) -> Result<Vec<u8>, CameraError> {
        // TODO: Dequeue buffer from V4L2
        Err(CameraError::CaptureFailed("not implemented".into()))
    }

    /// Check if a frame is too dark (IR emitter likely off).
    pub fn is_dark_frame(frame: &[u8], threshold: f32) -> bool {
        if frame.is_empty() {
            return true;
        }
        let avg: f32 = frame.iter().map(|&b| b as f32).sum::<f32>() / frame.len() as f32;
        avg < threshold
    }
}
