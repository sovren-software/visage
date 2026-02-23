# ADR 008 — Hardware Compatibility Detection and Documentation Strategy

**Date:** 2026-02-22
**Status:** Accepted
**Deciders:** Sovren Software

---

## Context

Visage v0.1 was end-to-end tested on one machine (ASUS Zenbook 14 UM3406HA). Before
public announcement, the project needed a clear, actionable answer to the question:
**which IR cameras does Visage actually support on Linux?**

A research pass across common Linux laptop lines revealed a critical split in the Linux
IR camera ecosystem:

- **USB UVC cameras** (`uvcvideo` driver) — appear as standard V4L2 devices, fully
  compatible with Visage's camera pipeline. Present in many ThinkPad, HP EliteBook,
  ASUS ZenBook, Dell Latitude, and TUXEDO configurations.

- **Intel IPU6 cameras** — use Intel's proprietary camera HAL, require libcamera, and
  do not appear as V4L2 devices in the normal sense. Present in newer Dell XPS, many
  ThinkPad Gen 11+, Microsoft Surface, and "AI PC" class machines.

Without distinguishing these at runtime, users with IPU6 cameras would experience a
confusing failure with no guidance: enrollment would appear to start then fail at the
camera open step, with an error message that doesn't explain the underlying cause.

Additionally, `visage discover` gave no indication of which kernel driver handled each
device — the single most actionable signal for diagnosing compatibility.

---

## Decision

### 1. Add sysfs-based driver detection to visage-hw

Expose two public functions in `visage-hw::quirks`:

- `get_driver(device_path) -> Option<String>` — reads the driver symlink basename from
  `/sys/class/video4linux/{dev}/device/driver`. Returns e.g. `"uvcvideo"` or
  `"intel_ipu6_imx_phy"`.
- `is_ipu6_camera(device_path) -> bool` — returns true if driver name contains `"ipu6"`
  or `"intel_ipu"`.

**Implementation:** sysfs symlink read via `std::fs::read_link`. No ioctl, no root
required, no external dependencies.

### 2. Update `visage discover` to surface driver information and warn on IPU6

Every `/dev/video*` device now shows its kernel driver in the output. IPU6 devices are
labelled `[NOT SUPPORTED — IPU6 camera, not UVC]` inline. After scanning, if any IPU6
device was found, a structured warning block explains what IPU6 is, that a separate UVC
IR node may still be present, and points to `docs/hardware-compatibility.md`.

### 3. Create `docs/hardware-compatibility.md`

A standalone reference documenting:
- The UVC/IPU6/no-IR tier system with concrete laptop examples
- How to diagnose your camera stack (`visage discover` output examples)
- The IR emitter quirks system and contribution process
- IPU6 support timeline (v0.3 roadmap)

### 4. Rewrite README hardware section

Replace the single-sentence "Tested on ASUS Zenbook" with a tier table, ThinkPad/
EliteBook compatibility notes, explicit "no linux-enable-ir-emitter dependency" callout,
and a pointer to `docs/hardware-compatibility.md`.

### 5. Do not add unverified ThinkPad/EliteBook quirk TOML entries

Hardware quirks require physically verified `control_bytes`. Wrong bytes cause silent
emitter failure. Adding placeholder entries with fabricated byte values is worse than
having no entry — a user with a matching VID:PID would think their device is supported
when emitter activation silently fails.

The correct path: community PRs from device owners using `visage discover` +
`linux-enable-ir-emitter configure` to find the correct bytes.

---

## Alternatives considered

### Alternative A: Curated VID:PID allowlist for IPU6 detection

Instead of driver name string matching, maintain a list of known IPU6 camera VID:PID
pairs and flag them at discovery time.

**Rejected:** Intel releases new IPU6 camera variants regularly. A VID:PID list requires
constant maintenance and will always lag new hardware. The driver name approach is
forward-compatible — new IPU6 variants automatically produce a `driver=intel_ipu6*`
name without any Visage update.

### Alternative B: Attempt to open device and fail gracefully

Try `Camera::open()` on each discovered device and report the error for non-UVC devices.

**Rejected:** Opening an IPU6 camera device may have side effects (partial initialization,
resource locks). Detecting at the sysfs level is non-destructive and provides the answer
before any kernel device interaction. It also gives a more informative error message than
whatever V4L2 would return on an IPU6 device.

### Alternative C: Add ThinkPad/EliteBook quirk entries with placeholder control_bytes

Add TOML entries with known VID:PIDs and `control_bytes = []` as a placeholder.

**Rejected:** The quirks system is used to activate IR emitters. An entry with empty or
wrong control_bytes will be matched by VID:PID, suppress the "no quirk" message in
`visage discover`, and silently fail to activate the emitter. Users would believe their
device is fully supported when IR illumination is broken. A missing entry is less harmful
than a wrong entry.

---

## Consequences

### Positive

- Users with UVC IR cameras get confirmation (`driver=uvcvideo`) that their camera is
  compatible before investing time in enrollment.
- Users with IPU6 cameras get a clear, early explanation with a pointer to documentation,
  instead of a confusing V4L2 error that doesn't mention the underlying cause.
- The project's hardware target is now accurately communicated in README and docs.
- `linux-enable-ir-emitter` dependency is explicitly called out as unnecessary —
  relevant to users migrating from Howdy.
- The tier table (Supported / Not supported / No IR) sets accurate expectations for
  potential contributors and distribution packagers.

### Negative / Trade-offs

- IPU6 detection uses driver name string matching — coarse but forward-compatible.
  The risk of false positives (a UVC camera with "ipu6" in its driver name) is
  negligible; UVC cameras always use the `uvcvideo` driver.
- `docs/hardware-compatibility.md` references specific laptop models (ThinkPad Gen 11+,
  Dell XPS, etc.) based on research inference, not first-hand testing. Model-specific
  claims require community validation and may need updating as hardware evolves.
- The ThinkPad Gen 11+ note ("may use IPU6") creates ambiguity — some Gen 11
  configs still have a separate USB UVC IR camera. Users need to run `visage discover`
  rather than relying on the docs alone.

### Known limitations that remain open

| Limitation | Impact | Mitigation |
|------------|--------|------------|
| IPU6 not supported | ~30% of 2023+ Intel laptop IR cameras unsupported | IPU6 on v0.3 roadmap; `visage discover` warns clearly |
| Community quirk coverage thin (1 device) | Many UVC IR cameras have no emitter quirk | Contribution process documented; cameras work without emitter if ambient IR is present |
| `docs/hardware-compatibility.md` models are research-inferred | Some model claims may be inaccurate | Marked as needing community validation; issue tracker for corrections |

---

## Implementation

**Commit:** `7d0f9e1` — feat(discover): IPU6 detection, driver visibility, hardware compat docs
**Files changed:**
- `crates/visage-hw/src/quirks.rs` — `get_driver()`, `is_ipu6_camera()`
- `crates/visage-hw/src/lib.rs` — re-export both functions
- `crates/visage-cli/src/main.rs` — `cmd_discover()` updated
- `README.md` — Hardware Support section rewritten
- `docs/hardware-compatibility.md` — new file
- `docs/decisions/008-hardware-compatibility-detection.md` — this ADR
