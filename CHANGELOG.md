# Changelog

## Unreleased

### Fixed

- **`visaged` now handles SIGTERM correctly.** The shutdown signal handler in
  `crates/visaged/src/main.rs` previously relied on `tokio::signal::ctrl_c()`,
  which is SIGINT-only on Unix. `systemctl stop` / `systemctl restart` (and
  `visage-resume.service` after suspend/hibernate) send SIGTERM, which the daemon
  ignored — systemd then waited the default `TimeoutStopSec=90s` before escalating
  to SIGKILL, manifesting as a ~90s hang on `systemctl restart visaged.service`
  after hibernate resume. Visaged now installs handlers for both SIGINT and SIGTERM
  via `tokio::signal::unix::signal` and shuts down cleanly. Fixes #26.
- **`visaged.service` adds `TimeoutStopSec=10s`** as defense in depth — covers the
  edge case where a v4l2 capture is mid-flight and not promptly interruptible
  (e.g. a stale camera fd after hibernate resume). Fixes #26.

### Documentation

- Added ASUS ExpertBook B3302FEA/B5302FEA hardware validation showing the built-in
  Azurewave/IMC `13d3:56ea` UVC webcam is RGB-only and not compatible with
  Visage's secure IR-backed PAM authentication path.

## v0.3.0 — 2026-02-23

### What's changed

- **Security-first model integrity** — ONNX model files are now verified via pinned SHA-256.
  `visage setup` verifies checksums on download and `visaged` verifies the model directory at
  startup (fails closed on missing/mismatched models).
- **Shared model manifest** — added `visage-models` crate containing the model list and
  verification helpers used by both the CLI and daemon.
- **OSS contribution governance** — added `SECURITY.md` (private vulnerability reporting
  via GitHub Security Advisories), branch protection on `main` (required PR + CI + review),
  `CODEOWNERS`, issue/PR templates, DCO sign-off policy, Dependabot for dependency updates,
  and documented merge strategy with review timeline commitments. See ADR 010.

## v0.2.0 — 2026-02-23

### What's changed

- **Enterprise identity compatibility** — D-Bus `Verify(user)` caller validation now resolves user IDs via NSS (LDAP/SSSD/AD compatible) instead of parsing `/etc/passwd`.
- **CLI reliability** — `visage` CLI sets a D-Bus method timeout aligned with `VISAGE_VERIFY_TIMEOUT_SECS` (default 10s) to avoid indefinite hangs.
- **Enrollment quality** — enrollment now averages embeddings across captured frames (confidence-weighted) and re-normalizes the result.
- **Store hardening** — face DB blob parsing validates size/dimension and rejects NaN/Inf safely (no panics on corrupted blobs).
- **Status output** — `Status()` JSON includes additional config fields (paths, timeouts, frame counts, emitter/session flags).

## v0.1.0 — 2026-02-23

Initial release. All six implementation steps complete and end-to-end tested on Ubuntu 24.04.4 LTS.

### What's included

- **Camera pipeline** — V4L2 capture with GREY, YUYV, and Y16 format support. CLAHE preprocessing. Dark frame detection and rejection.
- **ONNX inference** — SCRFD face detection + ArcFace recognition via ONNX Runtime. CPU-capable, no CUDA required. Models download via `visage setup` with SHA-256 verification.
- **Persistent daemon** — `visaged` holds camera and model weights across auth requests. D-Bus IPC (`org.freedesktop.Visage1`). SQLite model store with WAL mode.
- **PAM module** — `pam-visage` integrates with any PAM-based application (sudo, login, screen lock). `PAM_IGNORE` fallback — face unavailable always falls through to password. Never blocks.
- **IR emitter control** — UVC extension unit control for Windows Hello-compatible IR cameras. Hardware quirks database (TOML). ASUS Zenbook 14 UM3406HA tested and confirmed.
- **Ubuntu packaging** — `.deb` with `pam-auth-update` integration, systemd hardening (`ProtectSystem=strict`, `NoNewPrivileges=yes`), and clean install/remove/purge lifecycle.
- **Security** — AES-256-GCM embedding encryption at rest, rate limiting (5 failures/60s → 5-min lockout), D-Bus caller UID validation.

### Known limitations

- **Ubuntu 24.04 only** — NixOS, AUR, and COPR packages are in progress.
- **~1.4s verify latency** on CPU-only ONNX with USB webcam. Target <500ms requires IR camera and hardware acceleration.
- **No active liveness detection** — IR emitter and multi-frame capture reduce spoofing risk; active challenge-response (blink detection) is planned for a future release.
- **`MemoryDenyWriteExecute=false`** — required for ONNX Runtime JIT compilation. All other sandbox directives are applied.

### Installation

```bash
# Download visage_0.1.0_amd64.deb from the release assets
sudo apt install ./visage_0.1.0_amd64.deb
sudo visage setup       # downloads ONNX models (~182 MB)
visage enroll           # enroll your face
sudo echo test          # verify PAM integration
```

See [docs/hardware-compatibility.md](docs/hardware-compatibility.md) for camera compatibility tiers and IR emitter setup.

### Requirements

- Ubuntu 24.04 LTS (amd64)
- V4L2-compatible camera (UVC preferred)
- libpam0g, libdbus-1-3 (installed automatically via .deb)
