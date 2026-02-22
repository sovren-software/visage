//! IR emitter control via UVC extension unit commands.
//!
//! Replaces the external `linux-enable-ir-emitter` dependency.
//! Sends vendor-specific control bytes to activate IR illumination
//! on Windows Hello-compatible cameras.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum EmitterError {
    #[error("device not found: {0}")]
    DeviceNotFound(String),
    #[error("UVC control failed: {0}")]
    UvcControlFailed(String),
    #[error("no quirk entry for this camera")]
    NoQuirkEntry,
}

/// UVC control configuration for an IR emitter.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EmitterConfig {
    pub device: String,
    pub unit: u8,
    pub selector: u8,
    pub control_bytes: Vec<u8>,
}

/// IR emitter controller.
pub struct IrEmitter {
    pub config: EmitterConfig,
}

impl IrEmitter {
    /// Create an emitter controller from a quirks database entry.
    pub fn from_config(config: EmitterConfig) -> Self {
        Self { config }
    }

    /// Send UVC control bytes to activate the IR emitter.
    pub fn activate(&self) -> Result<(), EmitterError> {
        tracing::info!(device = %self.config.device, "activating IR emitter");
        // TODO: Open device, send UVC XU control via ioctl
        // ioctl(fd, UVCIOC_CTRL_SET, &xu_control)
        Err(EmitterError::UvcControlFailed("not implemented".into()))
    }

    /// Deactivate the IR emitter.
    pub fn deactivate(&self) -> Result<(), EmitterError> {
        tracing::info!(device = %self.config.device, "deactivating IR emitter");
        // TODO: Send deactivation control bytes
        Err(EmitterError::UvcControlFailed("not implemented".into()))
    }
}
