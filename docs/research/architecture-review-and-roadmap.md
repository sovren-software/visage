# Visage Architecture Review and Implementation Roadmap

**Date:** 2026-02-21
**Status:** Living document
**Purpose:** Synthesizes external architecture review findings against our goals and constraints.
         Serves as the actionable reference for what to build, in what order, and why.

---

## Context and Objectives

**Goals:**
- Persistent Rust daemon (visaged) + thin PAM client + explicit hardware and inference layers
- SCRFD + ArcFace via ONNX Runtime — modern, CPU-capable, distribution-agnostic
- IR emitter control absorbed into daemon (no external dependency)
- D-Bus IPC following the fprintd precedent
- Ubuntu first, NixOS second, then Arch/Fedora

**Constraints:**
- Solo developer initially
- Testing on available hardware: ASUS Zenbook 14 UM3406HA (AMD Ryzen AI, /dev/video2 IR camera)
- MIT license, community-first project

**Success criteria (v0.1):**
`sudo echo test` authenticates via face in <500ms with >95% reliability,
falls back to password cleanly on timeout or failure, and can be installed/removed
on Ubuntu 24.04 without breaking authentication.

---

## What the External Review Confirmed

The architecture direction is sound:
- Privileged daemon + thin PAM client is the correct security boundary
- Component split (visaged / pam-visage / visage-core / visage-hw / visage-cli) maps cleanly to responsibilities
- SCRFD + ArcFace via ONNX is directionally stronger than Howdy's dlib pipeline

The current repo is correctly at "skeleton" stage — the design is right, implementation is pending.

---

## Applicable Lessons (Prioritized)

### HIGH PRIORITY

#### 1. D-Bus Security Policy

**Why:** Without caller authorization, any local process can call `Verify("root")` — a
confused-deputy privilege escalation attack.

**Implementation:**
- Ship `/etc/dbus-1/system.d/org.freedesktop.Visage1.conf`
- Use D-Bus peer credentials to verify caller UID matches the `user` parameter
- `Verify` callable by root (PAM always runs as root)
- `Enroll`, `RemoveModel`, `Calibrate` restricted to root
- `ListModels`, `Status` callable by matching user or root

```xml
<!-- /etc/dbus-1/system.d/org.freedesktop.Visage1.conf -->
<busconfig>
  <policy user="root">
    <allow own="org.freedesktop.Visage1"/>
    <allow send_destination="org.freedesktop.Visage1"/>
  </policy>
  <policy context="default">
    <allow send_destination="org.freedesktop.Visage1"
           send_interface="org.freedesktop.Visage1"
           send_member="Verify"/>
    <allow send_destination="org.freedesktop.Visage1"
           send_interface="org.freedesktop.Visage1"
           send_member="Status"/>
  </policy>
</busconfig>
```

#### 2. System Service with Systemd Hardening

**Why:** The daemon accesses camera hardware and stores embeddings. Minimal privilege
reduces blast radius. Distro maintainers require hardened units.

**Must be system service (not per-user):** PAM modules for `sudo`/`login` run before
any user session exists. A `systemd --user` service would be unavailable.

```ini
[Service]
User=root
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
DeviceAllow=/dev/video* rw
ReadWritePaths=/var/lib/visage /run/visage
CapabilityBoundingSet=
```

#### 3. End-to-End MVP Before Features

**Why:** Security policy, liveness detection, and packaging are meaningless until
Enroll → Store → Verify → Match works end-to-end.

**Order:** Camera capture → ONNX inference → daemon Enroll/Verify → CLI → PAM module.
Each milestone is independently testable.

#### 4. Strict PAM Timeout Enforcement

**Why:** A hanging PAM module blocks login, sudo, and screen unlock. Most destructive
possible failure mode. Non-negotiable.

**Implementation:**
- Hard 3-second wall-clock timeout on D-Bus `Verify` call in `pam_visage.so`
- If daemon doesn't respond: return `PAM_IGNORE` immediately (never `PAM_AUTH_ERR`)
- `PAM_IGNORE` → PAM stack continues to next module (password prompt)
- `tokio::time::timeout` on daemon side to bound inference

### MEDIUM PRIORITY

