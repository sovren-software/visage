//! visage-hw — Hardware abstraction for camera capture and IR emitter control.
//!
//! Provides V4L2-based camera access and UVC control byte management
//! for IR emitter activation.

pub mod camera;
pub mod frame;
pub mod ir_emitter;
pub mod quirks;

pub use camera::{Camera, CameraError, PixelFormat};
pub use frame::Frame;
