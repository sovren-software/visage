# Visage Technical Domain Audit

**Date:** 2026-02-21
**Scope:** All technical domains Visage depends on — v2 implementation and v3 horizon.
**Purpose:** Identify what is understood, what is unknown, and which reference material to acquire.

---

## What Is Already Understood

The following is verified by working code and live hardware tests:

- **V4L2 camera capture** — format negotiation (GREY/YUYV), mmap streaming, dark frame filtering, CLAHE. All working on `/dev/video2`. 9/9 unit tests pass.
  - **Gap confirmed (chissu-pam audit 2026-02-21):** Y16 format not handled. Some IR cameras output 16-bit little-endian. Fix: `>> 8` high-byte extract in `buf_to_grayscale()`. ~10 lines.
- **Frame data model** — `Frame` struct, YUYV→grayscale, histogram analysis implemented.
- **ONNX crate selection** — `ort = "2.0.0-rc.11"` with `ndarray = "0.17"` (bumped from 0.16 — compile blocker resolved 2026-02-21). Session API documented in `Reference/Visage/ORT-Rust-API-Reference.md`.
- **InsightFace model specs** — SCRFD det_10g.onnx I/O, anchor grid decode, ArcFace w600k_r50.onnx normalization (`/127.5`), 4-DOF similarity transform, L2 normalization, cosine similarity. All documented in `Reference/Visage/InsightFace-Model-Reference.md`.
- **D-Bus interface skeleton** — zbus 5 `#[interface]` macro, method signatures, D-Bus security policy design (peer credential checks, XML policy for `Visage1`).
- **PAM return code contract** — `PAM_IGNORE` (25) on all non-success paths. Never `PAM_AUTH_ERR`.
  - **Gap confirmed (chissu-pam audit 2026-02-21):** `pam_sm_authenticate` signature is wrong — zero args, must be 4 (`pamh`, `flags`, `argc`, `argv`). Missing `pam_sm_setcred` export. No PAM conversation for user feedback. All three are Step 4 scope but the signature must be fixed before pam-visage can link.
- **SQLite schema design** — embeddings as variable-length BLOB, WAL mode, root-only permissions, `quality_score` and `pose_label` columns planned.
- **Quirks database format** — TOML files, VID:PID keyed, one file per camera model.
- **System file layout** — all production paths documented in the roadmap.
- **Threat model** — tier structure, attack surface, D-Bus confused-deputy pattern.

---

## Domain Audit by Group

### Group 1: Hardware & Kernel Interfaces

#### 1.1 V4L2 — Camera Capture (Step 1, complete)

**Complexity:** Medium | **Status:** Done

What works: format negotiation, streaming, both GREY and YUYV dispatch, frame metadata.

**Remaining concern:** `capture_frame()` creates a fresh `MmapStream` per call. The daemon should hold the stream open across auth sessions to avoid re-initialization latency. This tension is unresolved in the skeleton — address in Step 3.

**Knowledge base:** V4L2 API spec (streaming/mmap chapter) — for daemon camera management.

---

#### 1.2 UVC Extension Unit — IR Emitter (Step 5)

**Complexity:** High | **Status:** Stub only

**What Visage needs:**
- `UVCIOC_CTRL_SET` ioctl with `uvc_xu_control_query` struct
- `AsRawFd` on `v4l::Device` (confirmed available) to bypass the crate abstraction
- VID:PID reading from `/sys/class/video4linux/video*/device/idVendor|idProduct`
- Quirks auto-detection at daemon startup

**Unknown (must research before Step 5):**
- `uvc_xu_control_query` struct layout (pointer field, 64-bit padding) — check `linux/usb/video.h`
- `UVCIOC_CTRL_SET` ioctl number (`_IOWR('u', 6, ...)`) — verify in kernel headers, not assumed
- Whether `nix::ioctl_readwrite!` macro handles pointer fields correctly at the call site

