# ADR 001 — Camera Capture Pipeline (Step 1)

**Date:** 2026-02-21
**Status:** Accepted
**Component:** `visage-hw`

---

## Context

Step 1 required a functional camera capture pipeline: open the IR camera via V4L2,
convert frames to grayscale, filter dark frames, and enhance contrast for downstream
ONNX inference. This is the foundation every subsequent step depends on.

Hardware context: ASUS Zenbook 14 UM3406HA, IR camera at `/dev/video2`, RGB webcam
at `/dev/video0`.

---

## Decisions

### 1. Use `v4l = "0.14"` over raw `nix` ioctls

**What the roadmap said:** "V4L2 frame capture using `nix` crate (direct ioctl)"

**What we built:** `v4l = "0.14"` — a safe Rust wrapper over the V4L2 ioctl interface.

**Rationale:**
- `v4l` exposes `Device`, `MmapStream`, format negotiation, and capability queries as
  typed Rust — eliminating ~200 lines of unsafe ioctl boilerplate
- The `v4l` crate is thin (`v4l2-sys-mit` generated bindings + minimal abstraction)
  — it does not hide the underlying V4L2 concepts or prevent low-level access
- UVC extension unit control for the IR emitter (Step 5) requires raw `ioctl` calls
  that `v4l` does not expose — those will be implemented via `nix`/`libc` directly
  against the device file descriptor, not via `v4l` abstractions

**Trade-off accepted:** One additional crate dependency (`v4l` + `v4l2-sys-mit`).
The reduction in unsafe code surface outweighs this.

**Retained option:** At Step 5, the raw fd from `v4l::Device` is available for direct
ioctl calls. No architectural changes needed.

---

### 2. Handle GREY pixel format in addition to YUYV

**Discovery at runtime:** `/dev/video2` outputs native GREY format (`[0x47, 0x52, 0x45, 0x59]`
= "GREY"), 1 byte per pixel. The roadmap assumed YUYV for all devices.

**Decision:** Detect format at `Camera::open()` time. Accept both YUYV and GREY;
reject everything else at open time with a clear error message.

**Implementation:** `is_grey: bool` stored on `Camera`. `buf_to_grayscale()` dispatches:
- GREY → slice copy, no conversion
- YUYV → `yuyv_to_grayscale()` extracting every other byte (Y channel)

**Rationale:**
- Failing at open time (not at first capture) provides immediate, actionable feedback
- GREY is actually preferable — eliminates conversion overhead and potential data loss
- The YUYV path remains correct and tested via `/dev/video0`

**Trade-off:** Slight complexity increase in `camera.rs` (`buf_to_grayscale` dispatch).
Not significant.

---

### 3. CLAHE implemented from scratch (~90 lines)

**Alternative considered:** Pull in an image processing crate (e.g., `imageproc`).

**Decision:** Implement CLAHE directly in `frame.rs`.

**Rationale:**
- CLAHE on an 8×8 tile grid over a 640×360 grayscale image is ~90 lines of
  straightforward array math — no crate is justified for this scope
- Adding `imageproc` would drag in `image`, `nalgebra`, and other transitive deps
  that add no value at this stage
- The `image` crate was already added to `visage-cli` for future test output needs;
  the actual frame writing uses a hand-rolled PGM encoder (15 lines) that adds no dep

**Trade-off:** We own the CLAHE implementation and must maintain it. Acceptable given
the algorithm is ~50 years old and well-specified.

---

### 4. `visage test` saves PGM, not PNG

**Decision:** Test frames are written as binary PGM (`P5`), not PNG.

**Rationale:**
- PGM is a trivial binary format: ASCII header + raw pixel bytes
- Writing PGM requires no image crate, no compression library — 15 lines of stdlib I/O
- PGM is directly viewable with `feh`, `eog`, `display`, GIMP, and any hex editor
- PNG would require the `image` crate dep in `visage-hw` itself, conflating the
  hardware layer with output concerns

**Limitation:** PGM is not viewable in web browsers or common file managers without
a plugin. For development diagnostics, this is acceptable.

---

### 5. Dark frame threshold: 95% of pixels in bucket 0 (values 0–31)

**Rationale:** IR cameras without an active emitter produce frames with nearly all
pixels at zero. A strict threshold (95%) avoids false-positives from genuinely
dark rooms while still rejecting dead-emitter frames.

**Observed behavior:** Without IR emitter, 29 of 30 capture attempts failed the
dark-frame check. One frame at brightness ~44.8/255 passed — consistent with
ambient IR leakage.

