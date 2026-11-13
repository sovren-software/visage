# ADR 002 — ONNX Inference Preparation: KB Infrastructure and Blocker Resolution

**Date:** 2026-02-21
**Status:** Accepted
**Component:** `visage-core`, Vault Reference KB

---

## Context

Step 2 (ONNX inference — SCRFD face detection + ArcFace face recognition) was blocked
on two things before implementation could begin:

1. **Missing reference KB** — No offline documentation for InsightFace model I/O specs
   (SCRFD preprocessing, anchor decode, ArcFace normalization, similarity transform) or
   the ort 2.x Rust API. Without this, each implementation detail required runtime
   discovery and debugging rather than deliberate construction.

2. **ndarray version conflict** — `Cargo.toml` declared `ndarray = "0.16"` but
   `ort 2.0.0-rc.11` requires `ndarray = "0.17"`. This is a hard compile error:
   `TensorRef::from_array_view` expects `ArrayView` from ndarray 0.17; providing one
   from 0.16 produces a type mismatch that cannot be worked around at the call site.

This ADR documents the session that resolved both blockers and the decisions made.

---

## Decisions

### 1. InsightFace Model Selection: SCRFD det_10g + ArcFace w600k_r50

**What was selected:** `det_10g.onnx` (SCRFD 10G detection) + `w600k_r50.onnx`
(ArcFace ResNet50, trained on WebFace600K).

**Rationale:**

- **SCRFD det_10g** is the standard InsightFace production detector. The "10G" refers to
  GFLOPs, placing it between the lightweight variants (500M, 2.5G) and the heavy ones
  (34G). For a face authentication use case with a single enrolled face at enrollment
  distance, det_10g provides more than sufficient accuracy at acceptable CPU latency
  (~20-30ms on Ryzen AI).

- **ArcFace w600k_r50** is the production face recognition checkpoint used by most
  InsightFace integrations. ResNet50 at WebFace600K scale achieves state-of-the-art
  LFW accuracy (99.7%+) while remaining practical on CPU (~15-20ms).

- Both models are available as ONNX exports from the InsightFace model zoo — no
  PyTorch runtime required at inference time.

**Alternatives not taken:**

- `det_500m.onnx` — faster (~5ms) but lower accuracy; not justified for auth where we
  can afford 20ms
- `buffalo_l` model pack — bundles detection + recognition into a single download but
  requires InsightFace Python runtime; we need raw ONNX for pure Rust integration

**Trade-offs:**

- Model sizes: det_10g ~16MB, w600k_r50 ~166MB — acceptable for a system daemon
- Cold load latency: ~200ms + ~400ms one-time at daemon startup — not a per-auth cost
- CPU-only v2: no GPU acceleration. Target auth latency ~60-80ms total is acceptable
  for a PAM auth prompt where the user expects a brief pause

> **Amendment, 2026-11-13: the latency figures above did not hold on the reference
> machine.** Measured from the `verify timing` line `visaged` 0.5.0 logs for every
> verify, on an ASUS Zenbook 14 UM3406HA (Ryzen 7 8840HS, IR camera 640×360 GREY,
> `cpu_threads=2` by default), 7 verifies from 2026-11-11 to 11-13:
>
> | Stage | Assumed here | Measured (per call, median and range) |
> |---|---|---|
> | SCRFD det_10g | ~20-30ms | **213 ms** (141–489 ms), 3 detections per verify |
> | ArcFace w600k_r50 | ~15-20ms | **114 ms** (49–160 ms), 1–2 extractions per verify |
> | Whole verify | ~60-80ms | **1,020 ms** (750–2,087 ms), including ~300 ms of capture |
>
> The two slowest verifies were the first two after the daemon started. Every verify
> also skipped 2–3 dark frames, which is consistent with the IR strobe in #124.
>
> So the reason given for rejecting `det_500m` ("we can afford 20ms") does not hold:
> detection is the largest single cost of a verify. The choice is **not re-decided**
> here. det_500m has lower recall, and a missed face costs a retry (200 ms) or a failed
> login. Deciding needs a benchmark on real frames from this camera that measures speed,
> face recall and the effect on match rate. The thread count, OpenVINO and the 640 input
> size are other levers to measure in that benchmark.

---

### 2. 4-DOF Similarity Transform (Not Full Affine) for Face Alignment

**Decision:** Face alignment from detected landmarks to the 112×112 ArcFace crop
uses a 4-DOF similarity transform (uniform scale + rotation + translation) rather
than a 6-DOF affine transform.

**Rationale:**

- ArcFace was trained with alignment using the 4-DOF similarity transform. Using the
  same transform at inference time is a correctness requirement, not a choice — the
  embedding space is calibrated to this alignment.