#### 5. Model Storage Design (Decide Before Implementing Enroll)

**Why:** The storage format affects Enroll, Verify, ListModels, and RemoveModel APIs.
Changing it after implementation means schema migrations and rewriting callers.

**Decision: SQLite**

```
/var/lib/visage/models.db  (permissions: 0600 root:root)
```

Schema:
```sql
CREATE TABLE models (
    model_id    TEXT PRIMARY KEY,
    user        TEXT NOT NULL,
    label       TEXT NOT NULL,
    created_at  INTEGER NOT NULL,  -- Unix timestamp
    embedding   BLOB NOT NULL      -- 512 x f32, little-endian
);
CREATE INDEX idx_user ON models(user);
```

Rationale over JSON files:
- Atomic writes (no corruption if killed mid-write)
- Concurrent access (WAL mode)
- Structured queries for list/remove
- Single file, root-only permissions

#### 6. D-Bus API Versioning

**Why:** Once any distro ships a package, the API is a compatibility contract.
A version mismatch between daemon and PAM module causes silent auth failures.

**Rules:**
- Bus name includes major version: `org.freedesktop.Visage1` (current)
- Expose `Version` property (semver string)
- Methods may gain optional parameters (backward compatible)
- Method signatures never change — add new methods instead
- PAM module must check `Version` property and fail gracefully if incompatible

#### 7. Hardware Quirks Contribution Process

**Why:** Visage is only useful on cameras it supports. Community contributions
of UVC control bytes are the adoption growth path.

**Implementation:**
- TOML files in `contrib/hw/{vendor_id}-{product_id}.toml`
- `visage discover` CLI command probes UVC extension units and generates a template
- README documents the contribution process clearly

---

## Observations Disregarded (and Why)

| Suggestion | Reason to Skip |
|------------|---------------|
| Split daemon into privileged broker + unprivileged inference worker | Over-engineering for v1.0. systemd hardening provides equivalent protection. Revisit when CVE risk in ort is demonstrated. |
| libcamera support | Windows Hello IR cameras are UVC devices, accessible via V4L2. libcamera targets MIPI/ISP pipelines (RPi, embedded). Add when a user submits a device that needs it. |
| Multi-factor orchestration and risk tiers | Enterprise PAM policy territory. PAM stack already handles this — `[success=end default=ignore]` is the orchestration. Not Visage's job. |
| Polkit integration | Desktop integration milestone 5+. Core PAM → D-Bus → daemon path works without it. |
| ONNX Runtime distribution strategy | The `ort` crate handles this (download-on-build or system library detection). Packaging concern, not architecture. Address during packaging milestone. |

---

## Implementation Roadmap

### Step 1: Camera Capture + Frame Pipeline (`visage-hw`) ✅ COMPLETE

**Implemented:** 2026-02-21 · Commit `678dda1`

**What was built:**
- V4L2 frame capture via `v4l = "0.14"` crate (safe ioctl wrapper)
- Open device, format negotiation — accepts YUYV and GREY pixel formats
- YUYV→grayscale conversion (Y-channel extraction), GREY passthrough
- 8-bucket histogram dark frame filter (>95% pixels in bucket 0 → dark)
- CLAHE contrast enhancement (~90 lines, implemented from scratch)
- `Frame` struct: `data`, `width`, `height`, `timestamp`, `sequence`, `is_dark`
- `visage test --device --frames` CLI command with device enumeration and PGM output

**Key implementation change from plan:** Used `v4l = "0.14"` rather than raw `nix`
ioctls. Safe abstraction, no loss of control — UVC extension unit ioctls for Step 5
will be added directly via `AsRawFd`/`nix`. See ADR 001.

**Hardware discovery:** `/dev/video2` outputs native GREY (not YUYV). Both formats
now supported. GREY eliminates conversion overhead.

**Test results:** 9/9 unit tests pass. Live capture: `/dev/video2` captures ~1 good
frame per 30 attempts (expected — no IR emitter yet). `/dev/video0` exercises YUYV path.

**ADR:** [docs/decisions/001-camera-capture-pipeline.md](../decisions/001-camera-capture-pipeline.md)

---

### Step 2: ONNX Inference Pipeline (`visage-core`)

