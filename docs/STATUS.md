# Visage v0.3 Release Status

**Last updated:** 2026-02-24
**Build state:** v0.3.0 shipped and validated locally. All 6 implementation steps complete + model integrity enforcement + OSS governance. End-to-end tested on Ubuntu 24.04.4 LTS (v0.1.0 → v0.3.0 upgrade path verified 2026-02-24).

---

## Implementation (All Steps Complete)

| Step | Component | Status |
|------|-----------|--------|
| 1 | Camera pipeline (`visage-hw`) | ✅ Complete — V4L2, GREY/YUYV/Y16, CLAHE, dark frame filter |
| 2 | ONNX inference (`visage-core`) | ✅ Complete — SCRFD detection, ArcFace recognition, face alignment |
| 3 | Daemon + D-Bus + SQLite (`visaged`) | ✅ Complete — persistent daemon, 5-method API, WAL store |
| 4 | PAM module (`pam-visage`) | ✅ Complete — PAM_IGNORE fallback, system bus, FFI safe |
| 5 | IR emitter (`visage-hw`) | ✅ Complete — UVC extension unit, quirks DB, ASUS Zenbook |
| 6 | Packaging | ✅ Complete — .deb, systemd, pam-auth-update, `visage setup` |
| 7 | Model integrity (`visage-models`) | ✅ Complete — pinned SHA-256, fail-closed daemon startup, shared manifest |

---

## Acceptance Test Checklist

Tested on Ubuntu 24.04.4 LTS (CCX20, USB webcam /dev/video2, GREY format, CPU-only ONNX).
Items marked ✅ have been verified; items marked ⬜ require hardware not available on the test machine.

### Core Function

- [x] `visage enroll --label default` — captures 5 frames, confidence-weighted averaging, stores encrypted model, returns UUID
- [x] `visage verify` — matches enrolled face, exits 0 (similarity 0.97 with v0.3.0 enrollment; 0.83 with legacy plaintext enrollment)
- [ ] `visage verify` — returns exit 1 on no-match (different person or covered camera) — requires interactive test
- [ ] `visage verify` completes in <500ms (warm daemon, good IR illumination) — 1.4s on USB webcam/CPU; needs IR+GPU test
- [ ] 10 consecutive `sudo echo test` attempts: ≥9 succeed via face recognition — requires interactive test

### Safety Properties (most critical)

- [ ] Cover camera → `sudo` falls back to password within 3 seconds (PAM timeout) — requires interactive test
- [x] Kill visaged → `sudo` falls back to password within 3 seconds
- [x] Restart daemon → re-enroll not required (data persists in SQLite)
- [x] No output in terminal on PAM failure — only in `/var/log/auth.log`

### Packaging Lifecycle

- [x] `sudo apt install ./visage_*.deb` on Ubuntu 24.04 succeeds (upgrade v0.1.0 → v0.3.0 verified)
- [x] `systemctl status visaged` shows active after setup (note: `systemctl restart visaged` required after package upgrade)
- [x] `grep visage /etc/pam.d/common-auth` shows pam_visage.so entry
- [x] `sudo visage setup` downloads and verifies both ONNX models (182 MB, SHA-256)
- [x] `sudo apt remove visage` → `grep visage /etc/pam.d/common-auth` shows no entry
- [x] Password-based `sudo` works correctly after remove
- [x] `sudo apt purge visage` removes `/var/lib/visage/` directory

### Systemd Hardening

- [x] `systemctl show visaged --property=ProtectSystem` returns `strict`
- [x] `systemctl show visaged --property=NoNewPrivileges` returns `yes`
- [x] `systemctl show visaged --property=DeviceAllow` returns `char-video4linux rw`

### D-Bus Access Control

- [x] `visage enroll` as non-root user is rejected (D-Bus policy)
- [x] `visage list` as non-root user is rejected (D-Bus policy)
- [x] `visage remove` cross-user is rejected (store-level protection)
- [x] `visage verify` as non-root user succeeds (D-Bus policy allows)
- [x] `visage status` as non-root user succeeds

### v0.3.0 Upgrade Path

- [x] Package upgrade v0.1.0 → v0.3.0 via `apt install` succeeds cleanly
- [x] Legacy plaintext enrollment readable after upgrade (transparent migration path)
- [x] New encryption key generated on first v0.3.0 daemon start (old key absent)
- [x] Model integrity check passes at daemon startup (silent success)
- [x] `visage status` shows new fields: `model_dir`, `timeout`, `verify_n`, `enroll_n`, `emitter`, `bus`
- [x] `visage discover` shows kernel driver per device, VID:PID, quirk status
- [x] Re-enrollment with v0.3.0 produces encrypted embedding (AES-256-GCM)
- [x] PAM face auth works after re-enrollment (`sudo -k && sudo echo test` — similarity 0.91)

### Boot/Suspend Cycle

- [x] IR emitter activates at daemon start (no manual intervention after reboot) — daemon starts via systemd on boot
- [x] Suspend → resume → `sudo echo test` works (daemon restarted via systemd sleep hook)

---

## Bugs Found During Testing (Fixed)

1. **`DeviceAllow=/dev/video* rw`** (commit 51b5eff) — glob pattern doesn't work in systemd's
   cgroup v2 device policy. Even root is blocked. Fixed to `char-video4linux rw` (kernel device type).

