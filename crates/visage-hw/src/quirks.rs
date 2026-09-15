//! Hardware quirks database.
//!
//! Maps camera USB VID:PID to UVC extension unit control parameters
//! needed to activate their IR emitters. Quirk files are embedded at
//! compile time from `contrib/hw/*.toml`.

use serde::Deserialize;
use std::sync::OnceLock;

/// Compile-time embedded quirk for the ASUS Zenbook 14 UM3406HA IR camera.
const QUIRK_04F2_B6D9: &str = include_str!("../../../contrib/hw/04f2-b6d9.toml");
/// Compile-time embedded quirk for the Lenovo ThinkPad P14s Gen 2a 21A0000RMX IR Camera.
const QUIRK_04F2_B6D0: &str = include_str!("../../../contrib/hw/04f2-b6d0.toml");
/// Compile-time embedded quirk for the Lenovo ThinkPad X1 Carbon Gen 9 20XW00FPUS IR camera.
const QUIRK_174F_2454: &str = include_str!("../../../contrib/hw/174f-2454.toml");
/// Compile-time embedded quirk for the Lenovo ThinkBook 14 MP2PQAZG IR camera.
const QUIRK_30C9_00C2: &str = include_str!("../../../contrib/hw/30c9-00c2.toml");
/// Compile-time embedded quirk for the HP OmniBook X Flip IR camera (Luxvisions 30c9:0120).
const QUIRK_30C9_0120: &str = include_str!("../../../contrib/hw/30c9-0120.toml");
/// Compile-time embedded quirk for the Lenovo ThinkPad P14s Gen 4 IR camera (Syntek 174f:11a8).
const QUIRK_174F_11A8: &str = include_str!("../../../contrib/hw/174f-11a8.toml");

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
    /// Payload bytes sent to deactivate the emitter.
    /// Defaults to zeros of `control_bytes` length.
    #[serde(default)]
    pub off_bytes: Option<Vec<u8>>,
    /// Flag `true` for cameras that reset the XU control when the controlling fd closes,
    /// making `IrEmitter` hold an fd open for the duration of each capture.
    #[serde(default)]
    pub reset_on_close: bool,
}

/// Public alias used by `IrEmitter`.
pub type CameraQuirk = QuirkFile;

/// Every embedded quirk source, paired with its filename for diagnostics.
///
/// Adding a camera means adding an `include_str!` const above and one entry
/// here. `quirk_sources_all_parse` compares this list's length against the
/// number that actually parsed, so a contribution that is embedded but
/// malformed fails CI instead of shipping inert.
const QUIRK_SOURCES: &[(&str, &str)] = &[
    ("04f2-b6d9.toml", QUIRK_04F2_B6D9),
    ("04f2-b6d0.toml", QUIRK_04F2_B6D0),
    ("174f-2454.toml", QUIRK_174F_2454),
    ("30c9-00c2.toml", QUIRK_30C9_00C2),
    ("30c9-0120.toml", QUIRK_30C9_0120),
    ("174f-11a8.toml", QUIRK_174F_11A8),
];

fn quirk_db() -> &'static Vec<QuirkFile> {
    QUIRK_DB.get_or_init(|| {
        let mut db = Vec::new();
        for (file, src) in QUIRK_SOURCES {
            match toml::from_str::<QuirkFile>(src) {
                Ok(q) => db.push(q),
                // Deliberately does NOT panic. This runs inside a daemon that
                // authenticates logins; one bad contributed quirk must not stop
                // it starting. The cost is that a malformed entry is invisible
                // at runtime — a camera with a broken quirk looks exactly like a
                // camera with no quirk. The tests below are what make it visible.
                Err(e) => eprintln!("visage-hw: bad quirk TOML in {file}: {e}"),
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

/// Read the kernel driver name for a `/dev/videoN` device from sysfs.
///
/// Returns the basename of the `driver` symlink, e.g. `"uvcvideo"` or
/// `"intel_ipu6_imx_phy"`. Returns `None` if the sysfs entry is absent
/// (device not enumerated via udev, or non-Linux system).
pub fn get_driver(device_path: &str) -> Option<String> {
    let dev_name = std::path::Path::new(device_path).file_name()?.to_str()?;
    let driver_link = format!("/sys/class/video4linux/{dev_name}/device/driver");
    let resolved = std::fs::read_link(&driver_link).ok()?;
    resolved.file_name()?.to_str().map(|s| s.to_string())
}

/// Returns `true` if this device is driven by Intel IPU6 (not UVC/V4L2).
///
/// IPU6 cameras use Intel's proprietary camera HAL and are **not** supported
/// by Visage's V4L2/UVC pipeline. Callers should warn the user and suggest
/// they check for a `uvcvideo`-driven IR device instead.
pub fn is_ipu6_camera(device_path: &str) -> bool {
    get_driver(device_path)
        .map(|d| d.contains("ipu6") || d.contains("intel_ipu"))
        .unwrap_or(false)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The guard this module exists for.
    ///
    /// `quirk_db()` skips a malformed TOML rather than panicking, so an
    /// embedded-but-broken contribution compiles, ships, and silently never
    /// fires — indistinguishable at runtime from a camera that simply has no
    /// quirk. Comparing parsed count against source count is what turns that
    /// into a CI failure. Deriving the expected number from `QUIRK_SOURCES`
    /// rather than hard-coding it means the assertion cannot drift as cameras
    /// are added.
    #[test]
    fn quirk_sources_all_parse() {
        let parsed = quirk_db().len();
        assert_eq!(
            parsed,
            QUIRK_SOURCES.len(),
            "{} of {} embedded quirk files failed to parse and were silently dropped \
             (stderr names which)",
            QUIRK_SOURCES.len() - parsed,
            QUIRK_SOURCES.len()
        );
    }

    /// Parsing is not enough — a quirk nothing can look up is still inert.
    #[test]
    fn every_quirk_is_reachable_by_lookup() {
        for q in quirk_db() {
            let (vid, pid) = (q.device.vendor_id, q.device.product_id);
            let found = lookup_quirk(vid, pid)
                .unwrap_or_else(|| panic!("{vid:04X}:{pid:04X} parsed but lookup_quirk missed it"));
            assert_eq!(found.device.name, q.device.name);
        }
    }

    /// Two entries claiming one device makes `lookup_quirk` order-dependent,
    /// so whichever is listed first silently wins.
    #[test]
    fn no_duplicate_vid_pid() {
        let mut seen = std::collections::HashSet::new();
        for q in quirk_db() {
            let key = (q.device.vendor_id, q.device.product_id);
            assert!(
                seen.insert(key),
                "duplicate quirk for {:04X}:{:04X} ({})",
                key.0,
                key.1,
                q.device.name
            );
        }
    }

    /// An empty payload writes nothing to the extension unit, and an `off_bytes`
    /// of a different length cannot deactivate what `control_bytes` activated.
    /// Both parse cleanly, so only an assertion catches them.
    #[test]
    fn emitter_payloads_are_coherent() {
        for q in quirk_db() {
            let n = &q.device.name;
            assert!(
                !q.emitter.control_bytes.is_empty(),
                "{n}: empty control_bytes"
            );
            if let Some(off) = &q.emitter.off_bytes {
                assert_eq!(
                    off.len(),
                    q.emitter.control_bytes.len(),
                    "{n}: off_bytes length differs from control_bytes"
                );
            }
        }
    }
}