**What to build:**
- `FaceDetector` backed by SCRFD-500M ONNX model (source: InsightFace model zoo)
- `FaceRecognizer` backed by ArcFace-R50 ONNX model
- Detection: input grayscale frame, output bounding boxes + 5 landmarks
- Alignment: affine transform using landmarks (normalize face to 112x112)
- Embedding: input aligned face crop, output 512-D normalized f32 vector
- Cosine similarity comparison between embeddings

**Model sources:**
- SCRFD: `buffalo_l/det_10g.onnx` from InsightFace (scrfd_10g_bnkps.onnx)
- ArcFace: `buffalo_l/w600k_r50.onnx` from InsightFace

**Model download:** `visage-models` CLI subcommand downloads to `/var/lib/visage/models/`
on first run. Not bundled in binary.

**Key test:** Load models. Run detect on test image with known face. Embed. Verify
512-D output. Measure: SCRFD <15ms, ArcFace <15ms, total <30ms per frame.

---

### Step 3: Daemon + D-Bus + Model Store (`visaged`)

**What to build:**
- Register `org.freedesktop.Visage1` on system D-Bus
- Implement all 5 methods with SQLite backing
- Hold camera in standby (pre-warmed) or open on demand (configurable)
- D-Bus policy file
- systemd unit with hardening settings
- `tmpfiles.d` entry for `/var/lib/visage`

**Enroll flow:**
1. Open camera (or use pre-warmed handle)
2. Capture up to 30 frames
3. Skip dark frames
4. Run SCRFD — require exactly 1 face
5. Extract ArcFace embedding
6. Write to SQLite with metadata

**Verify flow:**
1. Open camera (or use pre-warmed handle)
2. Load all embeddings for user from SQLite
3. Capture frames, run detect + embed
4. Cosine similarity against all stored embeddings
5. If max similarity > threshold (default 0.5): return (true, similarity, model_id)
6. On timeout (3s): return (false, 0.0, "")

**Key test:** `visage enroll --label test` then `visage verify`. List and remove work.
`Status` returns JSON with camera and model counts.

---

### Step 4: PAM Module (`pam-visage`)

**What to build:**
- Real `pam_sm_authenticate` via minimal C FFI (not `pam-rs` — fewer dependencies
  in the PAM path)
- Synchronous D-Bus call to `Verify(username)` with 3-second timeout
- Return `PAM_SUCCESS` on match, `PAM_IGNORE` on all failure/timeout cases
- Concurrent password + face race (optional — implement after basic flow works)

**PAM return code contract:**
- `PAM_SUCCESS` → face matched, auth granted
- `PAM_IGNORE` → face not matched or daemon unavailable, fall through to next module
- Never return `PAM_AUTH_ERR` — this would block password fallback on some PAM configs

**Key test:** Enable in `/etc/pam.d/sudo`. `sudo echo test`:
- Face in frame: succeeds in <3s
- Cover camera: password prompt appears within 3s
- Daemon not running: password prompt appears within 3s
- No hang under any condition

---

### Step 5: IR Emitter Integration (`visage-hw`)

**What to build:**
- UVC extension unit control via `UVCIOC_CTRL_SET` ioctl
- Load `contrib/hw/*.toml` files at daemon startup
- Auto-detect camera by reading USB VID:PID from `/sys/class/video4linux/video*/device/`
- Apply matching quirk on camera open
- `visage discover` CLI: iterate UVC extension units 1-20, selector 1-50, print
  which combinations produce a response (helps identify new camera control bytes)

**Known quirk to ship (Zenbook 14 UM3406HA):**
```toml
# contrib/hw/13d3-56d0.toml  (VID:PID to be confirmed)
vendor_id = 0x13D3   # placeholder — verify with lsusb
product_id = 0x56D0  # placeholder — verify with lsusb
name = "ASUS Zenbook 14 UM3406HA IR Camera"
device = "/dev/video2"

[emitter]
unit = 14
selector = 6
control = [1, 3, 3, 0, 0, 0, 0, 0, 0]
```

**Key test:** Reboot. Verify IR emitter activates at daemon start without manual
`linux-enable-ir-emitter run`. `sudo echo test` works immediately after boot.

