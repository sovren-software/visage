# Contributing to Visage

Visage is **feature-complete for facial authentication**. The core pipeline — IR camera
capture, SCRFD detection, ArcFace recognition, PAM integration, IR emitter control,
and Ubuntu packaging — is implemented, tested, and working.

What remains is **hardware validation and distribution coverage**. That is where
community contributions matter most.

---

## What the project needs

### 1. Hardware validation — IR camera quirks

Visage supports any Linux IR camera that presents as a standard V4L2 device under the
`uvcvideo` kernel driver. For emitter-equipped cameras, it needs a quirk TOML entry
mapping the USB VID:PID to the correct UVC control byte sequence.

**This is the highest-impact contribution you can make.** Each new quirk entry
unlocks support for every user with that laptop model.

**What to do:**

```bash
# 1. Find your camera's VID:PID
visage discover

# 2. Check if a quirk already exists
#    (discover will say "✓" or "no quirk")

# 3. If no quirk exists, find the control bytes
linux-enable-ir-emitter configure
#    Or: analyse UVC descriptors with uvc-util

# 4. Create contrib/hw/{vid}-{pid}.toml following the format below
# 5. Submit a PR
```

**Quirk file format** (`contrib/hw/{vid}-{pid}.toml`):

```toml
[device]
vendor_id  = 0x04F2
product_id = 0xB6D9
name       = "ASUS Zenbook 14 UM3406HA IR Camera"

[emitter]
unit          = 14
selector      = 6
control_bytes = [1, 3, 3, 0, 0, 0, 0, 0, 0]
```

See [contrib/hw/README.md](contrib/hw/README.md) for full field documentation.

---

### 2. The Adopt-a-Laptop program

Run the standard test suite on your hardware and submit a report. Even a "it works /
it doesn't work" report is valuable — it builds the public compatibility matrix.

**Test script:**

```bash
# Prerequisites: visage installed, visaged running, face enrolled
# Takes ~2 minutes

echo "=== Visage Hardware Report ===" > report.txt
echo "Date: $(date)" >> report.txt
echo "OS: $(lsb_release -ds 2>/dev/null || cat /etc/os-release | grep PRETTY_NAME)" >> report.txt
echo "Kernel: $(uname -r)" >> report.txt
echo "" >> report.txt

echo "=== Camera discovery ===" >> report.txt
visage discover >> report.txt 2>&1
echo "" >> report.txt

echo "=== Enroll (5 frames) ===" >> report.txt
time sudo visage enroll --label test >> report.txt 2>&1
echo "" >> report.txt

echo "=== Verify (3 attempts) ===" >> report.txt
for i in 1 2 3; do
  echo "Attempt $i:" >> report.txt
  time visage verify >> report.txt 2>&1
done

echo "=== Daemon status ===" >> report.txt
visage status >> report.txt 2>&1

cat report.txt
```

**Submit:** Open an issue with the title `[Hardware Report] <Laptop Model>` and paste the
output. Include whether the IR emitter activates (frames are bright) or not (frames are dark).

**Report template:**

```
Laptop: <brand model, e.g. "Lenovo ThinkPad T14s Gen 2">
Camera node: /dev/video? (from visage discover)
Camera driver: uvcvideo / intel_ipu6 / other
IR emitter: activates / dark frames / unknown
Enroll time: ~Xs
Verify time: ~Xs
Match success rate: X/3
OS: Ubuntu 24.04 / Fedora 41 / Arch / NixOS / other
Kernel: 6.x.x
Notes: <anything unusual>
```

---

### 3. Distribution packaging

Currently packaged for **Ubuntu 24.04** (`.deb`). These distributions are on the roadmap:

| Distro | Format | Status | Notes |
|--------|--------|--------|-------|
| Ubuntu 24.04 | `.deb` | ✅ Done | `packaging/debian/` |
| NixOS | flake / overlay | 🔲 Wanted | `packaging/nix/` — contributions welcome |
| Arch / AUR | PKGBUILD | 🔲 Wanted | `packaging/aur/` — contributions welcome |
| Fedora / COPR | `.spec` | 🔲 Wanted | Timing: Fedora 43 dlib removal window |
| Debian (stable) | `.deb` | 🔲 Future | Depends on stable ONNX Runtime packaging |

If you maintain packages for any of these distributions, open an issue or PR.
The core build (`cargo build --release --workspace`) works on any Linux with
`libpam0g-dev` and `libdbus-1-dev`.

---

## What we will NOT merge

Visage is **a focused facial authentication tool**. It does one thing: authenticate
users via face recognition through PAM. We will not merge PRs that expand that scope.

Specifically, these are **out of scope for Visage**:

| Feature | Why out of scope |
|---------|-----------------|
| Gesture or motion tracking | Planned for Augmentum OS desktop layer — not an auth primitive |
| Fingerprint authentication | Different hardware domain; fprintd already exists and is well-maintained |
| Alternative biometrics (iris, voice, behavioral) | Separate evaluation required; voice is planned for a future multi-modal platform |
| LLM or AI models in core crates | The authentication path is deterministic. Always. |
| Cloud sync of face models | Local-only is a design constraint, not a limitation |
| GUI enrollment tool | Out of v2 scope; contributes to `visage-assistant` in v3 if at all |

**Why document this explicitly:** The Linux community is generous with feature suggestions.
These boundaries exist to keep Visage maintainable and security-auditable, not because the
ideas are bad. If you want gestures or voice on Linux, watch the [Augmentum OS](https://augmentum.computer) project.

---

## Roadmap

**v0.2 targets** (before public community launch, Summer 2026):

- [x] AUR PKGBUILD (`packaging/aur/`)
- [x] NixOS derivation (`packaging/nix/`) — flake wiring pending
- [ ] Howdy vs Visage benchmark (matched hardware, published methodology)
- [ ] Active liveness detection (blink challenge — proof of concept)
- [ ] Enroll quality scoring (reject blurry / dark / partial frames at capture time)
- [ ] `visage discover --json` for structured output (gating requirement for v3 classifier)

**v0.3 targets:**

- [ ] Intel IPU6 camera support via libcamera
- [ ] GPU-accelerated inference (OpenCL/Vulkan)
- [ ] Per-user adaptive similarity threshold
- [ ] Enrollment quality model (ONNX, lightweight)

**v3 (future platform):** See [docs/STRATEGY.md](docs/STRATEGY.md) for the full roadmap.

---

## PR guidelines

- **Hardware quirks:** Open immediately — we merge these fast.
- **Distro packaging:** Open an issue first to coordinate; we want to review the
  packaging approach before you invest significant time.
- **Bug fixes:** PRs welcome without prior discussion for anything in the issue tracker.
- **New features:** Open an issue first. If it is in the out-of-scope list above,
  it will be declined — save yourself the time.
- **Core security changes** (`visaged`, `pam-visage`, `visage-core`): Discuss in an
  issue before opening a PR. The auth path deserves extra review.

**Code style:** `cargo fmt` + `cargo clippy --workspace -- -D warnings` must both pass.
Tests: `cargo test --workspace`. No new warnings.

---

## Getting started

```bash
git clone https://github.com/sovren-software/visage
cd visage

# Check all crates compile
cargo check --workspace

# Run tests
cargo test --workspace

# Build the CLI for local testing
cargo build -p visage-cli --release
./target/release/visage discover
```

See [docs/operations-guide.md](docs/operations-guide.md) for installation and setup.
See [docs/hardware-compatibility.md](docs/hardware-compatibility.md) for camera compatibility.

---

*Visage is the default face authentication layer for [Augmentum OS](https://augmentum.computer),
shipping Summer 2026.*
