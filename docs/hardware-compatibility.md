# Hardware Compatibility

Visage works with **USB UVC IR cameras** — IR cameras that appear as standard V4L2
devices under the `uvcvideo` kernel driver. The key question for any laptop is:
**which kernel driver handles its IR camera?**

Run `visage discover` to get the answer for your machine.

---

## Quick reference

| Camera stack | `visage discover` output | Visage support |
|-------------|--------------------------|----------------|
| USB UVC IR | `driver=uvcvideo` plus an IR node/format or known emitter quirk | ✅ Supported |
| USB UVC RGB-only | `driver=uvcvideo`, but only a normal webcam stream | ❌ Not secure-compatible |
| Intel IPU6 | `driver=intel_ipu6*` | ❌ Not supported (v0.1) |
| MIPI / libcamera | varies | ❌ Not supported (v0.1) |

---

## Laptop compatibility tiers

### Tier 1 — Well-supported (UVC IR, active community, Howdy history)

| Brand / Line | IR camera | Linux driver | Notes |
|---|---|---|---|
| **Lenovo ThinkPad** T/X/L/P series (pre-Gen 11) | Optional | `uvcvideo` | Separate USB IR node alongside RGB webcam. T14, T14s, X1 Carbon Gen 6–10 frequently reported working with Howdy. Verify node with `visage discover`. |
| **HP EliteBook** 8xx G8+ | Optional | `uvcvideo` | IR + presence detection on many SKUs. UVC-based on tested G8/G9 configs. Newer "AI PC" models may shift to IPU6. |
| **ASUS ZenBook** (UX series, ZenBook 14) | Yes (most SKUs) | `uvcvideo` | Reference hardware for Visage — ZenBook 14 UM3406HA tested end-to-end. ⚠️ **The same model ships more than one camera module.** The `04f2:b6d9` (Chicony) quirk does not apply to `3277:0055` (Shinetech) units. Model name does not predict emitter support — run `visage discover`. See [report](hardware-reports/asus-zenbook-um3406ha-3277-0055.md). |
| **TUXEDO** InfinityBook, Pulse | Some SKUs | `uvcvideo` | Linux-first OEM; users have reported Howdy working on IR-equipped configs. |

### Tier 2 — Likely supported but more variable

| Brand / Line | IR camera | Linux driver | Notes |
|---|---|---|---|
| **Dell Latitude** 5x30, 5x40, 7x20 | Optional | `uvcvideo` (older) / `intel_ipu6` (newer) | Ubuntu-certified fleet laptops. UVC IR works on pre-2023 gens. 2023+ may use IPU6 — verify with `visage discover`. |
| **Lenovo ThinkPad** Gen 11+ | Optional | Often `intel_ipu6` | Many Gen 11 models switched the integrated camera stack to IPU6. A separate USB IR sensor may still appear under `uvcvideo` — check all `/dev/video*` nodes. |

### Tier 3 — No IR camera (Visage not applicable)

| Brand / Line | Notes |
|---|---|
| **ASUS ExpertBook B3302FEA/B5302FEA** | Built-in Azurewave/IMC `13d3:56ea` camera is RGB-only UVC on tested hardware: `/dev/video0` captures MJPG/YUYV and `/dev/video1` is metadata only. No IR node, IR pixel format, Microsoft face-auth XU, or known emitter quirk was found. See [hardware report](hardware-reports/asus-expertbook-b3302fea-13d3-56ea.md). |
| **Framework** 13 / 16 | Fingerprint only. No IR camera. |
| **System76** (all models) | Standard RGB webcam only. |
| **Purism Librem** | Privacy-focused; standard webcam with kill switch. |

### Tier 4 — IR camera present but not supported

| Brand / Line | IR camera | Linux driver | Notes |
|---|---|---|---|
| **Dell XPS** 15/16 (2023+) | Yes | `intel_ipu6` | IPU6 camera stack. Even the RGB webcam may not work on Linux without distro-specific libcamera support. |
| **Microsoft Surface** (all lines) | Yes | Custom HAL | Requires linux-surface kernel patches + libcamera. IR via PAM not practical in v0.1. |

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

A UVC RGB-only camera is not enough for secure Visage auth, even though the
kernel driver is supported:
```
/dev/video0  driver=uvcvideo  VID=0x13d3 PID=0x56ea  no quirk (VID=0x13d3 PID=0x56ea)
```

An IPU6 camera looks like:
```
/dev/video0  driver=intel_ipu6_imx_phy  [NOT SUPPORTED — IPU6 camera, not UVC]
```

