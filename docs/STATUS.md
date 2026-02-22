# Visage v0.1 Release Status

**Last updated:** 2026-02-22
**Build state:** All 6 implementation steps complete. End-to-end testing pending.

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

These are the tests that must pass before v0.1 can be announced. Each requires an Ubuntu 24.04
system with an IR camera. Items marked ✅ have been verified; ⬜ items are pending.

### Core Function

- [ ] `visage enroll --label normal` — captures 5 frames, stores model, returns UUID
- [ ] `visage verify` — matches enrolled face, exits 0
- [ ] `visage verify` — returns exit 1 on no-match (different person or covered camera)
- [ ] `visage verify` completes in <500ms (warm daemon, good IR illumination)
- [ ] 10 consecutive `sudo echo test` attempts: ≥9 succeed via face recognition

### Safety Properties (most critical)

- [ ] Cover camera → `sudo` falls back to password within 3 seconds (PAM timeout)
- [ ] Kill visaged → `sudo` falls back to password within 3 seconds
- [ ] Restart daemon → re-enroll not required (data persists in SQLite)
- [ ] No output in terminal on PAM failure — only in `/var/log/auth.log`

### Packaging Lifecycle

- [ ] `sudo apt install ./visage_*.deb` on **clean Ubuntu 24.04 VM** succeeds
- [ ] `systemctl status visaged` shows active after install
- [ ] `grep visage /etc/pam.d/common-auth` shows pam_visage.so entry
- [ ] `sudo visage setup` downloads and verifies both ONNX models
- [ ] `sudo apt remove visage` → `grep visage /etc/pam.d/common-auth` shows no entry
- [ ] Password-based `sudo` works correctly after remove
- [ ] `sudo apt purge visage` removes `/var/lib/visage/` directory

### Systemd Hardening

- [ ] `systemctl show visaged --property=ProtectSystem` returns `strict`
- [ ] `systemctl show visaged --property=NoNewPrivileges` returns `yes`
- [ ] `systemctl show visaged --property=DeviceAllow` returns `char-video rw`

### D-Bus Access Control

- [ ] `visage enroll` as non-root user is rejected (D-Bus policy)
- [ ] `visage verify` as non-root user succeeds (D-Bus policy allows)
- [ ] `visage status` as non-root user succeeds

### Boot/Suspend Cycle

- [ ] IR emitter activates at daemon start (no manual intervention after reboot)
- [ ] Suspend → resume → `sudo echo test` works (IR re-activates via sleep hook or re-open)

---

## Remaining Work (Before v0.1 Announcement)

These are blockers or near-blockers for a public release:

### Blockers

1. **End-to-end install test on Ubuntu 24.04 VM** (Acceptance Checklist above)
   - The `.deb` structure is complete but has never been tested via `apt install`
   - Risk: postinst path assumptions, service enable on non-systemd VM, pam-auth-update version differences

2. **GitHub Actions CI pipeline** — no automated build, no release assets
   - Currently requires local build: `cargo build --release --workspace && cargo deb -p visaged`
   - Users cannot install without Rust toolchain + cargo-deb

3. **IR emitter suspend/resume hook** — daemon must re-initialize emitter after suspend
   - Covered in Step 5 known limitations, not yet implemented
   - Blocks the "Suspend → resume → sudo works" acceptance test

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
| No rate limiting | Unlimited face attempts | Physical access required; IR-only pipeline raises bar | — |
| No active liveness | High-quality IR photo could pass | Emitter + multi-frame reduces risk; impractical in practice | ADR 007 |
| D-Bus `user` param not validated | Compromised process can probe any user | root-only mutations; Verify is read-only | ADR 007 |
| `MemoryDenyWriteExecute=false` | Daemon can map W+X pages | All other sandbox directives apply | ADR 007 |
| Face embeddings not encrypted | DB readable as root | Read requires root; full disk encryption recommended | ADR 003 |
| Ubuntu only | No other distributions | .deb ships; NixOS, AUR, COPR pending | ADR 007 |
| No CI | Manual build required | 6 implementation steps have 45 unit tests | — |
| End-to-end test pending | Install lifecycle unverified | Structure matches Ubuntu conventions | ADR 007 |

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