**Knowledge base needed:**
- `linux/usb/video.h` — `uvc_xu_control_query` struct, ioctl numbers
- UVC 1.5 spec — extension unit descriptor format (for v3 descriptor-guided probing)
- `linux-enable-ir-emitter` source — existing Python reference implementation

**Risk:** Struct alignment errors produce `EFAULT` with no obvious diagnostic. Define struct via `bindgen` or verify byte-by-byte against the C definition.

---

#### 1.3 /sys Device Identity

**Complexity:** Low | **Status:** Designed, not implemented

Reading `/sys/class/video4linux/video*/device/idVendor` and `idProduct` as hex strings. Straightforward `std::fs::read_to_string`. Handle missing file (PCI-attached cameras have no VID:PID).

---

### Group 2: Rust Systems Programming

#### 2.1 PAM C FFI (Step 4)

**Complexity:** Critical | **Status:** Wrong stub

**The current stub has 0 arguments.** The correct signature is:

```rust
#[no_mangle]
pub unsafe extern "C" fn pam_sm_authenticate(
    pamh: *mut PamHandle,
    flags: c_int,
    argc: c_int,
    argv: *const *const c_char,
) -> c_int
```

Must also export `pam_sm_setcred` (trivially returns `PAM_SUCCESS`).

**Unknown (must research before Step 4):**
- `pam_handle_t` as an opaque struct definition for Rust FFI
- `PAM_USER`, `PAM_IGNORE`, `PAM_SUCCESS` constant values for Ubuntu 24.04 (check `/usr/include/security/_pam_types.h`)
- Whether `zbus::blocking::Connection` is safe to use inside a `cdylib` loaded by PAM (no tokio runtime needed)
- The correct call for username extraction: `pam_get_user` vs. `pam_get_item(PAM_USER)` — they differ subtly

**Knowledge base needed:**
- Linux-PAM Module Writers' Guide — full function signatures, return code table, item constants
- `pam_unix.c` source — reference implementation of a real PAM module
- `/usr/include/security/pam_modules.h` on Ubuntu 24.04 — exact constant values

**Risk:** PAM modules load into the calling process (sudo, sshd, gdm). A crash hangs the terminal. A wrong `PAM_AUTH_ERR` return blocks password fallback. Test exclusively on a VM first.

---

#### 2.2 UVC ioctl via Unsafe Rust

Covered in §1.2. The Rust-specific concern: `repr(C)` alignment of `uvc_xu_control_query` with a raw pointer field. Use `bindgen` or manual `repr(C)` with explicit size assertion (`assert_eq!(size_of::<UvcXuControlQuery>(), 16)`).

---

#### 2.3 Async Daemon Architecture (Step 3)

**Complexity:** Medium | **Status:** Skeleton

**Design decisions to make before Step 3:**
- Camera state: `Arc<Mutex<Camera>>` serializes all auth — acceptable for v2 (no concurrent auth needed)
- ONNX inference is synchronous C++: wrap in `tokio::task::spawn_blocking`
- `request_name` flags: `ReplaceExisting | DoNotQueue` to prevent dual-daemon
- Graceful SIGTERM: `tokio::signal::unix::signal(SignalKind::terminate())`

**Unknown:**
- zbus 5 peer credential API for UID extraction (changed from zbus 3) — check zbus 5 changelog
- Whether `#[interface]` methods can take `&self` with `Arc<Mutex<>>` interior state, or need `&mut self`

**Knowledge base needed:** zbus 5 Book (system bus, ObjectServer, peer credentials chapters).

---

#### 2.4 SQLite from Async Context (Step 3)

**Complexity:** Medium

`rusqlite` is synchronous. Wrap all DB calls in `tokio::task::spawn_blocking`. Use WAL mode from first open. Embedding serialization: 512 × `f32` as little-endian bytes (2048 bytes per embedding) — must use the same byte order in both `Enroll` and `Verify`.

**Knowledge base:** rusqlite docs. No additional reference material needed.

---

### Group 3: Face Recognition Pipeline

#### 3.1 ONNX Runtime — ort 2.x (Step 2)

