# Visage Release Status

**Last updated:** 2026-11-13
**Build state:** **v0.5.1 released 2026-11-13.** It adds `visage stats` (#159): the share of
login attempts that unlock, why the others fail, by hour of day, and the time to unlock,
read from one `visage-auth-outcome v1` line the daemon now logs per `Verify`.

**v0.5.0 shipped 2026-11-11** (GitHub release with the `.deb`). It clears
eight RustSec advisories, makes `/var/lib/visage` root-only on every package, turns
`pam_visage`'s `timeout=N` into a total budget with retries inside it, reads frames as they
arrive on one camera stream, and adds `visage-enroll`, optional OpenVINO inference, a
face-wait window and multi-control IR emitter quirks. MSRV 1.89. The reference machine
(ASUS Zenbook 14 UM3406HA, Shinetech `3277:0055`) runs v0.5.0 since 2026-11-11.

**Unreleased:** nothing yet. See [`../CHANGELOG.md`](../CHANGELOG.md).

📏 **Measured reliability baseline (before `visage stats`, read by hand).** Four weeks of
journal on the reference machine under 0.4.0 (2026-10-16 to 11-11): 95 verifies, 59
matched, 1 face that did not match, 18 failed on dark frames only and 17 found no face.
That is **about 62% success**, against the >95% goal in the
[Howdy analysis](research/howdy-analysis-and-visage-design.md). Failures ran at 50%
before 13:00 and 24% after. One candidate cause is #124: in daylight, the unlit half of
the IR strobe passes the dark-frame test. Under 0.5.0, 7 of 7 verifies matched by
2026-11-13. That is too few to conclude anything, which is what `visage stats` is for.

✅ **Hardware-validated 2026-09-16** on the-first (Shinetech `3277:0055`): 4 of 4 angles
enrolled through `visage-enroll`, best-face confidence 0.8657, the preview legible at the
160 px cap, and `require_root_caller` exercised on the system bus rather than skipped by
session-bus mode. This is the first end-to-end confirmation that the preview works on a
real IR sensor, not only in unit tests.

⛔ The `visage-gui` egui prototype was **removed**, not parked. It existed to decide
between a graphical and a terminal front-end; the terminal won on a structural argument,
not taste — `Enroll` is root-only, a GUI under `sudo` fights Wayland/X authorization and
`XDG_RUNTIME_DIR`, and a TUI under `sudo` is ordinary. Deleting it also dropped 199
packages from the lockfile.

**Prior state (v0.3.6):** All 6 implementation steps complete + model integrity enforcement + OSS governance + passive liveness detection. Since v0.3.3: fixed capture degradation on shared webcams (per-capture V4L2 format re-assert + in-process camera self-heal, #48); the IR emitter quirks DB now covers ASUS Zenbook 14 UM3406HA, Lenovo ThinkPad X1 Carbon Gen 9, Lenovo ThinkBook 14 MP2PQAZG, and HP OmniBook X Flip; NixOS flake build fixed; Dependabot security updates + a scheduled `cargo audit` enabled; contribution review reframed problem-first (ADR 010 §9). v0.3.6 added a security hardening batch: in-process root checks on the privileged D-Bus methods (`Enroll`/`RemoveModel`/`ListModels`), the `VISAGE_SESSION_BUS` flag and passive liveness now fail closed, `zbus` pinned to the tokio executor (drops the `async-io` stack), and an AES-256-GCM known-answer + blob-format test. End-to-end tested on Ubuntu 24.04.4 LTS.

⚠️ **Passive liveness had its first hardware spoof validation on 2026-08-17, and it did not pass.** On an ASUS Zenbook 14 UM3406HA with a Shinetech `3277:0055` IR module, a hand-held phone-screen spoof produced landmark displacement (**0.681 px**) *higher* than two genuine live attempts (**0.263**, 0.670), so no threshold separates them — and the identity stage matched that photo at **0.9013**. Live users were falsely rejected **13–17%** of the time at the default 0.8 px floor (20/23–20/24 live pass as of 2026-08-17 13:43 local), so the ≥ 9-in-10 reliability criterion is **not met**. Sample is n=1 on the spoof side, so `threat-model.md`'s claims are **not** yet revised. **Resolved on that host by setting `liveness.minDisplacement = 0.1`** — 15 attempts, 12 pass, 0 liveness rejections; both `sudo` and lock-screen face auth then verified end to end by the operator. See [`liveness-remaining-work.md`](liveness-remaining-work.md) and the [hardware report](hardware-reports/asus-zenbook-um3406ha-3277-0055.md).

---

## Implementation (All Steps Complete)

| Step | Component | Status |
|------|-----------|--------|
| 1 | Camera pipeline (`visage-hw`) | ✅ Complete — V4L2, GREY/YUYV/Y16, CLAHE, dark frame filter |
| 2 | ONNX inference (`visage-core`) | ✅ Complete — SCRFD detection, ArcFace recognition, face alignment |
| 3 | Daemon + D-Bus + SQLite (`visaged`) | ✅ Complete — persistent daemon, 5-method API, WAL store |
| 4 | PAM module (`pam-visage`) | ✅ Complete — PAM_IGNORE fallback, system bus, FFI safe |
| 5 | IR emitter (`visage-hw`) | ✅ Complete — UVC extension unit, quirks DB (ASUS Zenbook 14, Lenovo X1 Carbon Gen 9) |
| 6 | Packaging | ✅ Complete — .deb, systemd, pam-auth-update, `visage setup` |
| 7 | Model integrity (`visage-models`) | ✅ Complete — pinned SHA-256, fail-closed daemon startup, shared manifest |
| 8 | Passive liveness (`visage-core`, `visaged`) | ❌ **Hardware-validated 2026-08-17 and did not discriminate** — a hand-held screen spoof displaced *more* than a live face on `3277:0055`. Code works as specified; the metric does not separate live from spoof on that module. Do not lower the threshold. |

---

## Acceptance Test Checklist

Tested on Ubuntu 24.04.4 LTS (CCX20, USB webcam /dev/video2, GREY format, CPU-only ONNX).
Items marked ✅ have been verified; items marked ⬜ require hardware not available on the test machine.

### Core Function

- [x] `visage enroll --label default` — captures 5 frames, confidence-weighted averaging, stores encrypted model, returns UUID
- [x] `visage verify` — matches enrolled face, exits 0 (similarity 0.97 with v0.3.0 enrollment; 0.83 with legacy plaintext enrollment)
- [ ] `visage verify` — returns exit 1 on no-match (different person or covered camera). **Partially evidenced 2026-08-17:** PAM logged `no match for user 'cc'` and fell through to `pam_unix` with `res=success`. The CLI's own exit code was not separately recorded, so this stays open.
- [ ] `visage verify` completes in <500ms (warm daemon, good IR illumination). **Not met.** 1.4s on USB webcam/CPU; on the `3277:0055` IR module, median **2273 ms** / max **2329 ms** over 10 runs. Still needs an IR+GPU path to be plausible.
- [ ] 10 consecutive `sudo echo test` attempts: ≥9 succeed via face recognition. **Exercised on `3277:0055` 2026-08-17; left open deliberately.** At the default 0.8 liveness floor: **20/23–20/24 = 83–87%**, below the bar. After lowering `liveness.minDisplacement` to 0.1: 12/15 (the 3 non-passes were `no face detected`, nobody in front of the camera), then 8/8, then **10/10 closure-only at gen 10 with 0 liveness rejections**. Identity matching never failed across 12 attempts (0.4609–0.9847 against a 0.40 threshold).

  This stays unchecked not because the count was missed but because of what the numbers are: every figure is **single-operator, single-session, uncontrolled posture**, and the pre-fix rate swung **82% → 100% → back with no variable held fixed**. The session ADR states it plainly — *"treat every point estimate here as indicative, not measured."* Ticking this needs a controlled run, not a higher number.

  ⚠️ **There are two different `10/10`s in this corpus and they are not the same measurement.** The one in the [hardware report](hardware-reports/asus-zenbook-um3406ha-3277-0055.md) is **Howdy 2.6.1 on Ubuntu**, prior art on the same laptop. Visage's own is the **gen-10 closure-only** figure in the session ADR. Session notes shortened both to "10/10", so check which one a citation means before repeating it.

### Safety Properties (most critical)

- [ ] Cover camera → `sudo` falls back to password within 3 seconds (PAM timeout). **Fallback demonstrated 2026-08-17** (`no match` → `grantors=pam_unix`, `res=success`), but the 3-second bound was never separately timed. Note the PAM D-Bus timeout is now configurable and was raised to **6s** on that host, because measured verify latency left under 700 ms of headroom against the old 3s default.
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
- Active liveness detection (blink challenge — complements passive liveness, ADR 011)

---

## Known Limitations at v0.3

| Limitation | Impact | Mitigation | ADR |
|------------|--------|------------|-----|
| ~~No rate limiting~~ | ~~Unlimited face attempts~~ | **Resolved** — 5 failures/60 s → 5 min lockout; engine errors excluded | -- |
| ~~D-Bus `user` param not validated~~ | ~~Compromised process can probe any user~~ | **Resolved** — caller UID verified via GetConnectionUnixUser; root exempt; session bus skips (dev mode) | ADR 007 |
| ~~Face embeddings not encrypted~~ | ~~DB readable as root~~ | **Resolved** — AES-256-GCM at rest; per-installation key at `{db_dir}/.key` (mode 0600) | ADR 003 |
| ~~No liveness detection~~ | ~~High-quality IR photo could pass~~ | **Partially resolved** — passive landmark stability blocks static photos; video replay still possible | ADR 011 |
| No active liveness | Video replay of enrolled user passes passive check | Active blink/head-turn challenge required — deferred to v0.4 | ADR 011 |
| `MemoryDenyWriteExecute=false` | Daemon can map W+X pages | Architectural: ONNX Runtime requires JIT; all other sandbox directives apply | ADR 007 |
| Ubuntu only | No other distributions | .deb ships; NixOS, AUR, COPR pending | ADR 007 |
| ~1.4s verify latency | Above 500ms target | Hardware-dependent: CPU-only ONNX on USB webcam; target <500 ms requires IR camera + hardware acceleration | -- |

---

## Test Coverage Summary

### Unit tests

| Crate | Tests | What they cover |
|-------|-------|----------------|
| `pam-visage` | 8 | PAM/syslog constant values, timeout argument parsing, D-Bus error handling without daemon |
| `visage-core` | 38 | Detection, alignment, recognition preprocessing, matching, liveness landmark stability |
| `visage-hw` | 13 | Frame processing, CLAHE, dark frame detection, pixel conversion, hardware quirk DB |
| `visage-models` | 4 | SHA-256 verification: missing file, checksum mismatch, checksum match, missing directory |
| `visaged` | 18 | Rate limiting, store roundtrip, encryption, corruption hardening, session-bus config |
| **Subtotal** | **81** | |

### Integration tests

Added 2026-08-24, in `crates/visaged/tests/`. These exist because **both of the worst bugs in
this project's history were structurally invisible to unit tests** — `success=end` (an invalid
PAM keyword libpam silently treated as `ignore`, making face auth a no-op from v0.1.0 to
v0.3.2) and the V4L2 format cache (#48). Neither was a logic error in a function; both were
contracts *between artifacts*.

| Test | Tests | What it guards |
|------|-------|----------------|
| `pam_control_contract` | 3 | The PAM control string is a valid libpam control, and identical across the Debian, Arch and NixOS packaging. Directly targets the `success=end` class. |
| `systemd_hardening_contract` | 2 | Every hardening directive `threat-model.md` claims exists in **both** systemd definitions. Targets the #78 class — a documented control nothing enforced. |
| `packaging_paths_contract` | 4 | Deb assets name real build targets, shipped source files exist, `ExecStart` matches the install path, PAM module path agrees across formats. |
| `dbus_contract` | 3 | The interface the daemon serves is the one its three independently-compiled clients call. |
| **Subtotal** | **12** | **Hermetic — no hardware, no root, no D-Bus, no models, no new dependencies** |

**Total: 93 running tests**, plus 3 ignored.

### Hardware tests

`daemon_lifecycle` (3 tests, `#[ignore]`) launches the real daemon on a private session bus and
drives it over D-Bus — bus-name registration, `Status`, `Verify` on an unenrolled user, and the
property that matters most for PAM: that a client call **fails promptly rather than hanging**
once the daemon is gone.

`spawn_engine` opens the camera before serving D-Bus, so there is no headless path. Run on a
machine with an IR camera:

```bash
cargo test -p visaged --test daemon_lifecycle -- --ignored --nocapture
# VISAGE_TEST_CAMERA   (default /dev/video2)
# VISAGE_TEST_MODEL_DIR (default /var/lib/visage/models)
```

⚠️ Stop a running production `visaged` first — it holds the capture device, so the test daemon
cannot open it while that is live.

Still absent: an end-to-end **recognition** test (enrol a face, verify it matches), which needs a
face in front of the camera and is inherently interactive; and a NixOS VM test exercising the
real PAM stack via `pamtester`.
