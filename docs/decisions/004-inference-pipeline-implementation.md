# ADR 004 — ONNX Inference Pipeline: Implementation Decisions

**Date:** 2026-02-21
**Status:** Accepted
**Component:** `visage-core` (alignment, detector, recognizer, types), `visage-hw` (camera)
**Implements:** Step 2 of the Visage implementation roadmap

---

## Context

Step 2 transforms `visage-core` from 100% scaffolded stubs into a working ONNX-based
face detection and recognition engine. The implementation scope:

- **Phase A:** Core type system (landmarks, MatchResult, Matcher trait, Embedding improvements)
- **Phase B:** Face alignment module (similarity transform + bilinear warp)
- **Phase C:** SCRFD face detector (full 3-stride anchor-free decode + NMS)
- **Phase D:** ArcFace face recognizer (alignment + embedding extraction + L2-norm)
- **Phase E:** Module wiring (lib.rs, re-exports, XDG model paths)
- **Phase F:** Y16 camera format support (visage-hw pre-step)
- **Phase G:** Model infrastructure (README, download instructions)

Post-implementation improvements added three risk mitigations:
- Discovery-based ONNX tensor naming
- Bilinear letterbox preprocessing
- Backward-compatible embedding comparison API

The ADR documents each decision, its rationale, trade-offs, and known limitations.

---

## Decisions

### 1. Discovery-Based SCRFD Output Tensor Ordering

**Context:** ONNX models exported from InsightFace Python may name outputs differently
depending on the export script version. Two observed schemes:

| Scheme | Tensor names |
|--------|-------------|
| Named | `score_8`, `bbox_8`, `kps_8`, `score_16`, `bbox_16`, ... |
| Positional | `0`, `1`, `2`, `3`, `4`, ... |

Hardcoding a positional layout `[(0,3,6), (1,4,7), (2,5,8)]` is the conventional fallback
but silently produces garbage results if the model was exported with a different scheme.

**Decision:** Implement `discover_output_indices()` that first attempts name-based discovery
(`score_8`/`bbox_8`/`kps_8` pattern) and falls back to the conventional positional layout
only if no named tensors match. The resolved mapping is stored in `FaceDetector::stride_indices`
and logged at load time, making any mismatch immediately visible in the application log.

**Rationale:**
- A wrong tensor mapping produces no runtime error — the model silently accepts the inputs
  and returns plausible-looking but meaningless detections.
- Silent failure in face authentication is a security risk: the system could report no face
  detected (DoS to the user) rather than a model configuration error.
- The fallback preserves compatibility with the majority of SCRFD exports; the named path
  handles the minority without configuration.

**Trade-offs:**
- Adds ~50 lines of discovery logic that runs once at load time.
- The positional fallback assumes `[(0,3,6), (1,4,7), (2,5,8)]` — this covers the standard
  InsightFace Python export. Models exported with custom scripts may use neither scheme;
  they will produce wrong detections with no explicit error. A future hardening step would
  add a sanity-check decode on a known synthetic input at load time.

**Alternatives not taken:**
- Make tensor indices configurable via constructor argument: adds API complexity for a problem
  most users will never encounter.
- Hard fail if names don't match: too strict — breaks positional exports that are equally valid.

---

### 2. Bilinear Interpolation for Letterbox Resize

**Context:** SCRFD preprocessing requires resizing arbitrary-size frames to 640×640 while
preserving aspect ratio (letterbox padding). The initial implementation used nearest-neighbor
sampling.

**Decision:** Replace nearest-neighbor with half-pixel-aligned bilinear interpolation,
using the formula:

```
src_x = (dst_x + 0.5) * inv_scale - 0.5
```

Bilinear sampling reads the four nearest source pixels and interpolates using fractional
weights. The half-pixel alignment matches OpenCV's `INTER_LINEAR` behavior.

**Rationale:**
- Nearest-neighbor introduces aliasing artifacts that affect edge sharpness at facial
  feature boundaries (eye corners, lip edges). These edges are anchor decode targets.
- InsightFace's reference Python preprocessing uses `cv2.resize(..., interpolation=cv2.INTER_LINEAR)`
  — bilinear is the reference behavior the model was trained with.
- Mismatch between train-time and inference-time preprocessing degrades detection accuracy,
  particularly for small faces or faces at detection boundary sizes.
- The performance difference is negligible for the 640×640 single-frame preprocessing
  step (not a per-pixel hot path).