**Complexity:** High | **Status:** Not started

**Unknown (must resolve before Step 2):**
- `ort` 2.x input/output tensor API — API changed significantly from 1.x. Do not use 1.x examples.
- Build-time model download: `ort` downloads `libonnxruntime.so` at build time. For packaging, need `load-dynamic` feature with vendored library.

**Knowledge base needed:** ort 2.x API docs (https://docs.rs/ort/latest/ort/). Read the 2.x migration guide explicitly.

---

#### 3.2 SCRFD — Face Detection (Step 2)

**Complexity:** Critical | **Status:** Not started

**What must be determined before writing any code:**

1. **Output tensor names and shapes for `buffalo_l/det_10g.onnx`** — inspect with Netron before writing any post-processing code. SCRFD-10G outputs anchors at strides 8, 16, 32 — each stride has a classification tensor and a regression tensor. The exact names must match what is in the graph.

2. **Is NMS baked in?** Some SCRFD export variants include NMS in the ONNX graph. If so, outputs are decoded boxes directly. If not, must implement NMS.

3. **Input normalization** — SCRFD from InsightFace uses pixel values in `[0, 255]` range, NOT `[0.0, 1.0]`. This differs from standard convention. Verify per model.

4. **Grayscale to RGB** — SCRFD was trained on RGB. IR frames must be channel-replicated (R=G=B=Y value) before feeding. This is not documented in the codebase.

**Unknown (critical path):**
- Anchor generation: center coordinates and sizes at each stride — must match training configuration exactly. Wrong anchors produce wrong box coordinates with no visible error.
- `BoundingBox` in `types.rs` lacks `landmarks: [[f32; 2]; 5]` — this field is required for facial alignment. Must add before Step 2.

**Knowledge base needed:**
- InsightFace SCRFD source — anchor configuration, output format
- SCRFD paper (https://arxiv.org/abs/2105.04714) — output head description
- Netron (visual ONNX graph inspector) — inspect actual tensor names and shapes before coding

---

#### 3.3 Facial Alignment — Affine Warp (Step 2)

**Complexity:** High | **Status:** Not mentioned anywhere in codebase

This is a **silent gap**: SCRFD provides 5 landmarks, ArcFace requires a 112×112 canonical pose, but nothing in the codebase performs the affine transformation between them.

**What is needed:**
- Reference landmark positions for 112×112 canonical face pose (hardcoded constants from InsightFace's `face_align.py`)
- Least-squares affine transform estimation from 5 point correspondences
- Affine warp: `image = "0.25"` cannot do this — need `imageproc` crate or manual implementation

**Unknown:**
- Reference landmark constants (must extract from InsightFace `face_align.py::norm_crop()`)
- Whether to add `imageproc` dependency or implement the affine solve manually via `nalgebra`

**Knowledge base needed:** InsightFace `face_align.py` — extract the `src` array (reference landmark positions) and the `norm_crop` function. These are ~20 lines of Python that must be ported to Rust.

---

#### 3.4 ArcFace — Recognition (Step 2)

**Complexity:** Medium | **Status:** Stub (once SCRFD + alignment work)

**Input:** Aligned 112×112 face crop, normalized to `[-1.0, 1.0]` (pixel / 127.5 - 1.0 for InsightFace models).
**Output:** `[1, 512]` f32 embedding. L2-normalize before storing if the model does not do so internally.

**Unknown:**
- Does `buffalo_l/w600k_r50.onnx` output normalized embeddings? Inspect with Netron. If not, must L2-normalize in `embed()`.

**Threshold note:** The 0.5 cosine threshold is a starting point. Will require empirical tuning on actual hardware — IR cameras vary. The structured event log in v2 is what enables this calibration.

---

### Group 4: Linux Security & IPC

#### 4.1 D-Bus System Bus — zbus 5 (Step 3)

**Complexity:** Medium | **Status:** Skeleton

**Unknown:**
- zbus 5 peer credential extraction API — changed from zbus 3. Must read zbus 5 changelog.
- Whether `#[interface]` methods require `&self` or `&mut self` with interior mutability

**Security requirement:** D-Bus XML policy is a coarse filter. The Rust code must enforce `caller_uid == user_param_uid` as the authoritative check. The policy alone is insufficient.

**Knowledge base needed:** zbus 5 Book (system bus chapter, peer credentials, ObjectServer).

---

#### 4.2 systemd Unit + Hardening (Step 3/6)

**Complexity:** Medium | **Status:** Designed

The unit stanza is fully specified in the roadmap. Key elements: `ProtectSystem=strict`, `NoNewPrivileges`, `DeviceAllow=/dev/video* rw`, `ReadWritePaths=/var/lib/visage /run/visage`. `CapabilityBoundingSet=` empty (correct for a root daemon that does not need to escalate).

**Gap:** Suspend/resume hook. The v0.1 gate requires IR emitter re-activation after suspend. This needs a `visage-resume.service` with `After=suspend.target`, `WantedBy=suspend.target`. Not yet in the design.

**Knowledge base:** systemd.exec(5) for hardening options. No new KB needed — roadmap has the unit stanza.

---

#### 4.3 Anti-Spoofing — Tier 0 (Step 3)

**Complexity:** Medium

Tier 0 mitigations are designed but not implemented:
- Multi-frame confirmation: require N consecutive matches above threshold
- Rate limiting: lock after M failures within T seconds
- Constant-time embedding comparison: current `cosine_similarity()` has early-exit on zero norm — not constant-time. Fix before v0.1.

No new knowledge base needed. These are straightforward implementation tasks.

---

### Group 5: Packaging & Distribution

#### 5.1 Debian Packaging — cargo-deb (Step 6)

**Complexity:** High | **Status:** Not started

**Critical unknowns:**
1. **ONNX Runtime distribution** — Ubuntu 24.04 does not ship `libonnxruntime`. Options:
   - Bundle `libonnxruntime.so` in the deb (simplest, but adds ~10MB to package size)
   - `ort load-dynamic` feature + vendored copy at `/usr/lib/visage/`
   - Static linking (if ort supports it — verify)
   This must be decided before Step 6 to avoid late-stage packaging rework.

2. **cargo-deb workspace** — requires `--package visaged` and `--package visage-cli` separately; PAM module is a separate `cdylib` crate with its own packaging requirements.

3. **Model download in postinst** — downloading from the internet during `apt install` is fragile. Use a lazy download triggered by `visage-models download` on first use, not in postinst.

**Knowledge base needed:** cargo-deb docs, Debian Policy Manual §10.9 (shared libraries), pam-auth-update(8) man page.

---

#### 5.2 pam-auth-update Integration (Step 6)

**Complexity:** Medium

Profile file at `/usr/share/pam-configs/visage`. Use `Auth-Type: Primary` and `[success=end default=ignore]` flags (not `sufficient`).

**Risk:** If `postinst` fails mid-execution with a partially modified PAM stack, the system may be in a broken auth state. Use `set -e` in postinst. Test on a clean Ubuntu 24.04 VM as the first thing in Step 6.

**Knowledge base:** pam-auth-update(8) man page — exact profile file format.

---

### Group 6: v3 Domains (future — not blocking v2)

#### 6.1 Voice Biometrics

**Complexity:** Critical | **v3 only**

Pipeline: ALSA/PipeWire capture → Silero VAD → ECAPA-TDNN or TitaNet speaker embedding → cosine match. New `visage-audio` crate (explicitly separate from `visage-hw`).

**Open questions (from v3-vision.md):**
- Enrollment phrase: user-chosen vs. standardized vs. random challenge?
- Multi-modal confidence fusion: face and voice scores are on incompatible scales — calibration required
- PipeWire is session-scoped: voice auth may not work for pre-login PAM contexts (e.g., dm, sshd)

**Knowledge base (acquire when v3 begins):** ECAPA-TDNN paper, Silero VAD repo, NIST SRE benchmarks, ISO/IEC 24745.

---

#### 6.2 Hardware Compatibility Classifier

**Complexity:** High (data problem, not ML problem) | **v3 only**

A small ONNX model trained on UVC descriptor features to predict emitter control bytes. The bottleneck is community data — requires hundreds of quirk entries.

**v2 prerequisite:** `visage discover` must output structured JSON. Build data collection infrastructure first.

---

#### 6.3 LLM Assistant (`visage-assistant`)

**Complexity:** Low | **v3 only**

Separate binary only. Zero LLM code in core crates. No blocking dependency.

---

## Knowledge Base Acquisition Plan

Priority based on implementation sequence:

| Priority | Knowledge Base | Needed For | Source |
|----------|---------------|-----------|--------|
| **Critical** | InsightFace face_align.py — reference landmark constants and `norm_crop` function | Step 2 — facial alignment | https://github.com/deepinsight/insightface |
| **Critical** | ort 2.x docs — input/output tensor API, Session construction | Step 2 — ONNX inference | https://docs.rs/ort/latest/ort/ |
| **Critical** | SCRFD output format — tensor names/shapes from `det_10g.onnx` via Netron | Step 2 — detection post-processing | Inspect model directly |
| **Critical** | Linux-PAM Module Writers' Guide — `pam_sm_authenticate` signature, constants, items | Step 4 — PAM module | https://www.man7.org/linux/man-pages/ |
| **High** | zbus 5 Book — system bus, peer credentials, ObjectServer | Step 3 — daemon | https://dbus2.github.io/zbus/ |
| **High** | `linux/usb/video.h` — `uvc_xu_control_query` struct, ioctl numbers | Step 5 — IR emitter | Kernel source tree |
| **High** | UVC 1.5 spec — extension unit descriptor format | Step 5 + v3 probing | USB.org |
| **High** | cargo-deb + Debian Policy §10.9 — workspace packaging, shared library rules | Step 6 | https://www.debian.org/doc/debian-policy/ |
| **High** | pam-auth-update(8) — profile file format | Step 6 | Ubuntu man page |
| **Medium** | V4L2 API spec — streaming/mmap lifecycle | Step 3 (daemon camera mgmt) | https://www.kernel.org/doc/html/latest/userspace-api/media/ |
| **Medium** | rusqlite docs — WAL, spawn_blocking pattern | Step 3 | https://docs.rs/rusqlite/ |
| **Low** | ECAPA-TDNN paper, Silero VAD, NIST SRE | v3 — voice | When v3 begins |
| **Low** | Biometric fusion literature (NIST BTAS) | v3 — multi-modal | When v3 begins |

---

## Silent Gaps (Not in Codebase, Not in Roadmap)

These are domains that were discovered by the audit but are not yet documented anywhere:

1. **Facial alignment affine warp** — no code, no mention. Required between SCRFD landmarks and ArcFace input. Must extract InsightFace reference constants before writing Step 2.

2. **Grayscale→RGB channel replication for SCRFD** — IR frames are 1-channel; SCRFD expects 3-channel RGB. Must replicate Y channel to R, G, B before inference.

3. **Suspend/resume IR emitter hook** — the v0.1 gate requires this but no systemd sleep hook is designed. Add to Step 5 or Step 6 scope.

4. **Constant-time embedding comparison** — `cosine_similarity()` has an early-exit path on zero norm. For security correctness (timing side-channel), this must be constant-time before v0.1.

5. **mmap stream lifecycle in daemon** — `capture_frames()` opens a fresh stream per call. The daemon should pre-warm the stream. This architectural decision affects Step 3 design.

---

## References

- [STRATEGY.md](../STRATEGY.md) — locked v2→v3 growth map
- [architecture-review-and-roadmap.md](architecture-review-and-roadmap.md) — v2 step-by-step implementation
- [v3-vision.md](v3-vision.md) — full v3 architecture analysis
- [../threat-model.md](../threat-model.md) — threat model and attack mitigations
