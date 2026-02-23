# Visage Strategy — Growth Map: v2 to v3

**Status:** Locked — this document reflects committed direction, not a work in progress.
**Last reviewed:** 2026-02-23

---

## The One-Line Summary

Visage is the Windows Hello equivalent for Linux — built right, from the ground up.

---

## Ecosystem Position

Visage is the entry point to the Sovren Software privacy stack.

**Sequencing:** Visage launches publicly (v0.2, Summer 2026) 4–6 months before Augmentum OS
ships. Early adopters test hardware, contribute quirk entries, and validate the install
lifecycle. By the time Augmentum OS ships, Visage has a live user base, a hardware
compatibility matrix, and community-submitted packaging for major distros.

**Flywheel:** Users install Visage today on Ubuntu/Arch/Fedora → they see
*"the default face authentication layer for Augmentum OS"* → they anticipate the full
hardened system. Visage builds credibility and awareness for Augmentum OS without any
feature coupling.

**Boundary:** Visage is and remains **facial authentication only**. Features that belong
to the Augmentum OS desktop layer — gesture tracking, motion-based input, behavioral
biometrics — will not be merged into Visage. This boundary is documented in
[CONTRIBUTING.md](../CONTRIBUTING.md) and enforced by maintainers.

**Launch target:** v0.2, Summer 2026. Coordinated announcement across r/linux, r/rust,
r/privacy, Hacker News, and Phoronix. The announcement leads with the architectural
story (persistent daemon, no subprocess cold start) rather than a specific speed
multiplier — the multiplier will be a measured, published benchmark before any
announcement copy references it.

---

## The Problem We Are Solving

Howdy works, occasionally. Two of its three camera backends have bugs. The PAM module
spawns a Python subprocess per authentication attempt (2-3s cold start). No anti-spoofing.
Model files are world-readable. Threshold defaults produce 44% match rates.

Linux deserves a biometric authentication layer that is **reliable, secure, and maintainable**.
That is what Visage v2 is.

---

## Version Map

| Version | Identity | Delivers |
|---------|----------|----------|
| Howdy | v1 — proof of concept | Face auth on Linux, fragile |
| **Visage v0.2** | **The Foundation** | **Reliable, secure, fast face auth via persistent Rust daemon** |
| Visage v0.3 | Hardware breadth | IPU6 support, GPU inference, adaptive thresholds |
| Visage v3 | The Platform | Self-calibrating, hardware-adaptive, multi-modal, AI-assisted |

v2 must be usable and complete without any v3 capability.
v3 is a consequence of v2's design quality, not a prerequisite for v2's success.

---

## v2: The Foundation

### What v2 Delivers

- Sub-500ms face authentication via persistent daemon (no subprocess cold start)
- Reliable IR camera support: GREY + YUYV format auto-detection, hardware quirks database
- SCRFD face detection + ArcFace recognition via ONNX — CPU-capable, no CUDA required
- Privileged daemon (`visaged`) + thin PAM module — correct security boundary
- D-Bus IPC (`org.freedesktop.Visage1`) following fprintd precedent
- SQLite model store with per-user multi-enrollment
- Ubuntu 24.04 `.deb` package with `pam-auth-update` integration
- `PAM_IGNORE` fallback — face unavailable always falls through to password. Never blocks.

### v0.2 Success Criteria (Public Launch Gate)

Visage authenticates via face with >95% reliability and falls back to password cleanly
on timeout or failure. A published Howdy comparison benchmark demonstrates the improvement
concretely on matched hardware. Distribution packages (AUR, NixOS, COPR) are available
so users can install on Arch and Fedora — not just Ubuntu.

Note: Visage at ~1.4s on CPU/USB webcam already beats Howdy's 2-3s cold-start (subprocess
spawn per attempt). Sub-500ms is a v0.3 hardware-acceleration goal, not a v0.2 gate.

