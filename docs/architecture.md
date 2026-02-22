# Visage Architecture

## Implementation Status

| Step | Component | Status |
|------|-----------|--------|
| 1 | Camera pipeline (visage-hw) | ✅ Complete |
| 2 | ONNX inference pipeline (visage-core) | ✅ Complete |
| 3 | Daemon (visaged) + CLI (visage-cli) | ✅ Complete |
| 4 | PAM module (pam-visage) + system bus migration | ✅ Complete |
| 5 | IR emitter control | Stub |
| 6 | Packaging (Ubuntu + NixOS) | Not started |

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

The camera pipeline handles three V4L2 pixel formats via the `PixelFormat` enum:

| Format | Bytes/pixel | Source | Conversion |
|--------|------------|--------|------------|
| `GREY` | 1 | IR cameras (native 8-bit grayscale) | None — used directly |
| `YUYV` | 2 | RGB webcams, some IR cameras | Y-channel extraction (every other byte) |
| `Y16` | 2 | IR cameras (native 16-bit grayscale) | `(high << 8 \| low) >> 8` — top byte kept |

Format is detected at `Camera::open()` and stored on the handle. The device
driver selects the actual format after negotiation — we request YUYV/GREY/Y16
and dispatch based on what is negotiated. Unknown formats are rejected at open time
with a clear error.

**Discovery:** The ASUS Zenbook 14 UM3406HA IR camera (`/dev/video2`) outputs
native GREY at 640×360. This is more efficient than YUYV — no conversion needed.

**Y16 note:** Many IR cameras default to 16-bit depth output (Y16). The upper 8 bits
are kept for face detection input (SCRFD trained on 8-bit images). The lower 8 bits
carry sub-pixel IR intensity detail; these are discarded in v2 but will be relevant
for liveness detection in v3. See ADR 004, §6 for details.

### Frame Processing

Every captured frame goes through:

1. **Format conversion** — YUYV→grayscale, GREY passthrough, or Y16→u8 top byte extraction
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

## ONNX Inference Pipeline (visage-core) — Implemented

### Models

| Model | File | Size | Purpose |
|-------|------|------|---------|
| SCRFD det_10g | `det_10g.onnx` | 16 MB | Face detection, 3-stride, 5-point landmarks |
| ArcFace w600k_r50 | `w600k_r50.onnx` | 166 MB | 512-D face embeddings |

Both models are loaded from `$XDG_DATA_HOME/visage/models/` (defaults to
`~/.local/share/visage/models/`). See `models/README.md` for download instructions.

### SCRFD Detector

**Input:** Arbitrary-size grayscale frame → 640×640 NCHW float32 (letterboxed)

**Preprocessing pipeline:**
1. Bilinear letterbox resize to 640×640, preserving aspect ratio with 127.5-padded borders
2. Grayscale → 3-channel replication (Y → [R=Y, G=Y, B=Y])
3. Normalize: `(pixel - 127.5) / 128.0`
4. Layout: NCHW `[1, 3, 640, 640]`

**Output decoding:**
- 9 tensors: 3 strides (8×, 16×, 32×) × 3 tensors (scores, bboxes, keypoints)
- Tensor mapping is resolved by name at load time (`score_8`, `bbox_8`, `kps_8` pattern)
  with positional fallback (`[(0,3,6), (1,4,7), (2,5,8)]`)
- Each stride decodes anchor grid → (cx, cy, w, h) bounding boxes + 5 landmark pairs
- Confidence threshold: 0.5 (configurable)
- NMS threshold: 0.4 (IoU-based)
- Output coordinates are denormalized back to original frame space

**Named constants:**

```rust
const SCRFD_INPUT_SIZE: usize = 640;
const SCRFD_MEAN: f32 = 127.5;
const SCRFD_STD: f32 = 128.0;      // ← different from ArcFace
const SCRFD_CONFIDENCE_THRESHOLD: f32 = 0.5;
const SCRFD_NMS_THRESHOLD: f32 = 0.4;
const SCRFD_STRIDES: [usize; 3] = [8, 16, 32];
const SCRFD_ANCHORS_PER_CELL: usize = 2;
```

### Face Alignment

Between detection and recognition, detected faces are aligned to a canonical 112×112
position using the five detected facial landmarks.

**Algorithm:** 4-DOF similarity transform (uniform scale + rotation + translation).

1. Solve least-squares over 5 point pairs (10 equations, 4 unknowns) via Gaussian
   elimination with partial pivoting → transform parameters [a, b, tx, ty]
2. Build 2×3 affine matrix: `[[a, -b, tx], [b, a, ty]]`
3. Invert the 2×2 rotation-scale part; apply bilinear interpolation to produce
   a 112×112 aligned crop

**Reference landmarks (ArcFace canonical space):**

```
left_eye:   (38.29, 51.70)
right_eye:  (73.53, 51.50)
nose:       (56.03, 71.74)
left_mouth: (41.55, 92.37)
right_mouth: (70.73, 92.20)
```

### ArcFace Recognizer

**Input:** 112×112 grayscale aligned crop → embedding

