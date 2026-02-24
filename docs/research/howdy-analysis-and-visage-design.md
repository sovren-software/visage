# Visage: A Modern Biometric Authentication Framework for Linux

## Lessons from Howdy, and a Path to Order-of-Magnitude Improvement

**Authors:** Sovren Software
**Date:** 2026-02-21
**Status:** Living document — updated as implementation progresses
**Repository:** https://github.com/sovren-software/visage

---

## Abstract

Linux lacks a robust, widely adopted biometric authentication framework comparable to
Windows Hello. Howdy (boltgolt/howdy) is the closest existing solution, providing
PAM-integrated face authentication via dlib. However, Howdy suffers from fundamental
architectural limitations: per-authentication Python subprocess spawning (2-3 second
overhead), absence of anti-spoofing measures, no rate limiting, world-readable model
files, and fragile camera abstraction with known bugs in two of three backends.

This paper presents a detailed analysis of Howdy's architecture derived from source
code study and production debugging on IR-equipped hardware (ASUS Zenbook 14 UM3406HA).
We identify the root causes of Howdy's unreliability and propose Visage — a Rust-based
biometric authentication daemon that eliminates these limitations through persistent
daemon architecture, modern ONNX-based inference, absorbed IR emitter control, and
built-in liveness detection.

Our hypothesis: replacing Howdy's per-request subprocess model with a persistent daemon,
upgrading from dlib HOG to SCRFD+ArcFace, and absorbing IR emitter control into the
authentication stack will produce an order-of-magnitude improvement in authentication
latency (target: sub-500ms vs Howdy's 3-6 seconds), reliability (target: >95% success
rate vs Howdy's observed 44% baseline), and security posture (Tier 1 liveness detection
vs Howdy's Tier 0 with no defaults).

---

## 1. Introduction

### 1.1 The Gap in Linux Authentication

Microsoft Windows ships Windows Hello — a biometric authentication framework that
integrates face recognition, fingerprint, and PIN authentication into the operating
system's security stack. Windows Hello leverages the Windows Biometric Framework (WBF),
providing a standardized interface between biometric hardware, authentication services,
and the credential provider system.

Linux has no equivalent. The fingerprint ecosystem is well-served by fprintd + libfprint
+ pam_fprintd, which follows a clean daemon-client architecture. Face authentication,
however, remains a gap. Howdy is the only actively maintained project that provides
PAM-integrated face authentication, but its adoption is limited by reliability issues,
security concerns, and distribution packaging friction.

### 1.2 Why Howdy Matters

Despite its limitations, Howdy validates a critical insight: **PAM-integrated face
authentication on Linux is achievable with commodity hardware.** IR cameras shipping
in modern laptops (designed for Windows Hello) are accessible via V4L2, dlib's face
recognition models are accurate enough for convenience authentication, and the PAM
stack is flexible enough to support biometric modules alongside password fallback.

The question is not whether Linux can do face authentication, but whether it can do
it well enough for mainstream adoption.

### 1.3 Research Method

Our analysis is based on:

1. **Source code review** of Howdy 3.0.0-beta (commit tree as of February 2026),
   covering all Python modules, the C++ PAM shared library, configuration system,
   and packaging infrastructure.

2. **Production debugging** on an ASUS Zenbook 14 UM3406HA (AMD Ryzen AI, IR camera
   at /dev/video2) running Ubuntu 24.04 with Howdy 2.6.1+slimbook5. This included
   baseline measurement (9 authentication attempts with diagnostic output), root
   cause analysis of a 44% success rate, config tuning to achieve 100% success,
   and IR emitter persistence engineering.

3. **Competitive landscape review** confirming Howdy's unique position: eyMate
   (no IR support), pam-face (inferior accuracy), fprintd (fingerprint only, no face).

---

## 2. Howdy Architecture Analysis

### 2.1 System Overview

Howdy operates as a two-process system:

```
┌────────────────┐     posix_spawnp()     ┌─────────────────┐
│  pam_howdy.so  │ ──────────────────────▶│  compare.py      │
│  (C++ PAM lib) │                        │  (Python process) │
│                │◀──── exit code ────────│                   │
└────────────────┘                        └─────────────────┘
     │                                          │
     │ pam_get_authtok()                        │ cv2.VideoCapture()
     │ (password thread)                        │ dlib face detection
     │                                          │ dlib face recognition
     ▼                                          ▼
  PAM stack                              Camera + Models
```

The PAM module (`pam_howdy.so`) is compiled C++ that spawns a Python subprocess
(`compare.py`) for each authentication attempt. Two threads race concurrently:

- **Face thread:** Waits for `compare.py` to exit with status 0 (match found)
- **Password thread:** Calls `pam_get_authtok()` to accept keyboard password input

Whichever completes first wins. This race design is architecturally sound — it
provides seamless password fallback without requiring the user to wait for face
recognition to time out.

### 2.2 Authentication Flow (Detailed)

On each `pam_sm_authenticate()` invocation:

1. Parse `/etc/howdy/config.ini` via inih C++ library
2. Check guards: disabled flag, SSH session, lid closed, model file exists
3. Send PAM notice: "Attempting facial authentication"
4. `posix_spawnp()` Python interpreter with `compare.py <username>`
5. Python process:
   a. Import ~15 modules (numpy, cv2, dlib, json, configparser, etc.)
   b. Load 3 dlib model files (~100MB total, from disk each time)
   c. Open camera via selected backend (opencv/ffmpeg/pyv4l2)
   d. Enter frame capture loop:
      - Capture frame
      - Convert to grayscale, apply CLAHE equalization
      - Check darkness histogram (skip dark frames)
      - Optionally rotate frame
      - Downscale to max_height
      - Run HOG or CNN face detector
      - For each detected face: extract 128-D embedding via ResNet
      - Compute L2 distance against all stored embeddings
      - If distance < certainty/10: exit(0) — success
   e. On timeout (default 4s): exit(11)
6. PAM module receives exit code, returns PAM_SUCCESS or PAM_IGNORE

### 2.3 Performance Characteristics

Measured on ASUS Zenbook 14 UM3406HA (AMD Ryzen AI 9 HX 370, 14GB RAM):

| Phase | Duration | Notes |
|-------|----------|-------|
| Python interpreter startup | ~200ms | Import overhead |
| Module imports (numpy, cv2, dlib) | ~800ms | cv2 alone is ~400ms |
| dlib model loading (3 files) | ~500ms | ~100MB from disk |
| Camera open + first frame | ~300ms | V4L2 via OpenCV |
| Per-frame HOG detection | ~28ms | Single face, 360p |
| Per-frame CNN detection | ~2745ms | Without CUDA — impractical |
| Per-frame embedding extraction | ~15ms | dlib ResNet |
| Per-frame L2 distance computation | <1ms | numpy vectorized |
| **Total cold start to first match** | **~2.5-3.5s** | Best case |
| **Typical with IR warm-up** | **3-6s** | 35% dark frames skipped |

The 2.5-3.5 second cold start is dominated by Python/library initialization and
model loading — work that is repeated identically on every authentication attempt.

### 2.4 Reliability Analysis

Baseline measurement (9 attempts, Howdy 2.6.1, default config):

| Attempt | Result | Confidence | Notes |
|---------|--------|------------|-------|
| 1 | Timeout | — | 0 faces detected in 60 frames |
| 2 | Success | 3.176 | Match on frame 23/60 |
| 3 | Timeout | — | Faces detected but no match < 3.5 |
| 4 | Success | 3.490 | Barely under threshold (margin: 0.010) |
| 5 | Timeout | — | IR emitter delay |
| 6 | Success | 3.312 | |
| 7 | Success | 3.285 | |
| 8 | Timeout | — | |
| 9 | Timeout | — | |

**Success rate: 44% (4/9)**

Root cause: the default certainty threshold (3.5, mapping to L2 distance 0.35) was
too tight for the measured winning distances (0.3176 - 0.3490). The margin between
best match and threshold was 0.001 - 0.032 — any slight head angle variation pushed
matches over the threshold, producing a silent timeout with no diagnostic feedback.

After tuning certainty to 4.5 (threshold 0.45, providing ~30% headroom):
**Success rate: 100% (10/10)**

This reveals a fundamental UX problem: Howdy ships with a threshold that is too
strict for most hardware, provides no calibration guidance, and fails silently.

### 2.5 Camera Pipeline Issues

Three backends, two with bugs:

| Backend | Status | Issues |
|---------|--------|--------|
| OpenCV | Works | Relies on OpenCV's V4L2 abstraction; limited control over format negotiation |
| ffmpeg | Buggy | Width/height swapped in frame reshape; batch captures 10 frames at once |
| pyv4l2 | Broken | Hardcoded 352x352 resolution; crashes on any other camera resolution |

All three backends lack:
- Explicit IR camera detection (user must manually find /dev/video* path)
- IR emitter control (delegated to external tools)
- Frame timestamp tracking (no way to detect stale frames)
- Exclusive device access (concurrent openers can interfere)

### 2.6 Security Assessment

| Category | Howdy Status | Severity |
|----------|-------------|----------|
| Anti-spoofing | None by default; optional rubberstamps plugin (disabled) | Critical |
| Rate limiting | None | High |
| Lockout | None | High |
| Audit logging | Minimal syslog (success/fail only) | Medium |
| Model file permissions | World-readable (755 on /etc/howdy/) | Medium |
| Shell injection | GTK UI uses unsanitized user input in shell commands | Medium |
| SSH bypass | Checked via env vars (reasonable) | Low |
| pthread_cancel | Self-documented as UNSAFE in workaround=native mode | Medium |

Howdy's security posture is best described as "convenience authentication with no
adversarial considerations." A printed photograph, a video replay on a phone screen,
or a rapid-fire brute force attempt with varying photos would all succeed or not be
throttled.

### 2.7 Distribution Packaging

Howdy's Debian packaging uses `pam-auth-update` — the correct mechanism for safe PAM
integration. However:
- No NixOS module exists
- Arch packaging is pre-meson era and manually copies files
- Fedora packaging is undocumented
- The dlib model download requires internet access during install
- Python dependency management conflicts with system packages

---

## 3. Problem Statement

Howdy demonstrates that PAM-integrated face authentication on Linux is viable. However,
it cannot achieve mainstream adoption due to five structural problems:

1. **Latency:** 2.5-6 second cold start per authentication, dominated by Python
   interpreter and model loading repeated on every attempt.

2. **Reliability:** Ships with a threshold that produces <50% success on typical
   hardware, with no calibration workflow and silent failure mode.

3. **Security:** No anti-spoofing, no rate limiting, no lockout. Not acceptable
   for any authentication system, even one positioned as "convenience."

4. **Fragility:** Two of three camera backends have known bugs. IR emitter control
   is an external dependency with its own bugs. No camera auto-detection.

5. **Packaging:** Python dependency management conflicts across distributions.
   No declarative module for NixOS. Dlib model download requires internet during
   install.

---

## 4. Hypothesis

We hypothesize that replacing Howdy's architecture with a persistent Rust daemon
will produce order-of-magnitude improvements across three dimensions:

### H1: Latency — Sub-500ms authentication (10x improvement)

**Mechanism:** A persistent daemon (`visaged`) loads ONNX models once at boot and
holds them in memory. Camera warm-up is amortized across requests. The authentication
path becomes: receive D-Bus request → capture frame → detect face → extract embedding
→ compare → respond. No interpreter startup, no module imports, no model loading.

**Prediction:** First-frame-to-decision in <200ms (SCRFD ~5ms + ArcFace ~10ms +
camera frame capture ~30ms + overhead). Total user-perceived latency <500ms including
D-Bus round-trip.

### H2: Reliability — >95% success rate out of the box (2x improvement)

**Mechanism:** Ship with a calibration workflow that measures actual match distances
during enrollment and sets the threshold with appropriate headroom. Provide real-time
diagnostic feedback (not silent timeouts). Auto-detect IR vs RGB cameras.

**Prediction:** Calibrated thresholds + multi-frame confirmation + SCRFD's superior
detection accuracy (vs dlib HOG) will achieve >95% success rate without per-user
config tuning.

### H3: Security — Tier 1 liveness by default (∞ improvement from zero baseline)

**Mechanism:** Built-in liveness detection (frame variance analysis, active challenge)
enabled by default. Rate limiting with exponential backoff. Per-user lockout after
configurable failure count. Structured audit logging.

**Prediction:** Defeats printed photo and video replay attacks. Does not claim to
defeat 3D masks or adversarial ML attacks (Tier 2+).

---

## 5. Visage Architecture

### 5.1 Component Model

```
┌─────────────┐     D-Bus IPC      ┌──────────────┐     V4L2      ┌──────────┐
│ pam_visage  │◀───────────────────▶│   visaged    │◀────────────▶│ IR Camera│
│ (PAM cdylib)│  org.freedesktop.  │  (Rust daemon)│              └──────────┘
└─────────────┘  Visage1           │              │
                                    │  ┌──────────┐│  UVC ioctl   ┌──────────┐
┌─────────────┐     D-Bus IPC      │  │visage-hw ││◀────────────▶│IR Emitter│
│ visage CLI  │◀───────────────────▶│  └──────────┘│              └──────────┘
└─────────────┘                     │              │
                                    │  ┌──────────┐│  ONNX Runtime
                                    │  │visage-   ││◀────────────▶ SCRFD
                                    │  │core      ││              ArcFace
                                    │  └──────────┘│
                                    └──────────────┘
```

### 5.2 Key Architectural Differences from Howdy

| Aspect | Howdy | Visage | Impact |
|--------|-------|--------|--------|
| Auth process model | Spawn Python subprocess per request | Persistent daemon, D-Bus IPC | 10x latency reduction |
| Model loading | Per-request (3 files, ~100MB) | Once at daemon start, held in memory | Eliminates 1.5s overhead |
| Camera access | Per-request open/close | Daemon holds device open (configurable) | Eliminates 300ms warm-up |
| Face detection | dlib HOG (28ms/frame, 2005-era) | SCRFD via ONNX (~5ms/frame, 2021) | 5x faster, more accurate |
| Face recognition | dlib ResNet 128-D (2017) | ArcFace 512-D via ONNX (2019) | Higher discriminative power |
| IR emitter | External tool dependency | Absorbed into daemon | Eliminates config fragility |
| Anti-spoofing | None (plugin system, disabled) | Built-in liveness, enabled by default | From 0 to Tier 1 |
| Rate limiting | None | Exponential backoff + lockout | Brute force protection |
| Camera backend | 3 Python backends (2 buggy) | Single V4L2 Rust implementation | One correct path |
| Config format | INI (no validation) | TOML (typed, validated at parse) | No silent misconfiguration |
| Model storage | JSON files, world-readable | SQLite, root-only, encrypted at rest | Privacy + integrity |
| Language | C++ PAM + Python recognition | Rust throughout | Memory safety in auth path |
| Packaging | Manual PAM edits or pam-auth-update | pam-auth-update + NixOS module | Safe install/remove |

### 5.3 D-Bus Interface

Bus name: `org.freedesktop.Visage1`
Object path: `/org/freedesktop/Visage1`

| Method | Signature | Description |
|--------|-----------|-------------|
| `Verify(user: s)` | → `(matched: b, confidence: d, model_id: s)` | Authenticate user against enrolled models |
| `Enroll(user: s, label: s)` | → `(model_id: s)` | Capture and store face embedding |
| `ListModels(user: s)` | → `(models: s)` | JSON array of enrolled models |
| `RemoveModel(user: s, model_id: s)` | → `(success: b)` | Delete specific model |
| `Status()` | → `(status: s)` | JSON daemon status (camera, models, IR) |
| `Calibrate(user: s)` | → `(threshold: d)` | Auto-calibrate threshold from enrolled models |

D-Bus policy: `Verify` callable by any local process (PAM runs as root). `Enroll`,
`RemoveModel`, `Calibrate` restricted to root. `ListModels` and `Status` callable by
the owning user or root.

### 5.4 Inference Pipeline

```
Frame (640x360 IR grayscale)
    │
    ▼
CLAHE equalization (contrast normalization)
    │
    ▼
Dark frame filter (histogram, skip if >95% in lowest bucket)
    │
    ▼
SCRFD face detection (ONNX, ~5ms)
    │ outputs: bounding boxes + landmarks
    ▼
Face alignment (affine transform using 5 landmarks)
    │
    ▼
ArcFace embedding extraction (ONNX, ~10ms)
    │ outputs: 512-D normalized float vector
    ▼
Cosine similarity against enrolled embeddings
    │
    ▼
Threshold check → Accept / Reject
```

### 5.5 Liveness Detection (Tier 1)

Default-enabled, two-stage:

**Stage 1 — Passive (every auth, no user action required):**
- Frame variance analysis: require non-zero optical flow across 3+ frames
- IR strobe pattern: verify alternating bright/dark frames match expected IR
  emitter frequency (detects static image or screen replay)

**Stage 2 — Active (configurable, triggered on low-confidence matches):**
- Random challenge: "look left," "blink," "nod" — tracked via landmark movement
- Time-bounded (3 seconds) with cooperative user feedback

### 5.6 Rate Limiting and Lockout

```
Attempt 1-3:   No delay
Attempt 4-5:   2-second delay between attempts
Attempt 6-10:  5-second delay
Attempt 11+:   30-second lockout
After 20:      5-minute lockout, syslog alert

Reset: successful password authentication clears the counter
```

State stored in `/run/visage/attempts/<user>.json` (tmpfs, cleared on reboot).

---

## 6. Technical Design Decisions

### 6.1 Why Rust

Face authentication runs in the PAM stack — the most security-sensitive code path on
a Linux system. Memory corruption in a PAM module is a privilege escalation vector.
Rust eliminates buffer overflows, use-after-free, and data races at compile time.

Additionally, Rust produces static binaries with no runtime dependencies (no Python
interpreter, no pip, no virtualenv). This eliminates the single largest source of
Howdy packaging issues across distributions.

### 6.2 Why SCRFD + ArcFace (not dlib)

| Model | Year | Params | Accuracy (LFW) | Speed (CPU) | Format |
|-------|------|--------|-----------------|-------------|--------|
| dlib HOG detector | 2005 | — | — | 28ms/frame | C++ lib |
| dlib CNN detector | 2017 | ~6M | — | 2745ms/frame | C++ lib |
| dlib ResNet embedder | 2017 | 29M | 99.38% | 15ms/face | C++ lib |
| SCRFD-500M detector | 2021 | 0.57M | — | ~5ms/frame | ONNX |
| SCRFD-10G detector | 2021 | 3.86M | — | ~15ms/frame | ONNX |
| ArcFace-R50 embedder | 2019 | 44M | 99.83% | ~10ms/face | ONNX |

SCRFD provides 5x faster detection than dlib HOG with better accuracy on rotated
and partially occluded faces. ArcFace's 512-D embeddings have higher discriminative
power than dlib's 128-D (99.83% vs 99.38% on LFW). Both are available as small ONNX
models that run on any CPU via ONNX Runtime — no CUDA required.

### 6.3 Why Absorb IR Emitter Control

Our production debugging revealed that `linux-enable-ir-emitter` 7.0.0-beta2 has:
- A TOML configuration parse bug (`--config` flag fails on valid TOML)
- Implicit `$HOME` dependency (fails in systemd services without `Environment=HOME=`)
- No boot/resume persistence (requires separate systemd service + sleep hook)
- Per-camera UVC control bytes that must be manually discovered

By absorbing IR emitter control into the daemon, Visage:
- Eliminates the external dependency and its bugs
- Activates the emitter as part of camera initialization (correct lifecycle)
- Ships a hardware quirks database (camera vendor:product → UVC control bytes)
- Deactivates the emitter when not authenticating (power saving)

### 6.4 Why D-Bus (not Unix Socket)

D-Bus provides:
- Policy-based access control (who can call which methods)
- Service activation (daemon auto-starts on first auth request)
- Desktop integration (Polkit agents can discover and use the service)
- Precedent (fprintd uses this exact pattern)

The overhead (~1ms per round-trip on local bus) is negligible compared to the
inference pipeline.

### 6.5 Why TOML Configuration (not INI)

Howdy's INI config has no type validation — `certainty = banana` silently defaults
to 3.5. TOML provides:
- Typed values (integer, float, boolean, string, array)
- Nested sections for logical grouping
- Standard format with parsers in every language
- Inline documentation via comments

Visage validates the entire config at daemon startup and rejects invalid values
with clear error messages.

---

## 7. Evaluation Plan

### 7.1 Latency Benchmark

Measure time from `Verify()` D-Bus call to response, with model pre-loaded:

| Metric | Howdy Baseline | Visage Target |
|--------|---------------|---------------|
| Cold start (daemon not running) | 3.5s | <1.5s (daemon auto-start + first frame) |
| Warm start (daemon running) | 3.5s (subprocess each time) | <500ms |
| Per-frame inference | 43ms (HOG + ResNet) | <20ms (SCRFD + ArcFace) |
| IR emitter activation | Manual/external | <50ms (integrated UVC ioctl) |

### 7.2 Reliability Benchmark

10 users, 10 attempts each, varying conditions:

| Condition | Howdy Target | Visage Target |
|-----------|-------------|---------------|
| Normal lighting, straight face | >90% | >99% |
| Dim lighting (screen glow only) | unknown | >90% |
| Slight head rotation (±15°) | unknown | >95% |
| Glasses on/off variation | unknown | >90% |
| After suspend/resume | 0% (no persistence) | >95% |
| Overall | ~44% (measured) | >95% |

### 7.3 Security Evaluation

| Attack | Howdy Result | Visage Target |
|--------|-------------|---------------|
| Printed photo | Succeeds | Blocked (IR + liveness) |
| Phone screen replay | Succeeds | Blocked (IR + frame variance) |
| Brute force (100 attempts) | All attempted | Locked out after 20 |
| Stolen model file | Readable by any user | Root-only, encrypted |
| Concurrent auth spam | No protection | Rate limited |

### 7.4 Packaging Verification

| Distribution | Test |
|-------------|------|
| Ubuntu 24.04 | Install, enable, auth via sudo + GDM + lock screen |
| NixOS 25.05 | Declarative module, nixos-rebuild switch, auth tests |
| Arch Linux | Manual PAM config, pacman package |
| Fedora 41 | RPM package, SELinux compatibility |

---

## 8. Lessons Learned from Howdy (Catalogue)

### 8.1 Patterns to Replicate

1. **Concurrent face + password race** — Architecturally sound UX model
2. **Guard chain before hardware access** — Check disabled/SSH/lid/model before camera
3. **CLAHE histogram equalization** — Essential for IR frame quality
4. **Dark frame histogram filter** — Cheap and effective IR warm-up detection
5. **Parallel model loading + camera init** — Hide latency behind I/O
6. **PAM `[success=end default=ignore]`** — Correct fallback semantics
7. **L2 distance matching** — Correct metric for dlib embeddings (cosine for ArcFace)
8. **Per-model metadata (id, timestamp, label)** — Useful for management
9. **Rubberstamps plugin concept** — Extensible post-auth verification
10. **Lid-closed detection via /proc/acpi** — Low-cost UX guard
11. **SSH detection scanning raw environ** — Necessary for sudo contexts

### 8.2 Anti-Patterns to Avoid

1. **Per-request subprocess spawn** — 2-3s overhead, repeated identically
2. **`pthread_cancel` for thread cancellation** — Documented as unsafe
3. **Hardcoded frame dimensions** — pyv4l2 352x352 crashes on other cameras
4. **Width/height swap in frame reshape** — ffmpeg backend produces garbage
5. **Config editing via regex on raw file** — No section awareness, corruption risk
6. **World-readable model files** — Privacy violation
7. **Shell injection via `subprocess.getstatusoutput`** — GTK UI vulnerability
8. **Python `builtins` module as global state** — Anti-pattern for shared state
9. **No rate limiting in authentication path** — Critical security gap
10. **Silent timeout with no diagnostic output** — Users cannot debug failures
11. **Inverted certainty scale** — Lower = stricter confuses users
12. **CNN mode without performance guard** — 2745ms/frame without warning

### 8.3 Improvements Over Howdy's Ideas

1. **Workaround system** → Coordinated display manager dismissal where possible;
   `uinput` Enter injection only as last resort for terminal contexts
2. **Snapshot logging** → Structured JSON audit log with rotation and retention
3. **Rubberstamps plugins** → Rust trait-based liveness plugins with TOML config
4. **Test command** → Diagnostic mode streaming annotated frames over Unix socket
5. **Certainty scale** → Present as "precision" (higher = stricter) with visual
   calibration showing actual match distances
6. **Camera warm-up** → Daemon keeps camera in standby; pre-warm on screen-wake event
7. **Model storage** → SQLite with per-user row-level access, not flat JSON files

---

## 9. Milestones

| # | Milestone | Scope | Validates |
|---|-----------|-------|-----------|
| 0 | Skeleton + research | Workspace, architecture docs, Howdy analysis | This document |
| 1 | Camera capture + ONNX inference | V4L2 frame capture, SCRFD detection, ArcFace embedding | H1 (latency) |
| 2 | Daemon + D-Bus API | visaged service, Enroll/Verify/Status over D-Bus | H1 (persistent daemon) |
| 3 | PAM module | pam_visage.so calls daemon, password race, fallback | End-to-end auth |
| 4 | IR emitter + quirks DB | UVC control bytes, hardware detection, boot persistence | Hardware reliability |
| 5 | Liveness detection | Passive frame variance + active challenge | H3 (security) |
| 6 | Rate limiting + audit | Attempt tracking, lockout, structured logging | H3 (security) |
| 7 | Calibration workflow | Auto-threshold from enrollment data, diagnostic output | H2 (reliability) |
| 8 | Ubuntu packaging | .deb package, pam-auth-update profile, systemd unit | Distribution target |
| 9 | NixOS module | Declarative module in nixpkgs or overlay format | Augmentum OS integration |
| 10 | CLI polish + docs | visage enroll/verify/list/remove/test/status/calibrate | User experience |

---

## 10. Conclusion

Howdy proves that PAM-integrated face authentication on Linux works. Its architecture
— a PAM module racing face recognition against password input — is fundamentally sound.
What Howdy gets wrong is implementation: Python subprocess overhead, absent security
measures, buggy camera backends, and silent failure modes.

Visage addresses these through three structural changes:

1. **Persistent daemon** eliminates per-request startup cost (2.5s → <200ms inference)
2. **Modern ONNX models** improve accuracy and speed (dlib HOG → SCRFD+ArcFace)
3. **Security by default** adds liveness, rate limiting, and audit logging from day one

The result should be face authentication that is fast enough to feel instant, reliable
enough to be trusted, and secure enough to be recommended — closing the gap between
Linux and Windows Hello for the first time.

---

## References

1. Microsoft Windows Hello Face Authentication — https://learn.microsoft.com/en-us/windows-hardware/design/device-experiences/windows-hello-face-authentication
2. Howdy (boltgolt/howdy) — https://github.com/boltgolt/howdy
3. ISO/IEC 30107-1 Biometric Presentation Attack Detection — https://www.iso.org/standard/53227.html
4. fprintd + pam_fprintd — https://packages.debian.org/sid/libpam-fprintd
5. libcamera — https://libcamera.org/introduction.html
6. PAM pam_sm_authenticate — https://man7.org/linux/man-pages/man3/pam_sm_authenticate.3.html
7. SCRFD: Sample and Computation Redistribution for Efficient Face Detection (2021) — https://arxiv.org/abs/2105.04714
8. ArcFace: Additive Angular Margin Loss (2019) — https://arxiv.org/abs/1801.07698
9. NIST FRVT Demographics — https://pages.nist.gov/frvt/reports/demographics/nistir_8429.pdf
10. NixOS PAM module — https://github.com/NixOS/nixpkgs/blob/master/nixos/modules/security/pam.nix
11. Arch Wiki Howdy — https://wiki.archlinux.org/title/Howdy
12. pam-auth-update — https://manpages.debian.org/testing/libpam-runtime/pam-auth-update.8.en.html

---

## Appendix A: Howdy Source File Index

| File | Lines | Purpose |
|------|-------|---------|
| `src/pam/main.cc` | ~440 | C++ PAM module — auth orchestration, workaround system |
| `src/compare.py` | ~380 | Python recognition loop — camera, detection, matching |
| `src/cli.py` | ~120 | CLI entry point and command dispatch |
| `src/cli/add.py` | ~210 | Face enrollment (capture + encode + store) |
| `src/cli/test.py` | ~140 | Live camera test with OpenCV window |
| `src/recorders/video_capture.py` | ~130 | Camera backend factory |
| `src/recorders/ffmpeg_reader.py` | ~100 | ffmpeg batch capture (buggy reshape) |
| `src/recorders/pyv4l2_reader.py` | ~100 | V4L2 direct capture (hardcoded 352x352) |
| `src/rubberstamps/nod.py` | ~70 | Head nod liveness detection |
| `src/snapshot.py` | ~60 | Annotated JPEG snapshot generation |
| `src/config.ini` | ~130 | Default configuration with all options |

## Appendix B: Measured IR Camera Characteristics (ASUS Zenbook 14 UM3406HA)

| Property | Value |
|----------|-------|
| Device path | /dev/video2 |
| Resolution | 640x360 |
| Format | GREY (native grayscale; driver returns GREY when YUYV requested) |
| IR emitter UVC unit | 14 |
| IR emitter UVC selector | 6 |
| IR emitter control bytes | [1, 3, 3, 0, 0, 0, 0, 0, 0] |
| Dark frame rate (emitter active) | ~35% (hardware strobe pattern) |
| HOG detection time | ~28ms/frame |
| Typical match L2 distance | 0.317 - 0.349 |
| Optimal certainty threshold | 4.5 (L2 distance 0.45) |
| Recording plugin | ffmpeg (grayscale fix) |