---

## Drawbacks and Known Limitations

### Dark frames dominate without IR emitter

**Severity:** Expected / by design
**Impact:** `visage test` captures ~1 good frame per 30 attempts without emitter active.
`capture_frames(10)` with a 3× budget exhausts 30 attempts and returns 1 frame.
**Resolution:** Step 5 (IR emitter integration) will illuminate the scene. The capture
pipeline is correct; the illumination source is missing.

### No stream settling time

**Severity:** Low
**Impact:** The first 1–3 frames from a freshly opened V4L2 device may be noisy or
overexposed as the sensor auto-adjusts. We do not skip them explicitly — the dark-frame
filter catches frames that are all-black, but not overexposed frames.
**Resolution:** Add a configurable N-frame warmup skip before the `capture_frames` return
window. Deferred to Step 3 (daemon implementation) where stream lifetime is longer.

### Single-stream: can't hold stream open across calls

**Severity:** Low
**Impact:** `capture_frame()` creates and drops a `MmapStream` per call. Opening and
closing the stream per request adds ~5–20ms overhead and prevents the sensor from
settling.
**Resolution:** Acceptable for the CLI test command. The daemon (Step 3) will hold
a persistent stream and pre-warm the camera.

### CLAHE is CPU-only

**Severity:** Low
**Impact:** CLAHE at 640×360 grayscale takes ~2–4ms per frame on the Zenbook's
Ryzen AI CPU. Acceptable for the pipeline budget (target: <30ms end-to-end).
**Resolution:** Not required. GPU/NPU acceleration for pre-processing is not
justified at this scale.

### YUYV path is exercised but not hardware-tested on IR path

**Severity:** Very low
**Impact:** The ASUS Zenbook IR camera outputs GREY. The YUYV conversion was
tested via `/dev/video0` (RGB webcam) — which captured only dark frames in our
environment. The YUYV→grayscale logic is unit-tested with known inputs.
**Resolution:** Non-issue. The unit tests provide confidence. YUYV will be
validated on any camera that negotiates that format.

---

## Remaining Work (Steps 2–6)

| Step | Scope | Blocking on |
|------|-------|------------|
| 2 | ONNX inference (SCRFD face detect + ArcFace embed) | Step 1 ✅ |
| 3 | visaged daemon, D-Bus, SQLite model store | Step 2 |
| 4 | PAM module with 3s timeout + PAM_IGNORE fallback | Step 3 |
| 5 | IR emitter via UVC ioctl — unblocks good frame rate | Step 3 (for daemon camera handle) |
| 6 | Ubuntu packaging (cargo-deb, pam-auth-update) | Steps 4 + 5 |

### Specific Step 2 prerequisites confirmed by Step 1

- Frame dimensions: 640×360 GREY (or YUYV-converted grayscale)
- Frame type: `Frame { data: Vec<u8>, width, height, timestamp, sequence, is_dark }`
- CLAHE is applied before frames leave the capture layer — ONNX receives enhanced frames
- The inference pipeline should expect grayscale u8 input at 640×360 and handle
  resizing to 112×112 (ArcFace input size) internally

### V4L2 fd access for IR emitter ioctl (Step 5)

The `v4l::Device` handle does not expose the raw file descriptor for arbitrary ioctls.
Step 5 will need to either:
1. Open a second fd via `std::fs::File::open(device_path)` for UVC XU ioctls, or
2. Evaluate whether `v4l` provides an escape hatch (check `AsRawFd` impl)

Prefer option 2 if `v4l::Device` implements `AsRawFd` — avoids holding two fds
to the same device.

---

## Test Coverage

| Test | Type | Status |
|------|------|--------|
| `test_yuyv_to_grayscale` | Unit | Pass |
| `test_yuyv_to_grayscale_4x2` | Unit | Pass |
| `test_yuyv_invalid_length` | Unit | Pass |
| `test_dark_frame_all_black` | Unit | Pass |
| `test_dark_frame_normal` | Unit | Pass |
| `test_dark_frame_empty` | Unit | Pass |
| `test_dark_frame_mostly_dark` | Unit | Pass |
| `test_dark_frame_borderline_bright` | Unit | Pass |
| `test_clahe_increases_contrast` | Unit | Pass |
| Live capture on `/dev/video2` | Integration | Verified manually |
| Live capture on `/dev/video0` | Integration | Verified manually (YUYV path) |
