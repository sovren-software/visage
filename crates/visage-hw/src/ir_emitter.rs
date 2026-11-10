//! IR emitter control via UVC extension unit commands.
//!
//! Sends vendor-specific UVC control bytes to activate IR illumination
//! on Windows Hello-compatible cameras, replacing the external
//! `linux-enable-ir-emitter` dependency.

use crate::quirks::{get_usb_ids, lookup_quirk, CameraQuirk, EmitterControl};
use std::cell::RefCell;
use std::fs::{File, OpenOptions};
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

    /// Additional options for cameras with special file descriptor (fd) rules
    active_fd: RefCell<Option<File>>,
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
            active_fd: RefCell::new(None),
        })
    }

    /// Activate the IR emitter by sending the quirk's controls in order.
    ///
    /// If activating any control fails, controls already set are rolled back
    /// (switched off) in reverse order so the camera is not left partially illuminated.
    pub fn activate(&self) -> Result<(), EmitterError> {
        tracing::debug!(device = %self.device_path, "activating IR emitter");

        // reset_on_close devices forget the control the moment the fd closes,
        // so open a fresh fd, set controls, and hold it open until deactivate().
        if self.quirk.emitter.reset_on_close {
            self.active_fd.borrow_mut().take(); // drop any stale fd first
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&self.device_path)
                .map_err(EmitterError::Open)?;
            Self::activate_controls_via_fd(&file, &self.quirk.emitter.controls)?;
            *self.active_fd.borrow_mut() = Some(file);
            return Ok(());
        }

        // Default: open, set controls, close.
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.device_path)
            .map_err(EmitterError::Open)?;
        Self::activate_controls_via_fd(&file, &self.quirk.emitter.controls)
    }

    /// Deactivate the IR emitter after a capture.
    ///
    /// Controls are deactivated in reverse order. Every control is attempted even if
    /// an earlier one fails, and the first error encountered is returned.
    pub fn deactivate(&self) -> Result<(), EmitterError> {
        tracing::debug!(device = %self.device_path, "deactivating IR emitter");

        // reset_on_close devices reset the control when the fd closes, so send
        // "off" through the held fd, then close it to return control to default.
        if self.quirk.emitter.reset_on_close {
            let result = match self.active_fd.borrow().as_ref() {
                Some(file) => Self::deactivate_controls_via_fd(file, &self.quirk.emitter.controls),
                None => Ok(()),
            };
            self.active_fd.borrow_mut().take();
            return result;
        }

        // Default: open, send "off", close.
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.device_path)
            .map_err(EmitterError::Open)?;
        Self::deactivate_controls_via_fd(&file, &self.quirk.emitter.controls)
    }

    /// Device path this emitter controls.
    pub fn device_path(&self) -> &str {
        &self.device_path
    }

    /// Human-readable name from the quirk database.
    pub fn name(&self) -> &str {
        &self.quirk.device.name
    }

    /// Activate controls in order, rolling back previously set controls in reverse
    /// order on failure.
    fn activate_controls_via_fd(
        file: &File,
        controls: &[EmitterControl],
    ) -> Result<(), EmitterError> {
        for (i, ctrl) in controls.iter().enumerate() {
            let mut payload = ctrl.control_bytes.clone();
            if let Err(err) = Self::send_xu_control(file, ctrl.unit, ctrl.selector, &mut payload) {
                tracing::warn!(
                    unit = ctrl.unit,
                    selector = ctrl.selector,
                    error = %err,
                    "failed to activate emitter control, rolling back"
                );
                for prev_ctrl in controls[..i].iter().rev() {
                    let mut off = prev_ctrl.off_payload();
                    let _ =
                        Self::send_xu_control(file, prev_ctrl.unit, prev_ctrl.selector, &mut off);
                }
                return Err(err);
            }
        }
        Ok(())
    }

    /// Deactivate controls in reverse order, attempting all controls and returning
    /// the first error encountered.
    fn deactivate_controls_via_fd(
        file: &File,
        controls: &[EmitterControl],
    ) -> Result<(), EmitterError> {
        let mut first_err = None;
        for ctrl in controls.iter().rev() {
            let mut payload = ctrl.off_payload();
            if let Err(err) = Self::send_xu_control(file, ctrl.unit, ctrl.selector, &mut payload) {
                tracing::warn!(
                    unit = ctrl.unit,
                    selector = ctrl.selector,
                    error = %err,
                    "failed to deactivate emitter control"
                );
                if first_err.is_none() {
                    first_err = Some(err);
                }
            }
        }
        match first_err {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    /// Send one UVC `SET_CUR` control over an already-open fd.
    fn send_xu_control(
        file: &File,
        unit: u8,
        selector: u8,
        payload: &mut [u8],
    ) -> Result<(), EmitterError> {
        let mut query = UvcXuControlQuery {
            unit,
            selector,
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
