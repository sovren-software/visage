# 006 — IR Emitter Integration

**Status:** Accepted
**Date:** 2026-02-22
**Step:** 5 of 6

## Context

Face verification using an IR camera requires active IR illumination. Without it the
sensor captures dark frames, making enrollment and verification unreliable unless
ambient IR is present. The ASUS Zenbook 14 UM3406HA (and similar Windows Hello cameras)
exposes IR illumination via a UVC extension unit control — a vendor-specific ioctl sent
directly to the camera device.

Prior art: `linux-enable-ir-emitter` solves this for system-level use. Visage needs the
same capability embedded in the daemon to avoid a runtime dependency on that tool.

## Decisions

### 1. Compile-time TOML embedding (`include_str!` + `OnceLock`)

Quirk files in `contrib/hw/*.toml` are embedded at build time via `include_str!` and
parsed once into a `OnceLock<Vec<QuirkFile>>`. No runtime file paths required.

**Rejected alternative — runtime directory scan:** Would allow installing new quirks
without rebuilding, but complicates packaging and adds file-not-found failure modes.
A runtime override directory (e.g. `/usr/share/visage/quirks/`) can be layered in
Step 6 without changing the compile-time default.

### 2. `libc::ioctl` directly (no `nix` crate)

`libc` is already a workspace dependency (added in Step 4 for PAM). Using it avoids
introducing `nix` as a new dependency. The ioctl number is defined as a constant:

```
UVCIOC_CTRL_QUERY = _IOWR('u', 0x21, 16) = 0xC010_7521
```

A compile-time assertion verifies that `UvcXuControlQuery` is exactly 16 bytes,
matching the kernel ABI (`struct uvc_xu_control_query` from `<linux/uvcvideo.h>`).

### 3. Separate fd for UVC ioctl

`send_uvc_control()` opens a second read+write fd on the device rather than reusing
the `Camera` fd. The `v4l::Device` fd is private; exposing it would require upstream
API changes. Opening separately is clean, slightly slower (~1 open/close per capture),
and acceptable given the infrequency of auth operations.

### 4. Per-verify activate / deactivate (not always-on)

The emitter is activated immediately before `capture_frames()` and deactivated
immediately after. Alternatives:

- **Always-on daemon lifetime:** Risks thermal issues and visible IR flicker during
  normal desktop use. Rejected.
- **Persistent across a session:** Complicates cleanup on crash. Rejected.
- **Per-verify:** Active only during the ~200ms capture window. Adopted.

### 5. 100ms AGC warm-up delay

IR cameras use automatic gain control. Without a brief delay after activation, the
first captured frame may be overexposed while the sensor adjusts. 100ms is the
observed settling time for the UM3406HA. This is hardcoded in Step 5; Step 6 will
expose it as `VISAGE_EMITTER_WARM_UP_MS`.

### 6. Failure is a warning — never fatal

If `activate()` or `deactivate()` fails (ioctl error, permission denied, device busy),
a warning is logged and capture proceeds with ambient light. The emitter error never
propagates to the D-Bus caller or the PAM module. This preserves the Step 4 behavior
as the fallback — authentication works without illumination, just less reliably.

### 7. `visage discover` subcommand

Lists `/dev/video*` devices with their sysfs VID:PID and quirk status. Useful for
hardware support debugging. `--probe` (send a test activation pulse) is deferred to
Step 6 as it requires root access and a safety prompt.

## Consequences

- Enrollment and verification on the UM3406HA now benefit from active IR illumination.
- No new runtime dependencies.
- Adding support for a new camera requires: (1) a new `contrib/hw/{vid}-{pid}.toml`
  file and (2) adding its `include_str!` to `quirks.rs`. Step 6 will introduce a
  runtime override directory to avoid the rebuild requirement.
- The `video` group (or root) is required to send UVC ioctls. A udev rule granting
  `visaged` access will be added in Step 6.

## Known Limitations (resolved in Step 6)

| Limitation | Severity |
|-----------|----------|
| `visage discover --probe` (test activation pulse) | Low |
| AGC warm-up time hardcoded at 100ms | Low |
| Runtime quirk override directory (`/usr/share/visage/quirks/`) | Low |
| Requires read+write on `/dev/videoN` (root or `video` group) — no udev rule yet | Medium |