If your IR camera appears as `driver=uvcvideo` but has `no quirk`, it may still work
for enrollment and verification — the quirk is only needed for IR emitter activation.
However, `uvcvideo` alone is not sufficient: a laptop that exposes only a normal
RGB webcam stream is not compatible with Visage's intended secure authentication
path. Check for a separate IR node, an IR-oriented pixel format (`GREY`/`Y16`),
or a documented emitter quirk before enabling PAM auth.
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
| Lenovo ThinkPad P14s Gen 2a 21A0000RMX | `04f2:b6d0` | ✅ Verified on hardware (community, [#76](https://github.com/sovren-software/visage/pull/76)) |
| Lenovo ThinkPad X1 Carbon Gen 9 20XW00FPUS | `174f:2454` | ✅ Verified on hardware |
| Lenovo ThinkBook 14 MP2PQAZG | `30c9:00c2` | ✅ Verified on hardware |
| HP OmniBook X Flip | `30c9:0120` | ✅ Verified on hardware |
| Lenovo ThinkPad P14s Gen 4 21HF | `174f:11a8` | ✅ Verified on hardware |

**Known devices with NO quirk entry:**

| Device | VID:PID | Notes |
|--------|---------|-------|
| ASUS Zenbook 14 UM3406HA (Shinetech module) | `3277:0055` | No quirk — but the **emitter strobes anyway** by firmware default, so face auth works without one. Exposes the Microsoft Camera Control XU (`{0f3f95dc-2632-4c4e-92c9-a04782f43bc8}`) at unit 14 advertising `MSXU_CONTROL_FACE_AUTHENTICATION` (selector 6) and `MSXU_CONTROL_METADATA` (selector 9) — **not** `IR_TORCH`. See [report](hardware-reports/asus-zenbook-um3406ha-3277-0055.md). |

⚠️ **A missing quirk does not imply missing illumination.** On modules whose firmware
enables face-auth mode by default, the emitter fires regardless and `visaged` still logs
`no IR emitter quirk for device; proceeding without illumination`. Confirm with the
per-frame brightness sequence (`visage test -n 20`) — a strobing emitter alternates
lit/unlit every frame. A dark-frame *count* cannot distinguish that from an exposure ramp.

**Contributing a quirk for your camera:**

1. Run `visage discover` to find your camera's VID:PID
2. Use `linux-enable-ir-emitter configure` or UVC descriptor analysis to find the
   control bytes (see [contrib/hw/README.md](../contrib/hw/README.md))
3. Create `contrib/hw/{vid}-{pid}.toml`
4. **Register it in `crates/visage-hw/src/quirks.rs`.** Quirk files are embedded at
   compile time with `include_str!` — there is no runtime file loading, so **dropping a
   `.toml` into `contrib/hw/` does nothing on its own.** Two lines:

   ```rust
   const QUIRK_04F2_B6D0: &str = include_str!("../../../contrib/hw/04f2-b6d0.toml");
   //  …then add it to QUIRK_SOURCES:
   ("04f2-b6d0.toml", QUIRK_04F2_B6D0),
   ```

5. **Verify the quirk is doing the work, not another tool.** Disable any external emitter
   activation tool (`linux-enable-ir-emitter` and similar) and **power-cycle the laptop or the
   camera** to clear residual control bytes, then test. A camera left illuminated by something
   else will make a non-working quirk look correct, and the contribution ships inert. This is a
   real negative control — it is what made [#76](https://github.com/sovren-software/visage/pull/76)
   trustworthy, and @rampa3 suggested documenting it in
   [#85](https://github.com/sovren-software/visage/issues/85).
6. Run `cargo test -p visage-hw quirks::` and submit a PR

⚠️ **Step 4 is the one that is easy to miss, and its failure is silent.** `quirk_db()`
skips a malformed or unregistered quirk rather than panicking — the daemon authenticates
logins, so one bad contribution must not stop it starting. The cost is that at runtime a
camera with an unregistered quirk is **indistinguishable from a camera with no quirk**: no
error, no crash, the emitter simply never fires. The tests in step 5 exist precisely to
catch that before it ships.

The full field reference, the registration step and what each test catches are in
[contrib/hw/README.md](../contrib/hw/README.md), which is the authoritative version of this
workflow.

---

## IPU6 support timeline

Intel IPU6 cameras are on the v0.3 roadmap. Supporting them requires libcamera
integration rather than direct V4L2 capture, which is a substantial architectural
addition. The primary blockers are:

- Stable libcamera Rust bindings
- Per-distro libcamera packaging consistency (Ubuntu LTS vs Fedora vs Arch)
- Testing infrastructure for IPU6 hardware

If you have an IPU6 laptop and want to contribute, open an issue on GitHub to
discuss the approach.