**Trade-offs:**
- Slightly more complex code (~30 extra lines) for the interpolation logic.
- No observable runtime cost difference at the scale of a single authentication frame.

---

### 3. Constant-Time Embedding Comparison

**Context:** `Embedding::similarity()` and `CosineMatcher::compare()` are used in
authentication decisions. Variable-time comparison can leak information through timing.

**Decision:**
- `Embedding::similarity()` always processes all dimensions — no early exit on zero norm.
  Uses a conditional assignment: `if denom > 0.0 { dot / denom } else { 0.0 }`.
- `CosineMatcher::compare()` always iterates all gallery entries — no early exit on
  match found. Best match is selected at end, not during traversal.

**Rationale:**
- A timing oracle on embedding comparison could leak: whether a near-match was found early
  in the gallery (leaking gallery ordering), whether any match was found at all (leaking
  enrollment status), or approximate similarity (leaking embedding proximity).
- While Visage v2 does not operate over a network, constant-time discipline is cheap to
  maintain now and expensive to retrofit when v3 adds remote auth scenarios.
- The performance cost is negligible: ArcFace gallery is typically 1-5 entries per user.

**Trade-offs:**
- `CosineMatcher` performs N comparisons even when the first entry is an exact match.
  At 512-D embeddings and 5 gallery entries, the cost is ~5μs total — not a bottleneck.
- The `similarity()` conditional assignment on zero-norm vectors changes the result
  compared to a simple early return, but the behavior is correct: zero-norm inputs
  produce zero similarity, not a division-by-zero panic.

---

### 4. L2-Normalized Embeddings

**Context:** The raw ArcFace ONNX output `[1, 512]` is unnormalized. Its magnitude is
arbitrary and varies with input face position, lighting, and alignment quality.

**Decision:** `FaceRecognizer::extract()` L2-normalizes the raw embedding immediately
after inference, before returning the `Embedding` struct. All stored embeddings are
unit-norm vectors on the 512-dimensional unit hypersphere.

**Rationale:**
- ArcFace is trained with Additive Angular Margin loss, which operates on the unit
  hypersphere. The embedding space is defined in terms of angles, not magnitudes.
- Cosine similarity between non-normalized vectors is equivalent to a dot product
  divided by varying magnitudes — the result is not comparable across embeddings with
  different magnitudes.
- L2-normalization at extraction time ensures that: `similarity(a, b) == dot(a, b)`
  (since both are unit vectors), and the cosine threshold semantics are consistent
  across all enrollments regardless of original magnitude.
- Normalizing at extraction time (rather than at comparison time) means stored
  embeddings are already in canonical form; comparison is just a dot product.

**Trade-offs:**
- None. This is the standard practice for ArcFace embeddings and has no downside.

---

### 5. 4-DOF Similarity Transform for Face Alignment

**Context:** Face alignment maps five detected landmarks to five reference positions in
the 112×112 ArcFace canonical space. The transform can be parameterized with 4 DOF
(uniform scale + rotation + translation) or 6 DOF (full affine).

**Decision:** Use a 4-DOF similarity transform, solved via least-squares over the
overdetermined system (5 point pairs = 10 equations, 4 unknowns).

**Rationale:**
- ArcFace was trained with similarity-transform alignment. The embedding space is
  calibrated to this specific alignment; using affine alignment at inference time
  degrades recognition accuracy.
