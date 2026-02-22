//! IR emitter control via UVC extension unit commands.
//!
//! Sends vendor-specific UVC control bytes to activate IR illumination
//! on Windows Hello-compatible cameras, replacing the external
//! `linux-enable-ir-emitter` dependency.

use crate::quirks::{get_usb_ids, lookup_quirk, CameraQuirk};
use std::os::unix::io::AsRawFd;
use thiserror::Error;

/// `UVCIOC_CTRL_QUERY` = `_IOWR('u', 0x21, struct uvc_xu_control_query)`
/// where sizeof(struct uvc_xu_control_query) = 16 bytes (verified by assert below).
const UVCIOC_CTRL_QUERY: libc::c_ulong = 0xC010_7521;

/// UVC_SET_CUR: set the current value of a control.
const UVC_SET_CUR: u8 = 0x01;

/// Mirror of `struct uvc_xu_control_query` from `<linux/uvcvideo.h>`.
///
/// Layout (64-bit Linux):
///   unit:u8 selector:u8 query:u8 _pad0:u8 size:u16 _pad1:u16 data:*mut u8
/// Total: 1+1+1+1+2+2+8 = 16 bytes — verified by compile-time assert.
#[repr(C)]
struct UvcXuControlQuery {
    unit: u8,
    selector: u8,
    query: u8,
    _pad0: u8,
    size: u16,
    _pad1: u16,
    data: *mut u8,
}

const _SIZE_ASSERT: () = assert!(
    std::mem::size_of::<UvcXuControlQuery>() == 16,
    "UvcXuControlQuery must be 16 bytes to match the kernel ABI"
);

/// Controls the IR emitter on a UVC camera.
pub struct IrEmitter {
    device_path: String,
    quirk: &'static CameraQuirk,
}

#[derive(Debug, Error)]
pub enum EmitterError {
    #[error("no quirk for device {0}")]
    NoQuirk(String),
    #[error("failed to open device: {0}")]
    Open(std::io::Error),
    #[error("UVC ioctl failed: {0}")]
    Ioctl(std::io::Error),
}

impl IrEmitter {
    /// Construct an `IrEmitter` for the given `/dev/videoN` device.
    ///
    /// Returns `None` if the device has no entry in the quirk database.
    pub fn for_device(device_path: &str) -> Option<Self> {
        let (vid, pid) = get_usb_ids(device_path)?;
        let quirk = lookup_quirk(vid, pid)?;
        Some(Self {
            device_path: device_path.to_string(),
            quirk,
        })
    }

    /// Activate the IR emitter by sending the quirk's control bytes.
    pub fn activate(&self) -> Result<(), EmitterError> {
        tracing::debug!(device = %self.device_path, "activating IR emitter");
        let mut payload = self.quirk.emitter.control_bytes.clone();
        self.send_uvc_control(&mut payload)
    }

    /// Deactivate the IR emitter by sending zeros of the same length.
    pub fn deactivate(&self) -> Result<(), EmitterError> {
        tracing::debug!(device = %self.device_path, "deactivating IR emitter");
        let mut payload = vec![0u8; self.quirk.emitter.control_bytes.len()];
        self.send_uvc_control(&mut payload)
    }

    /// Device path this emitter controls.
    pub fn device_path(&self) -> &str {
        &self.device_path
    }

    /// Human-readable name from the quirk database.
    pub fn name(&self) -> &str {
        &self.quirk.device.name
    }

    fn send_uvc_control(&self, payload: &mut [u8]) -> Result<(), EmitterError> {
        // Open the device with read+write access — needed for UVC ioctls.
        // We open a second fd here rather than requiring AsRawFd on Camera.
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.device_path)
            .map_err(EmitterError::Open)?;

        let mut query = UvcXuControlQuery {
            unit: self.quirk.emitter.unit,
            selector: self.quirk.emitter.selector,
            query: UVC_SET_CUR,
            _pad0: 0,
            size: payload.len() as u16,
            _pad1: 0,
            data: payload.as_mut_ptr(),
        };

        // SAFETY:
        // - fd is valid for the lifetime of `file`
        // - `query` is correctly sized and repr(C), matching the kernel ABI
        // - `payload` is valid and lives for the duration of this call
        let ret = unsafe {
            libc::ioctl(
                file.as_raw_fd(),
                UVCIOC_CTRL_QUERY,
                &mut query as *mut UvcXuControlQuery,
            )
        };

        if ret < 0 {
            Err(EmitterError::Ioctl(std::io::Error::last_os_error()))
        } else {
            Ok(())
        }
    }
}
