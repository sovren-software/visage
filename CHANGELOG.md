# Changelog

## Unreleased

## v0.3.4 — 2026-07-07

### Fixed

- **NixOS / Nix flake build: add `openssl` to `buildInputs`** (issue #38). The
  Nix derivation failed to build because `ort` (ONNX Runtime) pulls in `ureq` →
  `native-tls` → `openssl-sys`, whose build script needs the system OpenSSL
  library at link time. `nativeBuildInputs` already provided `pkg-config`, but
  `buildInputs` was missing `openssl`, so `openssl-sys` could not locate it.
  A follow-up can drop the C TLS dependency entirely by building `ort` with
  `default-features = false, features = ["load-dynamic"]` against
  `pkgs.onnxruntime`.
- **Camera capture no longer degrades over long sessions on a shared webcam**
  (issue #48). `visaged` negotiated the V4L2 capture format only once, at
  `Camera::open`, and never re-asserted it. On a webcam shared with other
  applications (e.g. a video-conferencing app), another process could change the
  device's format via `VIDIOC_S_FMT` and leave it there; `visaged` then captured
  wrong-format frames it decoded as garbage through its stale format cache —
  surfacing as "no face detected" until a manual `systemctl restart`. The daemon
  now re-asserts its format before each capture (a cheap `VIDIOC_G_FMT`; `S_FMT`
  only fires when the device actually drifted) and, as a safety net, re-opens the
  camera in-process after repeated capture failures instead of requiring a
  restart. The per-capture stream is retained, so the camera is still released
  between verifies and remains usable by other applications.
- **AUR install hook: corrected PAM keyword `success=end` → `success=done`.**
  `packaging/aur/visage.install` still printed the setup guidance with the
  invalid `[success=end …]` action — the same bug fixed everywhere else in
  v0.3.2. libpam treats the unknown `end` as `ignore`, so a user following the
  printed line verbatim would get a silent face-auth no-op. Now prints
  `[success=done default=ignore]`.

### Security

- **CI: added a scheduled `cargo audit` workflow** (`.github/workflows/audit.yml`).
  It scans `Cargo.lock` against the RustSec advisory database weekly and on
  demand, surfacing dependency advisories without waiting for a manual check.

## v0.3.3 — 2026-05-28

### Added

- **Hardware support: Lenovo ThinkPad X1 Carbon Gen 9 20XW00FPUS IR camera** (`174f:2454`).
  Verified on hardware. Quirk file at `contrib/hw/174f-2454.toml`. Contributed by
  @themariusus in #29.

### Packaging

- **AUR `PKGBUILD` disables LTO and debug** (`options=(!lto !debug)`). LTO operates on
  LLVM IR, but `ring` ships hand-written assembly via `cc` and `libsqlite3-sys`
  compiles `sqlite3.c` via `cc` — those `.o` files have no LTO-compatible IR, so the
  final link drops or fails to resolve their symbols. Without this, `makepkg -si`
  on a stock Arch system fails at link time with `undefined symbol:
  ring_core_0_17_14__LIMBS_window5_split_window` (and many more from both `ring`
  and `libsqlite3-sys`). Reported and fixed by @SomeCodecat in #25.

### Developer experience

- **`nix develop` shell now ships `rustfmt`, `clippy`, and `libclang`.**
  `inputsFrom = [ visage ]` brought the compiler but not these auxiliaries, so
  contributors hit `error: no such command: fmt` and bindgen failed to find
  `libclang.so`. Devshell now sets `LIBCLANG_PATH` and exposes both cargo
  subcommands matching CI's `dtolnay/rust-toolchain@stable` gates. (#32)

### Dependencies

- `tokio` 1.49.0 → 1.50.0
- `nix` 0.31.1 → 0.31.2
- `uuid` 1.21.0 → 1.23.0
- `image` 0.25.9 → 0.25.10
- `actions/checkout` v4 → v6 (CI)
- `actions/upload-artifact` v4 → v7 (CI)
- `actions/download-artifact` v4 → v8 (CI)

## v0.3.2 — 2026-05-28

### Fixed

- **PAM control keyword corrected: `success=end` → `success=done` across all 9 sites.**
  `pam.conf(5)` documents exactly `ignore | bad | die | ok | done | reset | N` —
  `end` is not a valid keyword. libpam logged a warning and treated it as
  `ignore`, meaning a successful face match silently fell through to the next
  rule (typically `pam_unix.so` → password prompt) instead of terminating the
  auth stack with success. Affected: `README.md`, `docs/operations-guide.md`,
  `docs/architecture.md`, `packaging/debian/pam-auth-update` (Ubuntu),
  `packaging/nix/module.nix` (NixOS — `sudo` and `login` rules), and several
  research docs. Caught by @SelfRef in #27. **Note for existing users:** if your
  PAM stack still references the old keyword (e.g. you manually edited
  `/etc/pam.d/system-auth` on Arch from the prior README, or you're on an old
  Debian/Ubuntu install that hasn't re-run `pam-auth-update`), face auth has
  been working as if Visage weren't installed — replace `success=end` with
  `success=done` and re-test.
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