---

### Step 6: Ubuntu Packaging (Complete — 2026-02-22)

**What was built:**
- `.deb` package via `cargo-deb` — configured in `crates/visaged/Cargo.toml`
- `packaging/systemd/visaged.service` — hardened unit (ProtectSystem=strict, DeviceAllow,
  CapabilityBoundingSet empty, MemoryDenyWriteExecute=false for ONNX Runtime JIT)
- `packaging/debian/pam-auth-update` — pam-configs profile, priority 900, `[success=end default=ignore]`
- `packaging/debian/postinst` — creates `/var/lib/visage`, runs `pam-auth-update --package`, enables service
- `packaging/debian/prerm` — stops service, runs `pam-auth-update --remove`
- `packaging/debian/postrm` — purges `/var/lib/visage` on `apt purge`
- `packaging/dbus/org.freedesktop.Visage1.conf` — restricts Enroll/RemoveModel/ListModels to root
- `visage setup` CLI subcommand — downloads ONNX models (~182MB) with SHA-256 verification;
  writes to `/var/lib/visage/models` (root) or XDG data dir (user)
- PAM module hardening: 3-second call timeout, syslog at LOG_AUTHPRIV, PAM_TEXT_INFO conversation

**Design decision change from plan:** Model download is `visage setup` (on-demand) rather than
postinst download. This makes offline installs work and gives users control over when 182MB
downloads happen.

**Key test:** `sudo apt install ./visage_*.deb` on clean Ubuntu 24.04 VM.
Verify `sudo echo test` authenticates via face.
`sudo apt remove visage` → password-only login still works.

**Status:** `.deb` structure complete; end-to-end install test on Ubuntu 24.04 pending.
See ADR 007 for full decisions, trade-offs, and remaining work.

---

## Implementation Progress

| Step | Status | Date |
|------|--------|------|
| 1 — Camera capture pipeline | ✅ Complete | 2026-02-21 |
| 2 — ONNX inference (SCRFD + ArcFace) | ✅ Complete | 2026-02-21 |
| 3 — Daemon + D-Bus + SQLite | ✅ Complete | 2026-02-22 |
| 4 — PAM module | ✅ Complete | 2026-02-22 |
| 5 — IR emitter integration | ✅ Complete | 2026-02-22 |
| 6 — Ubuntu packaging | ✅ Complete | 2026-02-22 |

---

## v0.1 Release Gate (Full Acceptance Checklist)

- [ ] `visage enroll --label normal` captures and stores face model
- [ ] `visage verify` matches enrolled face with cosine similarity > 0.5
- [ ] `visage verify` completes in <500ms (warm daemon, good lighting)
- [ ] `sudo echo test` authenticates via face, falls back to password on timeout
- [ ] 10 consecutive `sudo` attempts: ≥9 succeed via face
- [ ] Cover camera → password prompt appears within 3 seconds
- [ ] IR emitter activates at daemon start (no manual intervention after reboot)
- [ ] Suspend → resume → `sudo echo test` works (IR re-activated via sleep hook)
- [ ] `apt install visage` on clean Ubuntu 24.04 succeeds
- [ ] `apt remove visage` leaves system authenticating via password correctly
- [ ] `visaged` runs with `ProtectSystem=strict` (verified via `systemctl show visaged`)
- [ ] D-Bus policy: `visage enroll` as non-root user is rejected
- [ ] Daemon unavailable → PAM falls back to password (no hang, no error)

---

## Key File Locations (Production)

| Path | Purpose |
|------|---------|
| `/usr/bin/visaged` | Daemon binary |
| `/usr/bin/visage` | CLI binary |
| `/usr/lib/security/pam_visage.so` | PAM module |
| `/etc/visage/config.toml` | Configuration |
| `/etc/dbus-1/system.d/org.freedesktop.Visage1.conf` | D-Bus policy |
| `/usr/lib/systemd/system/visaged.service` | systemd unit |
| `/var/lib/visage/models.db` | SQLite face model store |
| `/var/lib/visage/models/` | ONNX model files |
| `/run/visage/` | Runtime state (tmpfs) |
| `contrib/hw/` | Hardware quirks database |
