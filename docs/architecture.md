# Visage Architecture

## Design Principles

1. **Daemon owns hardware** — PAM module never touches the camera
2. **D-Bus for IPC** — Standard Linux desktop integration pattern (fprintd model)
3. **IR emitter absorbed** — No external dependency for emitter control
4. **Pluggable models** — ONNX Runtime for inference, swap models without recompilation
5. **Distribution-agnostic** — Ubuntu first, NixOS second, then Arch/Fedora

## Component Overview

```
┌───────────┐    D-Bus     ┌──────────┐    V4L2    ┌──────────┐
│ pam_visage│───────────▶ │ visaged  │──────────▶│ IR Camera│
│ (cdylib)  │             │ (daemon) │           └──────────┘
└───────────┘             └────┬─────┘
                               │
┌───────────┐    D-Bus    ┌────▼─────┐    ONNX    ┌──────────┐
│ visage    │───────────▶│ visage-  │──────────▶│ SCRFD    │
│ (CLI)     │            │ core     │           │ ArcFace  │
└───────────┘            └──────────┘           └──────────┘
```

## Authentication Flow

1. PAM stack triggers `pam_visage.so`
2. PAM module connects to `org.freedesktop.Visage1` D-Bus service
3. Calls `Verify(username)` with a timeout
4. Daemon activates IR emitter (if needed)
5. Captures N frames, skipping dark frames
6. SCRFD detects face bounding boxes
7. ArcFace extracts embedding from best detection
8. Compares embedding against enrolled models (cosine similarity)
9. Returns match/no-match to PAM module
10. PAM module returns PAM_SUCCESS or PAM_IGNORE (safe fallback)

## Camera Pipeline (visage-hw) — Implemented

### Pixel Format Handling

The camera pipeline handles two V4L2 pixel formats:

| Format | Bytes/pixel | Source | Conversion |
|--------|------------|--------|------------|
| `GREY` | 1 | IR cameras (native grayscale) | None — used directly |
| `YUYV` | 2 | RGB webcams, some IR cameras | Y-channel extraction (every other byte) |

Format is detected at `Camera::open()` and stored on the handle. The device
driver selects the actual format after negotiation — we request YUYV but accept
GREY. Any other format is rejected at open time.

**Discovery:** The ASUS Zenbook 14 UM3406HA IR camera (`/dev/video2`) outputs
native GREY at 640×360. This is more efficient than YUYV — no conversion needed.

### Frame Processing

Every captured frame goes through:

1. **Format conversion** — YUYV→grayscale or GREY passthrough
2. **Dark frame detection** — 8-bucket histogram; >95% of pixels in bucket 0
   (values 0–31) → frame marked dark and skipped
3. **CLAHE contrast enhancement** — Applied to non-dark frames before return

### CLAHE Parameters

| Parameter | Value | Rationale |
|-----------|-------|-----------|
| Tile grid | 8×8 | Balances local/global contrast adaptation |
| Clip limit | 0.02 (2% of tile pixels) | Suppresses noise amplification |
| Interpolation | Bilinear between tile CDFs | Prevents tile boundary artifacts |

CLAHE is implemented from scratch in ~90 lines (`frame::clahe_enhance`). No
additional image processing crate dependency.

### Dark Frame Behavior (Current)

Without the IR emitter active, most frames from `/dev/video2` are dark.
In testing, 29 of 30 capture attempts were rejected. One good frame with
brightness ~44.8/255 passed through.

This is expected and correct. The IR emitter (Step 5) will illuminate the scene,
yielding high frame pass rates during authentication attempts.

### Public API Surface

```rust
// Open device; negotiates format, validates capabilities
Camera::open(device_path: &str) -> Result<Camera, CameraError>

// Capture a single converted frame
Camera::capture_frame(&self) -> Result<Frame, CameraError>

// Capture count good frames (budget: count*3 raw attempts); applies CLAHE
// Returns (good_frames, dark_frames_skipped)
Camera::capture_frames(&self, count: usize) -> Result<(Vec<Frame>, usize), CameraError>

// Enumerate V4L2 capture devices
Camera::list_devices() -> Vec<DeviceInfo>
```

The `Frame` struct carries: `data` (grayscale pixels), `width`, `height`,
`timestamp`, `sequence` (V4L2 buffer sequence number), `is_dark`.

## Security Model

See [threat-model.md](threat-model.md).
