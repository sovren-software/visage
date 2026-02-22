//! Hardware quirks database.
//!
//! Maps camera USB VID:PID to UVC extension unit control parameters
//! needed to activate their IR emitters. Quirk files are embedded at
//! compile time from `contrib/hw/*.toml`.

use serde::Deserialize;
use std::sync::OnceLock;

/// Compile-time embedded quirk for the ASUS Zenbook 14 UM3406HA IR camera.
const QUIRK_04F2_B6D9: &str = include_str!("../../../contrib/hw/04f2-b6d9.toml");

static QUIRK_DB: OnceLock<Vec<QuirkFile>> = OnceLock::new();

/// Top-level quirk file structure (one per `contrib/hw/*.toml`).
#[derive(Debug, Clone, Deserialize)]
pub struct QuirkFile {
    pub device: DeviceInfo,
    pub emitter: EmitterInfo,
}

/// Camera identification fields from the `[device]` section.
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceInfo {
    pub vendor_id: u16,
    pub product_id: u16,
    pub name: String,
}

/// UVC extension unit parameters from the `[emitter]` section.
#[derive(Debug, Clone, Deserialize)]
pub struct EmitterInfo {
    pub unit: u8,
    pub selector: u8,
    /// Payload bytes sent to activate the emitter.
    /// Zeros of the same length deactivate it.
    pub control_bytes: Vec<u8>,
}

/// Public alias used by `IrEmitter`.
pub type CameraQuirk = QuirkFile;

fn quirk_db() -> &'static Vec<QuirkFile> {
    QUIRK_DB.get_or_init(|| {
        let mut db = Vec::new();
        for src in [QUIRK_04F2_B6D9] {
            match toml::from_str::<QuirkFile>(src) {
                Ok(q) => db.push(q),
                Err(e) => eprintln!("visage-hw: bad quirk TOML: {e}"),
            }
        }
        db
    })
}

/// Look up a quirk by USB vendor:product ID.
/// Returns a `'static` reference into the embedded database.
pub fn lookup_quirk(vid: u16, pid: u16) -> Option<&'static QuirkFile> {
    quirk_db()
        .iter()
        .find(|q| q.device.vendor_id == vid && q.device.product_id == pid)
}

/// List all known quirks.
pub fn list_quirks() -> &'static [QuirkFile] {
    quirk_db()
}

/// Read USB VID:PID from sysfs for a `/dev/videoN` device.
///
/// Returns `None` if the device is not USB or sysfs is unavailable.
pub fn get_usb_ids(device_path: &str) -> Option<(u16, u16)> {
    // /dev/video2 → "video2"
    let dev_name = std::path::Path::new(device_path).file_name()?.to_str()?;
    // /sys/class/video4linux/video2/device is a symlink to the USB interface dir
    let device_link = format!("/sys/class/video4linux/{dev_name}/device");
    // Resolve: interface dir → parent = USB device dir
    let interface_dir = std::fs::canonicalize(&device_link).ok()?;
    let usb_device_dir = interface_dir.parent()?;

    let vid_str = std::fs::read_to_string(usb_device_dir.join("idVendor")).ok()?;
    let pid_str = std::fs::read_to_string(usb_device_dir.join("idProduct")).ok()?;

    let vid = u16::from_str_radix(vid_str.trim(), 16).ok()?;
    let pid = u16::from_str_radix(pid_str.trim(), 16).ok()?;
    Some((vid, pid))
}