- The 4-DOF formulation prevents shear and non-uniform scaling — both of which distort
  facial proportions without a recognition error signal. (The model still outputs an
  embedding; it's just wrong.)
- The overdetermined formulation (5 pairs, not 3) averages landmark detection noise
  rather than extrapolating from a minimal set. This makes the transform more robust
  to individual landmark placement errors.
- Gaussian elimination with partial pivoting solves the 4×4 normal equations without
  SVD, keeping the implementation dependency-free.

**Trade-offs:**
- 4-DOF cannot correct for facial roll beyond what the landmark geometry implies.
  If detected landmarks are inaccurate (low-quality detection), the transform may
  produce a suboptimal alignment. Mitigation: only call `extract()` on detections
  above the confidence threshold (0.5).

---

### 6. Y16 Pixel Format as First-Class

**Context:** V4L2 camera format negotiation may produce Y16 (16-bit grayscale, LE)
on some IR cameras. The prior implementation accepted only GREY (8-bit) and YUYV (4:2:2).

**Decision:** Extend `PixelFormat` enum to include `Y16`. Negotiate it in `Camera::open()`
and implement conversion: `(high << 8 | low) >> 8` extracts the top 8 bits.

**Rationale:**
- IR cameras commonly default to Y16. A library that silently negotiates Y16 but doesn't
  handle it produces buffer-length panics or silent corrupt frames, both of which are
  worse than a clear error at open time.
- The downscale from 16-bit to 8-bit loses dynamic range but is the correct approach
  for face detection input: SCRFD was trained on 8-bit normalized images.
- Supporting Y16 doubles hardware compatibility with no API changes.

**Trade-offs:**
- Discarding the lower 8 bits of Y16 loses fine-grained IR intensity information.
  For v3 liveness detection (depth-from-IR), the full Y16 values would be needed.
  Mitigation: `Frame` should eventually store pixel depth alongside grayscale. Deferred
  to Step 5 (IR emitter integration) where frame quality metadata is needed anyway.

---

### 7. PixelFormat Enum Over Boolean

**Context:** The prior `is_grey: bool` field on `Camera` was sufficient for two formats
but would require replacement (or a second bool) to add Y16 or future formats.

**Decision:** Replace `is_grey: bool` with `pub enum PixelFormat { Yuyv, Grey, Y16 }`.

**Rationale:**
- Enums are exhaustively matched by the Rust compiler; adding a new variant without
  handling it produces a compile error. A second `bool` would produce a runtime bug.
- `PixelFormat` is a clean domain concept; two booleans (`is_grey`, `is_y16`) are not.
- The public API change (`pub use camera::PixelFormat`) is additive — no existing code
  used `is_grey` directly.

**Trade-offs:**
- Public type change requires callers to update `match` arms when new variants are added.
  This is the desired behavior.

---

### 8. Alignment as Separate Module

**Context:** Face alignment logic (similarity transform estimation + affine warp) could
live in `recognizer.rs` as private functions.

**Decision:** Create `alignment.rs` as a separate public module.

**Rationale:**
- Alignment is independently testable. The `test_landmark_roundtrip` and transform tests
  provide confidence without requiring a loaded ONNX model.
- Alignment may be consumed by future components (e.g., a preprocessing CLI tool,
  a frame quality scorer that measures how well detected landmarks align with reference).
- Keeping it separate makes `recognizer.rs` focused on inference concerns; alignment
  is a geometric transform, not an inference operation.

**Trade-offs:**
- Slight additional API surface (`pub mod alignment`). Callers should use `align_face()`
  rather than calling the internal `estimate_similarity_transform()` or `warp_affine()`.

---

### 9. Backward-Compatible cosine_similarity() Alias

**Context:** `Embedding::cosine_similarity()` was the original name. Renaming it to
`similarity()` is the correct API (it shouldn't need the qualifier since Euclidean
distance is the only alternative), but existing code may reference the old name.

**Decision:** Mark `cosine_similarity()` as `#[deprecated(since = "0.1.0")]` and
forward it to `similarity()`. The deprecation warning guides callers to migrate.

**Rationale:**
- Hard removal at 0.1.0 would be premature — the crate hasn't shipped a stable release.
- Deprecation provides a migration path without breaking callers.
- The deprecated attribute produces a compiler warning, not an error — callers continue
  to work while being nudged toward the correct name.

**Trade-offs:**
- Dead code lint may fire for the alias if the caller never uses it. Suppressed at
  definition with `#[deprecated]` rather than `#[allow(dead_code)]`.

---

## Expected Benefits

1. **Integration-ready pipeline.** `visage-core` can now: load SCRFD + ArcFace from
   ONNX files, detect faces with landmarks in arbitrary grayscale frames, align detected
   faces to canonical 112×112 crops, extract 512-D normalized embeddings, and compare
   embeddings to an enrolled gallery with a configurable threshold. This is the complete
   Step 2 scope.

2. **Hardware compatibility doubled.** Y16 support expands camera compatibility to include
   IR cameras that default to 16-bit depth output. No API changes required by callers.

3. **Silent failure surface reduced.** Discovery-based tensor ordering with logging means
   SCRFD model incompatibility produces visible errors rather than wrong detections.

4. **Auth timing side-channels closed.** Constant-time comparison prevents timing oracles
   on enrollment status and match position. Cheap to maintain, expensive to retrofit.

5. **Step 3 (daemon) unblocked.** `visage-core` now has a stable public API:
   `FaceDetector`, `FaceRecognizer`, `CosineMatcher`, `MatchResult`, `Embedding`.
   The daemon can call these types directly without revisiting core inference.

---

## Drawbacks and Known Limitations

### 1. Integration Tests Require Downloaded Models

**Severity:** Medium
**Details:** Unit tests are comprehensive (36 tests, all without ONNX models). But
end-to-end tests — loading models, running real inference, verifying detection accuracy —
require `det_10g.onnx` and `w600k_r50.onnx` to be present. These are not included in
the repo (166MB + 16MB).

**Mitigation:** The `models/README.md` documents download instructions. A future
`cargo test --features integration` feature-gated test suite should be added that
skips gracefully when models are absent.

**Not a blocker for:** daemon implementation (Step 3), PAM module (Step 4).

---

### 2. ORT Execution Provider: CPU Only

**Severity:** Low for v2; architectural constraint for v3
**Details:** Session creation uses `Session::builder()?.commit_from_file()` with no
execution provider configured — this defaults to CPU. ONNX Runtime supports CUDA, CoreML,
DirectML, and Vulkan providers, but configuring them adds platform dependencies.

**Auth latency target:** ~60–80ms total (preprocess + detect + align + embed + compare)
on Ryzen AI 9 HX 370. Acceptable for a PAM auth prompt.

**v3 path:** `FaceDetector::load()` and `FaceRecognizer::load()` should accept an optional
`ExecutionProvider` argument. The current API (`load(&str)`) is too rigid for this. A
builder pattern would be the right retrofit:
```rust
FaceDetector::builder()
    .model_path("det_10g.onnx")
    .with_provider(ExecutionProvider::Cuda(Default::default()))
    .build()
```
Deferred to v3 as documented in the v3 vision.

---

### 3. Decoder Assumes 2 Anchors Per Cell

**Severity:** Low
**Details:** `decode_stride()` hardcodes `SCRFD_ANCHORS_PER_CELL = 2`. The SCRFD paper
describes variants with different anchor counts per cell for different model sizes.

The `det_10g` model uses 2 anchors per cell. If a different SCRFD variant is used with a
different anchor count, the decode loop produces wrong bounding boxes without an error.

**Mitigation:** Read anchor count from session metadata if available. If not, document
the assumption prominently. Currently documented as a named constant.

---

### 4. No Sanity Check at Model Load

**Severity:** Low
**Details:** `FaceDetector::load()` and `FaceRecognizer::load()` verify that the model
file exists and the session loads. They do not run a forward pass on a synthetic input
to verify the output tensor shapes match expectations.

A misconfigured model (wrong ONNX version, wrong model type, truncated file) is only
detected at the first real `detect()` or `extract()` call, not at load time.

**Mitigation:** Future hardening: run a single forward pass with a zero-filled tensor
at load time, check that output shapes are `[N, X, Y]` for SCRFD and `[1, 512]` for
ArcFace.

---

### 5. Single-threaded Session (2 intra-op threads)

**Severity:** Low
**Details:** Both sessions use `.with_intra_threads(2)`. For the auth use case (one
auth attempt at a time), this is appropriate. If the daemon ever serves concurrent
authentication requests (two users logging in simultaneously), both will contend for
the same 2-thread pool, serializing inference.

**Mitigation:** v2 daemon architecture (single-threaded D-Bus service) means only one
auth flows at a time. The thread count is a tuning parameter; 2 is a conservative choice
that avoids thread starvation on low-core-count machines.

---

### 6. Y16 Discards Lower 8 Bits

**Severity:** Low for v2; constraint for v3 liveness
**Details:** `PixelFormat::Y16` conversion: `(high << 8 | low) >> 8` keeps only the
upper 8 bits of each 16-bit IR pixel. This is appropriate for face detection (trained
on 8-bit inputs) but discards fine-grained IR intensity data.

**v3 impact:** Active liveness detection via structured IR light analysis (Step 5+) may
benefit from the full 16-bit IR response. This requires `Frame` to carry both `u8` and
`u16` pixel data, or separate frame types for detection vs. liveness frames.

**Mitigation:** Deferred. The current API matches v2 needs. The `Frame` struct should
be revisited when IR emitter integration (Step 5) is implemented.

---

### 7. No Enrollment Record Versioning

**Severity:** Low
**Details:** `FaceModel` stores `model_version: Option<String>` on the `Embedding` but
there is no schema version on `FaceModel` itself. If the enrollment data format changes
(different landmarks, different similarity transform, different normalization), old enrolled
embeddings are silently incompatible with new inference code.

**Mitigation:** v2 scope is single-user enrollment. If the model or normalization changes,
the user re-enrolls. A future `FaceModel::schema_version: u32` field would enable migration.

---

## Remaining Work to Fully Complete Visage

Step 2 is complete. The remaining steps are:

### Step 3: Daemon (visaged) — ✅ COMPLETE

Step 3 is implemented. See [ADR 003 — Daemon Integration](003-daemon-integration.md) for the full
decision log. Summary of what was built:

| Component | File | Status |
|-----------|------|--------|
| Config struct, env var loading | `visaged/src/config.rs` | ✅ |
| SQLite model store (WAL, per-user) | `visaged/src/store.rs` | ✅ |
| Engine thread bridge (mpsc + oneshot) | `visaged/src/engine.rs` | ✅ |
| D-Bus handlers (enroll, verify, list, remove, status) | `visaged/src/dbus_interface.rs` | ✅ |
| Daemon startup + SIGINT handling | `visaged/src/main.rs` | ✅ |
| CLI D-Bus proxy (all 5 commands) | `visage-cli/src/main.rs` | ✅ |
| D-Bus policy file (system bus, Step 4) | `packaging/dbus/org.freedesktop.Visage1.conf` | ✅ |

---

### Step 4: PAM Module (pam-visage)

`pam-visage/src/lib.rs` is a stub. It must implement:

| Task | Notes |
|------|-------|
| `pam_sm_authenticate()` with correct C signature | See `rules/linux-hw.md` for export pattern |
| Connect to D-Bus `org.freedesktop.Visage1` | Call `Verify(username)` with timeout |
| Return `PAM_IGNORE` on auth unavailable | Graceful fallback, not auth failure |
| Return `PAM_SUCCESS` / `PAM_AUTH_ERR` | Based on daemon response |
| syslog logging (`LOG_AUTHPRIV` facility) | PAM modules must not use stdout/stderr |

**Pattern reference:** `chissu-pam` architecture documented in `rules/linux-hw.md`.

---

### Step 5: IR Emitter Integration (visage-hw)

The `IrEmitter` stub exists. It must implement UVC control byte probing:

| Task | Notes |
|------|-------|
| UVC Extension Unit ioctl wrapper | `VIDIOC_G_EXT_CTRLS` / `VIDIOC_S_EXT_CTRLS` |
| Probing via `contrib/hw/*.toml` quirk files | Load quirk for detected VID:PID |
| `visage discover` CLI command | Probe all candidate (unit, selector) pairs |
| Emitter lifetime: activate before capture, deactivate after | RAII guard pattern |

---

### Step 6: Packaging

| Task | Notes |
|------|-------|
| Ubuntu `.deb` package (systemd service + PAM config) | Priority: Ubuntu 24.04 LTS |
| NixOS derivation in `augmentum-os` | `onnxruntime` must be linked, not downloaded |
| Model download helper (checksums) | `sha256sum` verification for both ONNX files |
| Integration test suite (`--features integration`) | Skips gracefully when models absent |
| `visage enroll` / `visage verify` CLI subcommands | End-to-end from command line |

---

### Cross-Cutting: Structured Event Log

The v3 vision document specifies that `MatchResult` events should be logged in a structured
format for offline threshold tuning. v2 should emit these events as tracing spans, not raw
logs, so they can be routed to a JSONL file if the user opts in.

This is a "data plane for v3" item — cheap now, expensive later. Recommended to add
before the daemon implementation (Step 3) rather than after.

---

## References

- **ADR 001:** `docs/decisions/001-camera-capture-pipeline.md`
- **ADR 002:** `docs/decisions/002-onnx-inference-kb-and-blocker-resolution.md`
- **Architecture:** `docs/architecture.md`
- **Threat model:** `docs/threat-model.md`
- **v3 Vision:** `docs/research/v3-vision.md`
- **PAM module patterns:** `~/.dotfiles/.claude/rules/linux-hw.md` (pam-chissu pattern)
- **Vault KB:** `Reference/Visage/InsightFace-Model-Reference`, `Reference/Visage/ORT-Rust-API-Reference`