- A full affine transform (6-DOF) allows independent x/y scaling and shear, which can
  distort facial proportions. This produces misaligned crops and degraded recognition
  accuracy, with no error signal at inference time (the model still produces an embedding,
  it's just wrong).

- The 4-DOF least-squares solve over 5 point pairs is overdetermined (10 equations, 4
  unknowns) and numerically stable. It matches the reference InsightFace implementation.

**Implementation:** Standard least-squares via normal equations `A^T A x = A^T b`.
Reference landmarks (112×112 canonical positions) documented in the Reference KB.

**Trade-off accepted:** Slightly more implementation complexity than using `cv2::warpAffine`
equivalents directly. The correctness requirement makes this non-negotiable.

---

### 3. Normalization Constants: Per-Model, Not Shared

**Decision:** Different normalization constants for SCRFD and ArcFace are treated as
first-class documented constants, not unified.

| Model | Normalization | Formula |
|-------|--------------|---------|
| SCRFD | `(x - 127.5) / 128.0` | divide by 128 |
| ArcFace | `(x - 127.5) / 127.5` | divide by 127.5 |

**Rationale:** The difference (`/128.0` vs `/127.5`) is subtle and silent — the model
accepts both inputs without error but produces degraded embeddings with the wrong constant.
Documenting them explicitly in the Reference KB and as named constants in code prevents
the confusion from recurring.

**Risk:** Mixing constants produces no compile-time or runtime error — only degraded
recognition accuracy at threshold evaluation. Mitigation: named constants in each
preprocessing function, not magic numbers.

---

### 4. Cosine Similarity Over Euclidean Distance for Embedding Comparison

**Decision:** `Embedding::similarity()` uses cosine similarity (dot product of
L2-normalized vectors), not Euclidean distance.

**Rationale:**

- ArcFace is trained with Additive Angular Margin (ArcFace) loss, which explicitly
  optimizes for angular separability on a hypersphere. Cosine similarity measures the
  angle between vectors — it is geometrically appropriate to this embedding space.

- Euclidean distance on non-normalized embeddings does not produce consistent threshold
  values. The raw ArcFace output magnitude is arbitrary; L2 normalization followed by
  cosine similarity produces a value in [-1, 1] with stable threshold semantics.

- Recommended authentication threshold: 0.45 (strict, ~0.01% FAR) or 0.40 (balanced,
  ~0.1% FAR). These values are empirically established on the model's test set.

**Implementation requirement:** Embeddings must be L2-normalized *after* inference,
*before* storage or comparison. The raw output `[1, 512]` is not normalized.

---

### 5. ndarray 0.17 Upgrade (Compile Blocker)

**Decision:** Bump `ndarray = "0.16"` → `"0.17"` in workspace `Cargo.toml`.

**Rationale:**

- `ort 2.0.0-rc.11` depends on ndarray 0.17. The `TensorRef::from_array_view()` API
  takes `ArrayBase<ViewRepr<&T>, D>` from ndarray 0.17's type definitions. Providing
  an identical type from ndarray 0.16 produces a type mismatch (Rust treats them as
  distinct types from distinct crate versions) that cannot be resolved at the call site.

- ndarray 0.16 → 0.17 is a non-breaking upgrade for Visage's usage. The APIs used
  (`Array4`, `ArrayView4`, `ArrayViewD`, `.view()`, `.as_slice()`, `Axis`, `s![]`)
  are unchanged between versions.

**Verification:** `cargo check -p visage-core` passes cleanly after the bump. ndarray
0.17.2 resolves and links correctly with ort 2.0.0-rc.11.

---

### 6. Reference KB Priority Split (P0/P1/P2)

**Decision:** Build 2 of 5 Visage KB pages now (P0 — directly unblock Step 2);
defer 3 pages to the steps that require them.

| Page | Priority | Status | Unblocks |
|------|----------|--------|---------|
| InsightFace-Model-Reference | P0 | Active | Step 2 (detector.rs, recognizer.rs) |
| ORT-Rust-API-Reference | P0 | Active | Step 2 (session creation, tensor I/O) |
| V4L2-IR-Emitter-Reference | P1 | Planned | Step 5 (IR emitter ioctl) |
| ZBUS-DBus-Reference | P2 | Planned | Step 3 (daemon D-Bus API) |
| PAM-Module-Development | P2 | Planned | Step 4 (pam-visage cdylib) |

**Rationale:** Writing KB pages before the implementation step that needs them produces
speculative documentation that may be wrong by the time it's used. P1/P2 pages are
written immediately before Steps 5, 3, and 4 respectively — not preemptively.

---

## Expected Benefits

1. **Step 2 implementation is unblocked.** The Reference KB provides all SCRFD/ArcFace
   specs needed to implement `detector.rs` and `recognizer.rs` without runtime discovery.

2. **Normalization bug prevented.** The `/127.5` vs `/128.0` distinction is documented
   prominently with a "CRITICAL" callout — the most common InsightFace integration mistake.

3. **Alignment correctness locked in.** The 4-DOF requirement and reference landmark
   coordinates are in the KB, preventing the silent-degradation affine-vs-similarity
   confusion.

4. **vault-inject wires KB to future sessions.** Any session mentioning "visage", "scrfd",
   "arcface", or "ort onnx" will have the Reference KB injected automatically, making
   the Step 3/4/5 implementation sessions self-documenting-infrastructure-aware.

---

## Drawbacks and Known Limitations

### P1/P2 KB pages not yet written

**Severity:** Low (by design)
**Impact:** Step 3/4/5 sessions will not have D-Bus, PAM, or IR emitter reference KB
available from day one. These sessions must write the KB page immediately before or
during implementation.
**Resolution:** Planned. The `_INDEX.md` lists them as "Planned" with explicit step
references.

### CPU-only inference (v2 scope decision)

**Severity:** Low for v2; architectural constraint for v3
**Impact:** Auth latency target is ~60-80ms total pipeline (detect + align + recognize).
This is acceptable for PAM auth but may feel slow relative to Windows Hello on hardware
with a dedicated NPU (the Zenbook has AMD XDNA).
**Resolution:** v3 vision document defines the GPU/NPU inference path. The ort CUDA
feature and Vulkan execution provider paths are designed in but not enabled in v2.
The `FaceDetector`/`FaceRecognizer` structs should be designed so the session creation
is isolated — swapping execution providers does not require API changes.

### Anti-spoofing is out of scope (v2)

**Severity:** Medium (security limitation)
**Impact:** v2 has no liveness detection. A high-quality photo of the enrolled user
could in principle bypass authentication.
**Mitigations in v2:** IR camera (not RGB) — prints and screens don't reflect IR
the same way as skin; physical IR emitter illumination. These provide passive liveness
signal without active detection.
**Resolution:** v3 adds active liveness (blinking, micro-expression detection, depth
from structured light). Documented in threat model and v3 vision.

### NixOS offline build requires manual ORT_LIB_LOCATION

**Severity:** Low (affects packaging only)
**Impact:** The ort crate downloads the ONNX Runtime shared library at build time by
default. This fails in the Nix sandbox (`network_access = false`). Packaging requires
either `fetchurl`-ing the ort binary or linking against `nixpkgs.onnxruntime`.
**Resolution:** Deferred to Step 6 (Ubuntu packaging). For local development builds,
`ORT_LIB_LOCATION` workaround works. NixOS packaging is explicitly a v2.1 concern.

### Corpus coverage shows 2/5 pages (WARN)

**Severity:** Expected / not a defect
**Impact:** `corpus-coverage-check.sh` reports WARN for Visage (2/5 pages). This is
intentional — P1/P2 pages are deliberately deferred.
**Resolution:** The WARN is correct behavior. It will resolve as Steps 3–5 are implemented
and the corresponding KB pages are written.

---

## Remaining Work (Step 2)

The immediate remaining work is implementing `detector.rs` and `recognizer.rs`:

| Task | File | Complexity |
|------|------|-----------|
| SCRFD preprocessing (resize + normalize) | `detector.rs` | Medium |
| Anchor grid decode (3 strides × {scores, bboxes, kps}) | `detector.rs` | Medium |
| NMS implementation | `detector.rs` | Low |
| Similarity transform (4-DOF least-squares) | `recognizer.rs` | Medium-High |
| Face crop (112×112 with alignment) | `recognizer.rs` | Medium |
| ArcFace preprocessing (normalize + NCHW) | `recognizer.rs` | Low |
| L2 normalization of output embedding | `recognizer.rs` | Low |
| `Embedding::similarity()` (cosine) | `types.rs` | Low |
| Model download infrastructure | new: `models.rs` or `cli` | Low |
| Integration test: end-to-end detect + recognize | `tests/` | Medium |

### Model Download Infrastructure

Models need to be present at `~/.local/share/visage/models/` for the session.
The `visage test` or `visage enroll` CLI commands should check for model presence
and emit a clear error (not a panic) if missing, pointing to the download location.

---

## References

- InsightFace model zoo: `github.com/deepinsight/insightface`
- ort crate: `docs.rs/ort/2.0.0-rc.11`, `github.com/pykeio/ort`
- Vault Reference KB: `Reference/Visage/` (InsightFace-Model-Reference, ORT-Rust-API-Reference)
- Step 1 ADR: `docs/decisions/001-camera-capture-pipeline.md`
- Domain audit: `docs/research/domain-audit.md`