Full checklist: [architecture-review-and-roadmap.md](research/architecture-review-and-roadmap.md#v02-release-gate)

### v2 Build Sequence

| Step | Component | Purpose |
|------|-----------|---------|
| ✅ 1 | `visage-hw` | Camera capture — V4L2, CLAHE, dark frame filtering |
| ✅ 2 | `visage-core` | ONNX inference — SCRFD detection + ArcFace recognition |
| ✅ 3 | `visaged` | Daemon — D-Bus, SQLite, session bus, persistent model store |
| ✅ 4 | `pam-visage` | PAM module — PAM_IGNORE fallback, system bus |
| ✅ 5 | `visage-hw` | IR emitter — UVC extension unit control, quirks DB |
| ✅ 6 | Packaging | Ubuntu .deb, pam-auth-update, systemd hardening, `visage setup` |

---

## The Bridge: What v2 Must Build for v3

Eight architectural decisions in v2 that cost almost nothing now and prevent
breaking changes later. This is the data plane for v3.

| Decision | v2 Cost | v3 Benefit |
|----------|---------|------------|
| `MatchResult` struct (not bool) from Verify | 1 struct | Adaptive thresholds, diagnostics, analytics |
| Structured event log per auth attempt | 1 tracing macro per stage | Anomaly detection, self-calibration, LLM diagnostics |
| Frame quality metadata (histogram mean, entropy) | 2 fields + 10 lines | Enrollment quality scoring, environmental adaptation |
| `Matcher` trait with `CosineMatcher` default | 1 trait + 1 impl | Pluggable adaptive matching |
| SQLite: `quality_score` + `pose_label` columns | 2 columns | Multi-enrollment, weighted matching |
| Embeddings stored as variable-length BLOB | Already done | Voice and multi-modal embeddings |
| `visage discover` outputs structured JSON | Output format | Hardware compatibility classifier training data |
| Hardware quirks as TOML files, not hardcoded | Already done | Community contribution pipeline |

**Principle: Build the data plane for v3. Build the control plane for v2.**

The expensive part of v3 is not the AI models — it is having the right data to train
and evaluate them. v2 instruments the pipeline and stores the telemetry. v3 consumes it.

---

## v3: The Platform

v3 transforms Visage from a reliable face auth tool into an intelligent biometric platform.
Five capability areas define v3:

### 1. Near-Zero-Config Hardware Support

**v2:** Hardware quirks database + manual discovery. One camera at a time.

**v3:** UVC descriptor-guided probing (reads what extension units the device advertises
before probing, reducing search space from ~1000 to ~15 safe attempts). Camera capability
fingerprinting at first boot. Automatic CLAHE parameter tuning per camera model.
Community quirk repository with structured submission and validation pipeline.

**Gating requirement:** `visage discover` must output structured JSON in v2.

### 2. Environmental Adaptation

**v2:** Static per-user threshold (default 0.5 cosine similarity). Static CLAHE parameters.

**v3:** Per-user adaptive threshold derived from enrollment quality and match history.
Multi-enrollment with quality scoring (front-facing vs. angled, good vs. poor lighting).
Failed-match learning loop: accumulates negative examples to tighten thresholds for
attack patterns while relaxing for normal environmental variation.

**Gating requirement:** `MatchResult` struct + quality metadata + structured event log in v2.

### 3. AI-Assisted System Intelligence

**v2:** Deterministic pipeline only. No learned models outside face detection/recognition.

**v3:**
- Hardware compatibility classifier (ONNX, tiny model): predicts whether a camera will
  work for face auth from UVC descriptor + capability fingerprint. Trained on community data.
- Enrollment quality model: rates captures before committing to SQLite. Rejects blurry,
  dark, partial, or multi-face frames.
- Anomaly detection: flags abnormal match patterns (e.g., too-consistent similarity
  scores suggesting replay attack).

**Gating requirement:** Structured event log + fingerprint data collected in v2.

### 4. LLM-Assisted Lifecycle Management

**v2:** CLI diagnostic output. Static documentation.

**v3:** `visage assistant` — a natural-language interface for setup, calibration, and
diagnostics. Uses an LLM (local or cloud, user's choice) for:
- Guided IR emitter discovery ("try this control byte sequence")
- Authentication failure diagnosis ("your match confidence dropped after you changed
  glasses — try re-enrolling")
- Quirk submission assistance ("your camera produced these descriptor tables — submit
  to the community database")

**Hard constraint:** No LLM dependency in any core crate (`visage-hw`, `visage-core`,
`visaged`, `pam-visage`). LLM lives in a separate `visage-assistant` binary.
The authentication path is and will remain deterministic.

### 5. Multi-Modal Biometric Platform

**v2:** Face only. `org.freedesktop.Visage1` D-Bus API.

**v3:** Face + voice. `org.freedesktop.Visage2` D-Bus API.

Three fusion modes:
- **Parallel:** Both modalities authenticate concurrently. Pass if either succeeds (OR gate).
  Best for convenience.
- **Sequential:** Face authenticates first. If confidence is marginal, voice is invoked.
  Best for accuracy.
- **Continuous:** Face unlocks the session. Voice commands within the session. Best for
  hands-free workflow integration.

`Visage1` bus name remains — it maps to `Verify(user, "face")`. Old PAM modules and
tools continue working unchanged.

**Gating requirement:** Pipeline stages as separable functions (not monolithic) in v2.
D-Bus method signatures must use named parameters.

---

## What v2 Will NOT Build

These are premature — they add complexity without near-term benefit:

| Anti-Pattern | Why Not |
|-------------|---------|
| `CameraBackend` or `BiometricPipeline` traits | No second implementation exists. Refactor is mechanical when one arrives. |
| `modality` parameter in v2 D-Bus API | Leaks unimplemented capability. `Visage2` bus name handles it cleanly. |
| Adaptive thresholds | Must collect match-quality data first. Static threshold is debuggable. |
| Temporal enrollment drift / auto-adapt | Collect data for a year, evaluate then. Adapt vs. alert is a design choice requiring data. |
| Audio capture in `visage-hw` | Different hardware domain (ALSA/PipeWire vs V4L2). Future `visage-audio` crate. |
| LLM dependency in any core crate | Core is deterministic. Always. |
| Fingerprint, iris, or behavioral biometrics | Out of scope for v3 — separate evaluation required. |

---

## Versioning Contract

| Bus Name | Version | Scope | Backward-Compatible |
|----------|---------|-------|---------------------|
| `org.freedesktop.Visage1` | v2 | Face only | Permanent — never removed |
| `org.freedesktop.Visage2` | v3 | Face + voice + modality | Additive only |

The PAM module shipped in v2 will work with v3 without modification. It calls `Visage1::Verify`.

---

## Further Reading

| Document | What it covers |
|----------|---------------|
| [architecture-review-and-roadmap.md](research/architecture-review-and-roadmap.md) | v2 step-by-step implementation details, security lessons, v0.1 release gate |
| [v3-vision.md](research/v3-vision.md) | Full v3 architecture analysis — 5 dimensions with concrete implementation paths |
| [architecture.md](architecture.md) | Component overview, data flow, API surface |
| [threat-model.md](threat-model.md) | Threat model, trust boundaries, attack mitigations |
| [decisions/001-camera-capture-pipeline.md](decisions/001-camera-capture-pipeline.md) | ADR: Step 1 — camera pipeline decisions |
| [decisions/002-onnx-inference-kb-and-blocker-resolution.md](decisions/002-onnx-inference-kb-and-blocker-resolution.md) | ADR: Step 2 — inference KB and blocker resolution |
| [decisions/003-daemon-integration.md](decisions/003-daemon-integration.md) | ADR: Step 3 — daemon architecture decisions |
| [decisions/004-inference-pipeline-implementation.md](decisions/004-inference-pipeline-implementation.md) | ADR: Step 4 — ONNX pipeline implementation details |
| [decisions/005-pam-system-bus-migration.md](decisions/005-pam-system-bus-migration.md) | ADR: Step 4 — PAM module and system bus migration |
| [decisions/006-ir-emitter-integration.md](decisions/006-ir-emitter-integration.md) | ADR: Step 5 — IR emitter integration |
| [decisions/007-ubuntu-packaging.md](decisions/007-ubuntu-packaging.md) | ADR: Step 6 — Ubuntu packaging and system integration |
| [marketing/distribution-strategy.md](marketing/distribution-strategy.md) | Distribution priority — why packages precede features |
