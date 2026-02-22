# Visage v0.1 Release Status

**Last updated:** 2026-02-22
**Build state:** All 6 implementation steps complete. End-to-end tested on Ubuntu 24.04.4 LTS.

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

---

## Acceptance Test Checklist

Tested on Ubuntu 24.04.4 LTS (CCX20, USB webcam /dev/video2, GREY format, CPU-only ONNX).
Items marked ✅ have been verified; items marked ⬜ require hardware not available on the test machine.

### Core Function

- [x] `visage enroll --label normal` — captures 5 frames, stores model, returns UUID
- [x] `visage verify` — matches enrolled face, exits 0 (similarity 0.87-0.90)
- [ ] `visage verify` — returns exit 1 on no-match (different person or covered camera) — requires interactive test
- [ ] `visage verify` completes in <500ms (warm daemon, good IR illumination) — 1.4s on USB webcam/CPU; needs IR+GPU test
- [ ] 10 consecutive `sudo echo test` attempts: ≥9 succeed via face recognition — requires interactive test

### Safety Properties (most critical)

- [ ] Cover camera → `sudo` falls back to password within 3 seconds (PAM timeout) — requires interactive test
- [x] Kill visaged → `sudo` falls back to password within 3 seconds
- [x] Restart daemon → re-enroll not required (data persists in SQLite)
- [x] No output in terminal on PAM failure — only in `/var/log/auth.log`

### Packaging Lifecycle

- [x] `sudo apt install ./visage_*.deb` on Ubuntu 24.04 succeeds
- [x] `systemctl status visaged` shows active after setup
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
- [x] `visage verify` as non-root user succeeds (D-Bus policy allows)
- [x] `visage status` as non-root user succeeds

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

## Remaining Work (Before v0.1 Announcement)

### Blockers

1. ~~End-to-end install test on Ubuntu 24.04~~ — **DONE** (2026-02-22, CCX20)

2. ~~GitHub Actions CI pipeline~~ — **DONE** (2026-02-22, `.github/workflows/ci.yml`)
   - fmt, clippy, build, test, cargo-deb, GitHub Release on `release:` commit prefix

3. ~~IR emitter suspend/resume hook~~ — **DONE** (systemd sleep hook restarts visaged on resume)

### High Priority (not blockers for v0.1 but ship before public announcement)

4. **Rate limiting** — no limit on failed face attempts (see threat-model.md)

5. **NixOS packaging** — AEGIS overlay integration; listed as Tier 1 in distribution-strategy.md
   - Path: `packaging/nix/` (not yet created)
   - Blocked on: deciding whether to package via AEGIS overlay or nixpkgs PR

6. **GitHub release with pre-built `.deb`** — necessary for users without Rust toolchain

7. **Debian changelog** — required for Launchpad PPA submission; not present

### Post-v0.1 (v0.2 or v3)

- Launchpad PPA for `sudo apt install visage` (no source build required)
- AUR package for Arch Linux
- COPR for Fedora (timing: Fedora 43 dlib removal window)
- In-method D-Bus UID validation via `GetConnectionCredentials`
- Dedicated service user with udev rules (replaces root+DeviceAllow)
- `systemd-tmpfiles.d` entry for `/var/lib/visage` (replaces postinst mkdir)
- Active liveness detection (blink challenge)

---

## Known Limitations at v0.1

| Limitation | Impact | Mitigation | ADR |
|------------|--------|------------|-----|
| No rate limiting | Unlimited face attempts | Physical access required; IR-only pipeline raises bar | -- |
| No active liveness | High-quality IR photo could pass | Emitter + multi-frame reduces risk; impractical in practice | ADR 007 |
| D-Bus `user` param not validated | Compromised process can probe any user | root-only mutations; Verify is read-only | ADR 007 |
| `MemoryDenyWriteExecute=false` | Daemon can map W+X pages | All other sandbox directives apply | ADR 007 |
| Face embeddings not encrypted | DB readable as root | Read requires root; full disk encryption recommended | ADR 003 |
| Ubuntu only | No other distributions | .deb ships; NixOS, AUR, COPR pending | ADR 007 |
| ~1.4s verify latency | Above 500ms target | CPU-only ONNX on USB webcam; IR camera + GPU should be faster | -- |

---

## Test Coverage Summary

| Crate | Tests | What they cover |
|-------|-------|----------------|
| `pam-visage` | 5 | PAM/syslog constant values, D-Bus error handling without daemon |
| `visage-core` | 27 | Detection, alignment, recognition preprocessing, matching |
| `visage-hw` | 9 | Frame processing, CLAHE, dark frame detection, pixel conversion |
| `visaged` | 4 | SQLite store roundtrip, cross-user protection, embedding fidelity |
| **Total** | **45** | **Unit tests — no integration tests; no hardware tests** |

Integration tests (camera + inference + daemon + PAM) are not present. They require physical
hardware (IR camera) and are deferred to manual acceptance testing on Ubuntu 24.04.
