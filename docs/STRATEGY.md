# Visage Strategy

**Status:** Living — reflects committed direction. Updated as versions ship.
**Last reviewed:** 2026-02-23

---

## The Position

Visage is the Windows Hello equivalent for Linux.

Linux has had face authentication since Howdy. Howdy proved the concept is viable. It also
proved that a Python subprocess spawned per auth attempt, with world-readable model files
and no anti-spoofing, is not a foundation worth building on.

Visage is the replacement: a persistent Rust daemon, encrypted embeddings at rest, D-Bus IPC
following fprintd precedent, and a PAM module that never blocks. The architecture is correct.
The security boundary is correct. The result is a tool that works reliably enough to actually use.

---

## The Problem Howdy Has

- PAM module spawns a Python subprocess per auth attempt — 2-3s cold start every time
- dlib is being dropped from Fedora 43 and is effectively unmaintained upstream
- Model files are world-readable (`/etc/howdy/models/`)
- Default threshold produces unacceptable false-accept rates
- Three camera backends — two with known bugs
- No IR emitter integration — visible-light spoofing is trivial

Linux deserves a biometric authentication layer that is **reliable, secure, and maintainable**.

---

## Where We Are: v0.1.0

**Shipped 2026-02-23. All 6 implementation steps complete. End-to-end tested on Ubuntu 24.04.4 LTS.**

| Component | What it delivers |
|-----------|-----------------|
| `visage-hw` | V4L2 capture, GREY/YUYV/Y16 format detection, CLAHE preprocessing, dark frame rejection |
| `visage-core` | SCRFD face detection + ArcFace recognition via ONNX Runtime — CPU-capable, no CUDA required |
| `visaged` | Persistent daemon — holds camera and model weights across auth requests, D-Bus IPC, SQLite WAL |
| `pam-visage` | Thin PAM module — `PAM_IGNORE` fallback, never blocks, system bus |
| IR emitter | UVC extension unit control, hardware quirks database, ASUS Zenbook 14 UM3406HA confirmed |
| Packaging | `.deb` with `pam-auth-update`, systemd hardening, AES-256-GCM embeddings at rest |

Visage authenticates in ~1.4s on CPU with a USB webcam. Howdy's Python subprocess cold-start
is 2-3s. Visage is already faster — without IR camera or GPU — because model weights are
loaded once at daemon start, not per attempt. That is the architectural advantage.

---

## Ecosystem Position

Visage is the identity layer of the Sovren Software stack.

**Sequencing:** Visage ships publicly before Augmentum OS. Early adopters validate hardware,
contribute quirk entries, and prove the install lifecycle. By the time Augmentum OS ships,
Visage has a live user base and a hardware compatibility matrix built from real deployments.

**Flywheel:** Users install Visage on Ubuntu, Arch, Fedora → they see *"the default face
authentication layer for Augmentum OS"* → they anticipate the full system. Visage builds
credibility for Augmentum OS with zero feature coupling.

**Boundary:** Visage is facial authentication only. Gesture tracking, behavioral biometrics,
and voice belong to the Augmentum OS layer. This is enforced in [CONTRIBUTING.md](../CONTRIBUTING.md).

---

## Roadmap

### v0.2 — Public Launch (Summer 2026)

Coordinated announcement across r/linux, r/rust, r/privacy, Hacker News, and Phoronix.
The announcement leads with the architectural story: persistent daemon, no subprocess overhead,
encrypted embeddings at rest. The benchmark provides the concrete numbers.

| Item | Status |
|------|--------|
| AUR PKGBUILD | ✅ `packaging/aur/` |
| NixOS derivation | ✅ `packaging/nix/` — flake wiring pending |
| COPR RPM spec | ⬜ `packaging/rpm/` |
| Howdy vs Visage benchmark | ⬜ Matched hardware, published methodology |
| Active liveness detection | ⬜ Blink challenge — proof of concept |
| Enrollment quality scoring | ⬜ Reject blurry / dark / partial frames at capture |
| `visage discover --json` | ⬜ Structured output — required for v3 hardware classifier |

### v0.3 — Hardware Breadth

- Intel IPU6 camera support via libcamera
- GPU-accelerated inference (OpenCL / Vulkan)
- Per-user adaptive similarity threshold
- Enrollment quality model (ONNX, lightweight)
- Sub-500ms authentication on IR camera + GPU path

### v3 — The Platform