**Preprocessing:**
1. Grayscale → 3-channel replication
2. Normalize: `(pixel - 127.5) / 127.5` ← note: different STD from SCRFD
3. Layout: NCHW `[1, 3, 112, 112]`

**Output:**
- Raw `[1, 512]` float32 tensor
- L2-normalized immediately after inference: all stored embeddings are unit vectors
- Tagged with `model_version: "w600k_r50"` for audit trail

**Named constants:**

```rust
const ARCFACE_INPUT_SIZE: usize = 112;
const ARCFACE_MEAN: f32 = 127.5;
const ARCFACE_STD: f32 = 127.5;   // ← different from SCRFD
const ARCFACE_EMBEDDING_DIM: usize = 512;
```

### Embedding Comparison

```rust
// Cosine similarity (primary API)
embedding_a.similarity(&embedding_b) -> f32  // range [-1, 1]

// Gallery matching
CosineMatcher.compare(&probe, &gallery, threshold) -> MatchResult

// Recommended thresholds (w600k_r50 empirical)
// 0.45 → ~0.01% FAR (strict)
// 0.40 → ~0.1% FAR  (balanced)
```

**Security property:** Both `similarity()` and `CosineMatcher::compare()` are constant-time:
all dimensions / all gallery entries are always processed. No early exit that could leak
similarity values or gallery size through timing.

### Public API Surface

```rust
// Detector
FaceDetector::load(model_path: &str) -> Result<FaceDetector, DetectorError>
FaceDetector::detect(&mut self, frame: &[u8], width: u32, height: u32)
    -> Result<Vec<BoundingBox>, DetectorError>

// Recognizer
FaceRecognizer::load(model_path: &str) -> Result<FaceRecognizer, RecognizerError>
FaceRecognizer::extract(&mut self, frame: &[u8], width: u32, height: u32, face: &BoundingBox)
    -> Result<Embedding, RecognizerError>

// Matching
CosineMatcher.compare(&probe: &Embedding, gallery: &[FaceModel], threshold: f32)
    -> MatchResult

// Alignment (low-level, used internally)
alignment::align_face(frame: &[u8], width: u32, height: u32, landmarks: &[(f32,f32); 5])
    -> Vec<u8>  // 112×112 grayscale crop

// Model paths
visage_core::default_model_dir() -> PathBuf  // $XDG_DATA_HOME/visage/models
```

### Known Limitations (v2)

- **CPU-only inference.** No CUDA/Vulkan execution providers. ~60-80ms total auth latency.
- **Anti-spoofing is passive.** IR + emitter provides passive liveness; no active detection.
- **No integration test suite.** Unit tests (36) require no models. End-to-end tests need
  downloaded ONNX files and are not yet gated behind `--features integration`.
- **No load-time sanity check.** Model compatibility is verified on first inference, not at load.

See [ADR 004](decisions/004-inference-pipeline-implementation.md) for full decision log, rationale, and v3 migration paths.

## Daemon (visaged) — Implemented

### Configuration

All settings are overridable via `VISAGE_*` environment variables. Defaults:

| Setting | Default | Env var |
|---------|---------|---------|
| Camera device | `/dev/video2` | `VISAGE_CAMERA_DEVICE` |
| Model directory | `$XDG_DATA_HOME/visage/models/` | `VISAGE_MODEL_DIR` |
| Database path | `$XDG_DATA_HOME/visage/faces.db` | `VISAGE_DB_PATH` |
| Similarity threshold | `0.40` | `VISAGE_SIMILARITY_THRESHOLD` |
| Verify timeout | `10s` | `VISAGE_VERIFY_TIMEOUT_SECS` |
| Warmup frames | `4` | `VISAGE_WARMUP_FRAMES` |
| Frames per verify | `3` | `VISAGE_FRAMES_PER_VERIFY` |
| Frames per enroll | `5` | `VISAGE_FRAMES_PER_ENROLL` |

### Startup Sequence (Fail-Fast)

```
1. Init tracing (RUST_LOG)
2. Load Config from env vars
3. spawn_engine() — opens camera + loads both ONNX models synchronously
   Warmup: discard N frames for camera AGC/AE stabilization
   Fail here → daemon exits; error visible in journal
4. FaceModelStore::open() — creates SQLite DB + runs migrations if needed
5. zbus session bus: register org.freedesktop.Visage1 at /org/freedesktop/Visage1
6. Wait for SIGINT/SIGTERM
```

### Engine Thread

Camera, FaceDetector, and FaceRecognizer are `!Sync` and take `&mut self`. They live on a
dedicated `std::thread` (not a tokio task). D-Bus handlers communicate via `mpsc::channel`
(depth: 4) + `oneshot` reply channels. This avoids `Arc<Mutex<_>>` contention on the hot path.

### D-Bus API (`org.freedesktop.Visage1`)

| Method | Signature | Returns |
|--------|-----------|---------|
| `Enroll` | `(user: s, label: s)` | `s` — model UUID |
| `Verify` | `(user: s)` | `b` — match result |
| `Status` | `()` | `s` — JSON status |
| `ListModels` | `(user: s)` | `s` — JSON array |
| `RemoveModel` | `(user: s, model_id: s)` | `b` — deleted |

