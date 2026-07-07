# Visage Strategy

**Status:** Living — reflects committed direction. Updated as versions ship.
**Last reviewed:** 2026-07-07

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

## Where We Are: v0.3.6

**Shipped 2026-05-28 (v0.3.3) and 2026-07-07 (v0.3.4, v0.3.5, v0.3.6), on top of the v0.3.0 foundation.**

v0.3.0 (2026-02-23) shipped all 6 implementation steps end-to-end on Ubuntu 24.04.4 LTS.
The v0.3.x point releases since then addressed two silent ship-time bugs and added
broader hardware + packaging coverage:

- **v0.3.2 (2026-05-28)** — fixed `PAM success=end → success=done` keyword (libpam was
  silently treating the unknown keyword as `ignore` since v0.1.0, so face auth was a
  silent no-op on the documented setup paths). Closed Issue #26 — `visaged` now handles
  SIGTERM correctly, dropping the ~90s post-hibernate `systemctl restart` hang to ~10s.
- **v0.3.3 (2026-05-28)** — Lenovo X1 Carbon Gen 9 IR camera quirk (second
  Tier-1-verified hardware target after ASUS Zenbook 14 UM3406HA); AUR `!lto !debug`
  fix so `makepkg -si` succeeds on stock Arch; devshell parity with CI;
  7 dependency bumps.
- **v0.3.4 (2026-07-07)** — fixed capture degradation on shared webcams. `visaged`
  cached its V4L2 format once at open and never re-asserted it; a co-resident app (e.g. a
  video call) could leave the device in another format, so `visaged` decoded garbage →
  "no face detected" until a manual restart. Fix: per-capture format re-assert +
  in-process camera self-heal (#48). Also: NixOS flake build fix (`openssl` in
  `buildInputs`), corrected the AUR install hook's invalid `success=end` keyword, and a
  scheduled `cargo audit` workflow.
- **v0.3.5 (2026-07-07)** — IR-emitter hardware support for the HP OmniBook X Flip
  (`30c9:0120`, with a quirk-schema extension for emitters that reject an all-zero "off"
  write) and the Lenovo ThinkBook 14 MP2PQAZG (`30c9:00c2`); `openssl` + `rustls-webpki`
  security bumps (Dependabot security updates now enabled). Contribution review reframed
  problem-first (a PR is a "push request" — ADR 010 §9).
- **v0.3.6 (2026-07-07)** — security hardening batch. In-process root checks on the
  privileged D-Bus methods (`Enroll`/`RemoveModel`/`ListModels`) — defense-in-depth over
  the system-bus policy file alone; `VISAGE_SESSION_BUS=0` no longer fail-opens session-bus
  mode (which skips UID validation); passive liveness fails closed on insufficient landmark
  data; `zbus` pinned to the tokio executor (drops the `async-io`/`smol` stack, closes a
  latent reactor-panic class); AES-256-GCM known-answer + on-disk blob-format test. No
  public API or D-Bus wire-format changes.

| Component | What it delivers |
|-----------|-----------------|
| `visage-hw` | V4L2 capture, GREY/YUYV/Y16 format detection, CLAHE preprocessing, dark frame rejection, per-capture format re-assert + camera self-heal. Quirks DB covers ASUS Zenbook 14, Lenovo X1 Carbon Gen 9, Lenovo ThinkBook 14, HP OmniBook X Flip |
| `visage-core` | SCRFD face detection + ArcFace recognition via ONNX Runtime — CPU-capable, no CUDA required |
| `visaged` | Persistent daemon — holds camera and model weights across auth requests, D-Bus IPC, SQLite WAL. SIGINT + SIGTERM shutdown handlers; `TimeoutStopSec=10s` defense in depth |
| `pam-visage` | Thin PAM module — `PAM_IGNORE` fallback, never blocks, system bus. `[success=done default=ignore]` control flow (corrected v0.3.2) |
| IR emitter | UVC extension unit control, hardware quirks database |
| Packaging | `.deb` with `pam-auth-update`, AUR `!lto !debug` PKGBUILD with verified `sha256sums`, NixOS module, systemd hardening, AES-256-GCM embeddings at rest |

Visage authenticates in ~1.4s on CPU with a USB webcam. Howdy's Python subprocess cold-start
is 2-3s. Visage is already faster — without IR camera or GPU — because model weights are
loaded once at daemon start, not per attempt. That is the architectural advantage.

See [ADR 012](decisions/012-post-launch-stabilization-v0.3.2-v0.3.3.md) for the full
v0.3.x stabilization context, the rationale behind each fix, and the trade-offs
accepted.

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

### Public Launch (Summer 2026)

Coordinated announcement across r/linux, r/rust, r/privacy, Hacker News, and Phoronix.
The announcement leads with the architectural story: persistent daemon, no subprocess overhead,
encrypted embeddings at rest. The benchmark provides the concrete numbers.

| Item | Status |
|------|--------|
| AUR PKGBUILD | ✅ `packaging/aur/` |
| NixOS derivation | ✅ `packaging/nix/` — flake wiring pending |
| OSS contribution governance | ✅ Branch protection, SECURITY.md, templates, CODEOWNERS, DCO (ADR 010) |
| COPR RPM spec | ⬜ `packaging/rpm/` |
| Howdy vs Visage benchmark | ⬜ Matched hardware, published methodology |
| Active liveness detection | ⬜ Blink challenge — proof of concept |
| Enrollment quality scoring | ⬜ Reject blurry / dark / partial frames at capture |
| `visage discover --json` | ⬜ Structured output — required for v3 hardware classifier |

### v0.3 — Security-First Hardening

- Strict ONNX model integrity verification (pinned SHA-256; fail-closed at daemon startup)
- Shared model manifest crate used by both CLI and daemon

### v0.4 — Hardware Breadth

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
| [SECURITY.md](../SECURITY.md) | Vulnerability reporting policy, component risk table, response timeline |
| [CONTRIBUTING.md](../CONTRIBUTING.md) | Contribution guide — scope, PR process, DCO, merge strategy |