2. **`tokio::time::timeout` panic** (commit 51b5eff) — zbus dispatches D-Bus method handlers on its
   own async executor, not Tokio's. `tokio::time::timeout` panics without Tokio reactor. Fixed by
   moving timeout enforcement into the engine thread via `std::time::Instant` deadline.

---

## Remaining Work (Before Public Announcement)

### Blockers

1. ~~End-to-end install test on Ubuntu 24.04~~ — **DONE** (2026-02-22, CCX20)

2. ~~GitHub Actions CI pipeline~~ — **DONE** (2026-02-22, `.github/workflows/ci.yml`)
   - fmt, clippy, build, test, cargo-deb, GitHub Release on `release:` commit prefix

3. ~~IR emitter suspend/resume hook~~ — **DONE** (systemd sleep hook restarts visaged on resume)

4. ~~ONNX model integrity verification~~ — **DONE** (v0.3.0, commit 5d001c2)
   - `visage-models` crate: pinned SHA-256, shared manifest, `verify_models_dir`
   - `visaged` fails closed at startup if models are missing or checksums mismatch
   - `visage setup` refactored to use shared manifest (no duplicated model list)
   - ADR 009 documents rationale, trade-offs, and known limitations

5. ~~OSS contribution governance~~ — **DONE** (2026-02-24)
   - `SECURITY.md`: private vulnerability reporting via GitHub Security Advisories
   - Branch protection on `main`: required PR, 1 approval, `test` status check, no force push
   - `CODEOWNERS`: `@sovren-software` owns all paths; explicit entries for security crates
   - Issue templates: bug report, hardware report, feature request + config.yml
   - PR template: type, description, testing, quality gate checklist
   - `CONTRIBUTING.md`: DCO sign-off policy, merge strategy, review timeline
   - Dependabot: weekly Cargo + GitHub Actions dependency PRs
   - LICENSE copyright corrected to Sovren Software
   - ADR 010 documents rationale, trade-offs, and known limitations

### High Priority (not blockers but ship before public announcement)

4. ~~**Rate limiting**~~ — **DONE** — 5 failures/60s sliding window → 5-min lockout

5. ~~**Hardware compatibility docs and IPU6 detection**~~ — **DONE** (commit 7d0f9e1)
   - `visage discover` now shows kernel driver per device; warns on IPU6 with explanation
   - `docs/hardware-compatibility.md` created with tier table, laptop examples, emitter process
   - README hardware section rewritten with UVC/IPU6 tier table
   - ADR 008 documents decision rationale and trade-offs

6. **NixOS packaging** — Augmentum OS overlay integration; Tier 1 in distribution strategy
   - Path: `packaging/nix/` (derivation present)
   - Blocked on: flake wiring / nixpkgs submission decisions

7. **GitHub release with pre-built `.deb`** — necessary for users without Rust toolchain

8. **Debian changelog** — required for Launchpad PPA submission; not present

### Post-v0.3 (v0.4 or v3)

- Launchpad PPA for `sudo apt install visage` (no source build required)
- AUR package for Arch Linux
- COPR for Fedora (timing: Fedora 43 dlib removal window)
- In-method D-Bus UID validation via `GetConnectionCredentials`
- Dedicated service user with udev rules (replaces root+DeviceAllow)
- `systemd-tmpfiles.d` entry for `/var/lib/visage` (replaces postinst mkdir)
- Active liveness detection (blink challenge)

---

## Known Limitations at v0.3

| Limitation | Impact | Mitigation | ADR |
|------------|--------|------------|-----|
| ~~No rate limiting~~ | ~~Unlimited face attempts~~ | **Resolved** — 5 failures/60 s → 5 min lockout; engine errors excluded | -- |
| ~~D-Bus `user` param not validated~~ | ~~Compromised process can probe any user~~ | **Resolved** — caller UID verified via GetConnectionUnixUser; root exempt; session bus skips (dev mode) | ADR 007 |
| ~~Face embeddings not encrypted~~ | ~~DB readable as root~~ | **Resolved** — AES-256-GCM at rest; per-installation key at `{db_dir}/.key` (mode 0600) | ADR 003 |
| No active liveness | High-quality IR photo could pass | Emitter + multi-frame reduces risk; impractical in practice | ADR 007 |
| `MemoryDenyWriteExecute=false` | Daemon can map W+X pages | Architectural: ONNX Runtime requires JIT; all other sandbox directives apply | ADR 007 |
| Ubuntu only | No other distributions | .deb ships; NixOS, AUR, COPR pending | ADR 007 |
| ~1.4s verify latency | Above 500ms target | Hardware-dependent: CPU-only ONNX on USB webcam; target <500 ms requires IR camera + hardware acceleration | -- |

---

## Test Coverage Summary

| Crate | Tests | What they cover |
|-------|-------|----------------|
| `pam-visage` | 5 | PAM/syslog constant values, D-Bus error handling without daemon |
| `visage-core` | 27 | Detection, alignment, recognition preprocessing, matching |
| `visage-hw` | 9 | Frame processing, CLAHE, dark frame detection, pixel conversion |
| `visage-models` | 4 | SHA-256 verification: missing file, checksum mismatch, checksum match, missing directory |
| `visaged` | 14 | Rate limiting, store roundtrip, encryption, corruption hardening |
| **Total** | **59** | **Unit tests — no integration tests; no hardware tests** |

Integration tests (camera + inference + daemon + PAM) are not present. They require physical
hardware (IR camera) and are deferred to manual acceptance testing on Ubuntu 24.04.