**Locking protocol:** Every D-Bus handler follows:
1. Lock `Arc<Mutex<AppState>>` → copy config values + clone `EngineHandle` → unlock
2. Call engine (async I/O over channel; no lock held)
3. Lock → write to store → unlock

This ensures concurrent `Status` / `ListModels` calls can proceed while an `Enroll` or
`Verify` is running.

### Storage (SQLite WAL)

Embeddings stored as raw little-endian `f32` bytes (512 × 4 = 2048 bytes each). Two
v3 data plane columns (`quality_score REAL`, `pose_label TEXT`) are included with
defaults — no migration needed when pose-indexed enrollment is added.

**Cross-user protection:** Every mutation includes `WHERE user = ?`. `RemoveModel` returns
`false` (not an error) if the model belongs to a different user.

### Startup Sequence (Step 4 — System Bus)

```
1. Init tracing (RUST_LOG)
2. Load Config from env vars
3. spawn_engine() — opens camera + loads both ONNX models synchronously
4. FaceModelStore::open() — creates SQLite DB + runs migrations if needed
5. zbus SYSTEM bus (or session bus if VISAGE_SESSION_BUS=1): register
   org.freedesktop.Visage1 at /org/freedesktop/Visage1
6. Log which bus is active, wait for SIGINT/SIGTERM
```

The system bus requires:
- D-Bus policy file installed at `/usr/share/dbus-1/system.d/org.freedesktop.Visage1.conf`
- Daemon started with `sudo` (to own `org.freedesktop.Visage1`)

### Known Limitations (v2)

1. **No D-Bus caller authentication.** The `user` parameter is caller-supplied and not
   validated against the D-Bus sender identity. A compromised caller can call `Verify`
   for any username. Step 6 should bind `user` to the D-Bus peer credentials using
   `GetConnectionCredentials`.

2. **best_quality unused.** `VerifyResult.best_quality` is computed but not exposed over
   D-Bus. Reserved as a v3 hook for quality metadata without a schema change.

3. **Single auth flow at a time.** The engine thread processes requests serially (depth-4
   queue). Concurrent `Verify` calls serialize. Acceptable for v2; v3 would use a pool.

See [ADR 003](decisions/003-daemon-integration.md) and [ADR 005](decisions/005-pam-system-bus-migration.md).

## PAM Module (pam-visage) — Implemented

### Authentication Flow

```
sudo echo test
  │
  ▼ PAM stack loads /path/to/libpam_visage.so
  │
  ├─ pam_get_user(pamh) → "ccross"
  │
  ├─ zbus::blocking::Connection::system()
  │     → org.freedesktop.Visage1.Verify("ccross")
  │
  ├─ true  → PAM_SUCCESS (0)  → sudo proceeds
  └─ false / error / timeout → PAM_IGNORE (25) → fall to password prompt
```

### Design Constraints

| Constraint | Enforcement |
|-----------|-------------|
| No async runtime | `zbus::blocking` only — no tokio |
| No panic across FFI | `std::panic::catch_unwind` wraps all Rust logic |
| Never lock out user | Every error path returns `PAM_IGNORE`, never `PAM_AUTH_ERR` |
| Correct ABI | 4-argument `extern "C"` — `pamh, flags, argc, argv` |
| Forward-compatible | `#![warn(unsafe_op_in_unsafe_fn)]` — explicit `unsafe {}` blocks |

### PAM Configuration

Add **before** `@include common-auth` in `/etc/pam.d/sudo`:

```
auth  sufficient  /path/to/target/debug/libpam_visage.so
```

For production, install to `/usr/lib/security/pam_visage.so`.

### Fallback Recovery

If the PAM entry breaks `sudo`, recover with:

```bash
# pkexec doesn't go through sudo's PAM stack
pkexec vim /etc/pam.d/sudo
# Or from a root shell:
su -c "vim /etc/pam.d/sudo"
```

### Known Limitations (Step 4)

1. **D-Bus timeout: 10–25 s.** Under normal conditions the daemon's 10 s verify timeout
   fires first. If the daemon deadlocks, the D-Bus default (~25 s) applies. A 3 s
   client-side timeout is deferred to Step 6.

2. **No caller authentication.** The `user` string passed to `Verify()` comes from
   `pam_get_user` and is not validated against D-Bus peer credentials. Step 6 should
   use `GetConnectionCredentials` to bind the call to the authenticated PAM user.

3. **Development-only PAM config.** Manual `/etc/pam.d/sudo` edit. `pam-auth-update`
   integration is Step 6 (packaging).

4. **IR emitter not active.** Testing requires ambient light. Step 5 resolves this.

5. **`eprintln!` logging.** Messages appear in the `sudo` terminal. Production syslog
   (`LOG_AUTHPRIV`) is deferred to Step 6.

See [ADR 005](decisions/005-pam-system-bus-migration.md) for full decision log.

## Security Model

See [threat-model.md](threat-model.md).