v3 transforms Visage from a reliable face auth tool into an intelligent biometric platform.
Full architecture: [research/v3-vision.md](research/v3-vision.md)

**Near-Zero-Config Hardware Support** — UVC descriptor-guided probing reduces search space
from ~1000 to ~15 safe attempts. Camera fingerprinting at first boot. Automatic CLAHE tuning.
Community quirk repository with structured submission pipeline.

**Environmental Adaptation** — Per-user adaptive threshold derived from enrollment quality and
match history. Multi-enrollment with quality scoring. Failed-match learning loop tightens
thresholds against attack patterns without degrading normal-variance tolerance.

**AI-Assisted System Intelligence** — Hardware compatibility classifier (trained on community
UVC data), enrollment quality model, anomaly detection. All ONNX. None in the auth path.

**LLM-Assisted Lifecycle Management** — `visage assistant`: guided IR emitter discovery, auth
failure diagnosis, quirk submission. Separate binary. No LLM dependency in any core crate. Ever.

**Multi-Modal Biometric Platform** — Face + voice via `org.freedesktop.Visage2`. Three fusion
modes: parallel (OR gate), sequential (face-first, voice on marginal confidence), continuous
(session-scoped). `Visage1` is permanent — existing PAM modules continue working unmodified.

---

## Architectural Forward-Compatibility

Eight decisions in the current codebase that cost almost nothing now and prevent breaking
changes at v3. The data plane for v3 is being built during v0.x.

| Decision | Cost Now | v3 Payoff |
|----------|----------|-----------|
| `MatchResult` struct (not bool) from Verify | 1 struct | Adaptive thresholds, diagnostics, analytics |
| Structured event log per auth attempt | 1 tracing macro per stage | Anomaly detection, self-calibration |
| Frame quality metadata (histogram mean, entropy) | 2 fields + 10 lines | Enrollment quality scoring |
| `Matcher` trait with `CosineMatcher` default | 1 trait + 1 impl | Pluggable adaptive matching |
| SQLite: `quality_score` + `pose_label` columns | 2 columns | Multi-enrollment, weighted matching |
| Embeddings stored as variable-length BLOB | Already done | Voice and multi-modal embeddings |
| `visage discover` outputs structured JSON | Output format | Hardware compatibility classifier training data |
| Hardware quirks as TOML files, not hardcoded | Already done | Community contribution pipeline |

**Principle: Build the data plane for v3. Build the control plane for v0.x.**

The expensive part of v3 is not the AI models — it is having the right data. v0.x instruments
the pipeline and stores the telemetry. v3 consumes it.

---

## What We Will Not Build

| Anti-Pattern | Why |
|-------------|-----|
| `CameraBackend` or `BiometricPipeline` traits | No second implementation exists. Refactor is mechanical when one arrives. |
| `modality` parameter in v0.x D-Bus API | Leaks unimplemented capability. `Visage2` bus name handles it cleanly. |
| Adaptive thresholds | Must collect match-quality data first. Static threshold is debuggable. |
| Temporal enrollment drift / auto-adapt | Collect data for a year, then evaluate. Adapt vs. alert is a design choice requiring data. |
| Audio capture in `visage-hw` | Different hardware domain (ALSA/PipeWire vs V4L2). Future `visage-audio` crate. |
| LLM dependency in any core crate | The authentication path is deterministic. Always. |
| Fingerprint, iris, behavioral biometrics | Out of scope for v3 — separate evaluation required. |

---

## Versioning Contract

| Bus Name | Shipped | Scope | Compatibility |
|----------|---------|-------|---------------|
| `org.freedesktop.Visage1` | v0.1.0 | Face only | Permanent — never removed |
| `org.freedesktop.Visage2` | v3 | Face + voice + modality | Additive only |

The PAM module shipped in v0.1.0 will work with v3 without modification. It calls `Visage1::Verify`.

---

## Further Reading

| Document | What it covers |
|----------|---------------|
| [research/architecture-review-and-roadmap.md](research/architecture-review-and-roadmap.md) | Implementation details, security decisions, release gate history |
| [research/v3-vision.md](research/v3-vision.md) | Full v3 architecture — 5 dimensions with concrete implementation paths |
| [architecture.md](architecture.md) | Component overview, data flow, API surface |
| [threat-model.md](threat-model.md) | Threat model, trust boundaries, attack mitigations |
| [marketing/distribution-strategy.md](marketing/distribution-strategy.md) | Distribution priority — why packages precede the public announcement |
