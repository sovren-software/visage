# Hardware Compatibility

Visage works with **USB UVC IR cameras** — IR cameras that appear as standard V4L2
devices under the `uvcvideo` kernel driver. The key question for any laptop is:
**which kernel driver handles its IR camera?**

Run `visage discover` to get the answer for your machine.

---

## Quick reference

| Camera stack | `visage discover` output | Visage support |
|-------------|--------------------------|----------------|
| USB UVC | `driver=uvcvideo` | ✅ Supported |
| Intel IPU6 | `driver=intel_ipu6*` | ❌ Not supported yet (v0.4+) |
| MIPI / libcamera | varies | ❌ Not supported yet (v0.4+) |

---

## Laptop compatibility tiers

### Tier 1 — Well-supported (UVC IR, active community, Howdy history)

| Brand / Line | IR camera | Linux driver | Notes |
|---|---|---|---|
| **Lenovo ThinkPad** T/X/L/P series (pre-Gen 11) | Optional | `uvcvideo` | Separate USB IR node alongside RGB webcam. T14, T14s, X1 Carbon Gen 6–10 frequently reported working with Howdy. Verify node with `visage discover`. |
| **HP EliteBook** 8xx G8+ | Optional | `uvcvideo` | IR + presence detection on many SKUs. UVC-based on tested G8/G9 configs. Newer "AI PC" models may shift to IPU6. |
| **ASUS ZenBook** (UX series, ZenBook 14) | Yes (most SKUs) | `uvcvideo` | Reference hardware for Visage — ZenBook 14 UM3406HA tested end-to-end. |
| **TUXEDO** InfinityBook, Pulse | Some SKUs | `uvcvideo` | Linux-first OEM; users have reported Howdy working on IR-equipped configs. |

### Tier 2 — Likely supported but more variable

| Brand / Line | IR camera | Linux driver | Notes |
|---|---|---|---|
| **Dell Latitude** 5x30, 5x40, 7x20 | Optional | `uvcvideo` (older) / `intel_ipu6` (newer) | Ubuntu-certified fleet laptops. UVC IR works on pre-2023 gens. 2023+ may use IPU6 — verify with `visage discover`. |
| **Lenovo ThinkPad** Gen 11+ | Optional | Often `intel_ipu6` | Many Gen 11 models switched the integrated camera stack to IPU6. A separate USB IR sensor may still appear under `uvcvideo` — check all `/dev/video*` nodes. |

### Tier 3 — No IR camera (Visage not applicable)

| Brand / Line | Notes |
|---|---|
| **Framework** 13 / 16 | Fingerprint only. No IR camera. |
| **System76** (all models) | Standard RGB webcam only. |
| **Purism Librem** | Privacy-focused; standard webcam with kill switch. |

### Tier 4 — IR camera present but not supported

| Brand / Line | IR camera | Linux driver | Notes |
|---|---|---|---|
| **Dell XPS** 15/16 (2023+) | Yes | `intel_ipu6` | IPU6 camera stack. Even the RGB webcam may not work on Linux without distro-specific libcamera support. |
| **Microsoft Surface** (all lines) | Yes | Custom HAL | Requires linux-surface kernel patches + libcamera. IR via PAM not practical yet. |

---

## How to identify your camera stack

```bash
# List all /dev/video* devices with driver and quirk status
visage discover
```

A UVC IR camera looks like:
```
/dev/video2  driver=uvcvideo  VID=0x04f2 PID=0xb6d9  quirk: ASUS Zenbook 14 UM3406HA IR Camera ✓
```

An IPU6 camera looks like:
```
/dev/video0  driver=intel_ipu6_imx_phy  [NOT SUPPORTED — IPU6 camera, not UVC]
```

If your IR camera appears as `driver=uvcvideo` but has `no quirk`, it may still work
for enrollment and verification — the quirk is only needed for IR emitter activation.
You can test without emitter support and contribute emitter bytes later via `contrib/hw/`.

---

## IR emitter support

Some UVC IR cameras require a specific control byte sequence sent to the camera's UVC
extension unit to power on the IR emitter. Without it, frames will be dark (the IR
camera captures IR light, but none is being emitted).

Visage includes built-in emitter control with no external dependencies. There is no
need for `linux-enable-ir-emitter`. The quirk database at `contrib/hw/` maps USB
VID:PID to the correct control bytes for each known device.

**Current quirk entries:**

| Device | VID:PID | Status |
|--------|---------|--------|
| ASUS Zenbook 14 UM3406HA | `04f2:b6d9` | ✅ Verified on hardware |

**Contributing a quirk for your camera:**

1. Run `visage discover` to find your camera's VID:PID
2. Use `linux-enable-ir-emitter configure` or UVC descriptor analysis to find the
   control bytes (see [contrib/hw/README.md](../contrib/hw/README.md))
3. Create `contrib/hw/{vid}-{pid}.toml` and submit a PR

---

## IPU6 support timeline

IPU6 cameras are planned for future versions. Supporting them requires libcamera
integration rather than direct V4L2 capture, which is a substantial architectural
addition. The primary blockers are:

- Stable libcamera Rust bindings
- Per-distro libcamera packaging consistency (Ubuntu LTS vs Fedora vs Arch)
- Testing infrastructure for IPU6 hardware

If you have an IPU6 laptop and want to contribute, open an issue on GitHub to
discuss the approach.
