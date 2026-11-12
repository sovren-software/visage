# Visage

**Linux face authentication via PAM — persistent daemon, IR camera support, ONNX inference.**

The Windows Hello equivalent for Linux. Visage authenticates `sudo`, login, and any
PAM-gated service using your face — no subprocess spawn, no interpreter startup, no model reload.

> Built in Rust by [Sovren Software](https://sovren.software). Ships standalone on any Linux system.

---

Visage runs as a persistent daemon: SCRFD face detection and ArcFace recognition are loaded
once at startup via ONNX Runtime, and camera ownership is held across auth requests. Compare
to [Howdy](https://github.com/boltgolt/howdy) — Python subprocess per auth attempt, 2–3s
cold start, no IR emitter integration.

⚠️ **On measured latency Visage does not yet beat Howdy by much.** The architecture removes
Howdy's per-auth startup cost, but the delivered number on our reference IR module is a
**median 2,273 ms** verify (max 2,329 ms over 10 runs) against Howdy's 2–3s. The design
budgeted <200ms of inference; CPU-only ONNX does not deliver that, and the verify path runs
recognition on every captured frame rather than the best one. **The pipeline has never been
profiled**, so where the remaining ~2s goes is genuinely unknown. Tracked in
[STATUS](docs/STATUS.md) and honestly unmet; do not quote a sub-second figure for Visage.

Built in Rust for memory safety throughout the authentication path. Integrates via standard
Linux-PAM — no kernel patches, no modified sudo.

## Status

**v0.4.0 — feature-complete, running on real hardware.**

Enrollment, verification, PAM integration for `sudo` and lock screens, systemd hardening,
D-Bus access control, package lifecycle and suspend/resume are implemented and tested end
to end. **`sudo visage onboard`** takes a fresh machine to working face auth in one command.

Since v0.3.6: one-command onboarding, the first hardware validation of passive liveness, a
configurable PAM timeout, the first integration tests, Fedora RPM packaging, and three more
IR emitter quirks. v0.4.0 added `PreviewFrame`, so an enrolling client can show you what the
camera sees, and a pre-install hardware check you can run before installing anything. See
[CHANGELOG](CHANGELOG.md) for the full history.

> ⚠️ **Keep a password fallback. Do not make this your only authentication factor yet.**
> On its first hardware spoof validation, passive liveness did **not** discriminate: a
> hand-held phone screen displaced *more* than two genuine live attempts, and the identity
> stage matched that same photo. Every PAM stack shipped here falls through to the password,
> and it should stay that way. Details: [Known Limitations](docs/STATUS.md#known-limitations-at-v03)
> and the [threat model](docs/threat-model.md).

## Architecture

```
┌─────────────┐     ┌──────────────┐     ┌──────────────┐
│  pam_visage │────▶│   visaged    │────▶│  IR Camera   │
│  (PAM module)│ D-Bus│  (daemon)    │     │  + Emitter   │
└─────────────┘     └──────┬───────┘     └──────────────┘
                           │
                    ┌──────▼───────┐
                    │ visage-core  │
                    │ SCRFD+ArcFace│
                    │ (ONNX)       │
                    └──────────────┘
```

### Components

| Crate | Type | Purpose |
|-------|------|---------|
| `visaged` | Binary | System daemon — owns camera, D-Bus API, IR emitter control |
| `pam-visage` | cdylib | Thin PAM module — calls daemon over D-Bus |
| `visage-cli` | Binary | CLI tool — enroll, verify, test, diagnostics |
| `visage-core` | Library | Face detection (SCRFD) + recognition (ArcFace) via ONNX |
| `visage-hw` | Library | Camera capture, IR emitter control, hardware quirks DB |
| `visage-models` | Library | ONNX model manifest, pinned SHA-256 checksums, integrity verification |
| `visage-ipc` | Library | The D-Bus client surface, defined once and shared by every client |
| `visage-tui` | Binary | `visage-enroll` — enrollment with a live view of the camera |

## Quick Start (Build from Source)

Clone the repository and run the quickstart script. It handles dependency checks,
building, packaging, installation, model download, face enrollment, and verification
— from zero to working face auth in one command.

```bash
git clone https://github.com/sovren-software/visage.git
cd visage
./scripts/quickstart.sh
```

The script validates your environment at each stage with clear pass/fail signals.
Requires Ubuntu 24.04 (amd64), Rust toolchain, a camera, and internet access.
Use `--no-enroll` for headless/CI builds.

For full instructions — configuration, troubleshooting, multi-user, removal — see
the [Operations Guide](docs/operations-guide.md).

## Installation

### Ubuntu / Debian (.deb)

```bash
sudo apt install ./visage_*_amd64.deb
sudo visage onboard                        # models, enrollment, verification — one command
sudo echo "face auth works"                # test — face first, password fallback
```

`onboard` downloads the ONNX models (~182 MB), captures several labelled angles with a
prompt between each, and verifies against the daemon before reporting success — so a
failed enrollment cannot look like a working one. It exits non-zero if verification does
not recognise you.

PAM is configured automatically via `pam-auth-update`.

```bash
sudo apt remove visage     # removes binaries, disables PAM and service
sudo apt purge visage      # also removes /var/lib/visage (models + face database)
```

The [quickstart script](#quick-start-build-from-source) automates building from source.
To build manually:

```bash
sudo apt install libpam0g-dev libdbus-1-dev
cargo install cargo-deb
cargo build --release --workspace
cargo deb -p visaged --no-build
sudo apt install ./target/debian/visage_*.deb
```

### NixOS (flake)

```nix
# flake.nix
{
  inputs.visage.url = "github:sovren-software/visage";

  outputs = { self, nixpkgs, visage, ... }: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      modules = [
        visage.nixosModules.default
        { services.visage.enable = true; }
      ];
    };
  };
}
```

Then run onboarding:

```bash
sudo visage onboard
```

The NixOS module handles systemd, D-Bus policy, and PAM integration declaratively.
See `packaging/nix/module.nix` for all options (`modelDir`, `camera`, `similarityThreshold`, etc.).

### Arch Linux (AUR)

```bash
git clone https://aur.archlinux.org/visage.git
# visage-git and visage-bin are also available
cd visage && makepkg -si
sudo visage onboard
# add --user <username> to onboard someone else
```

PAM requires a manual one-line edit on Arch — add before `pam_unix.so` in
`/etc/pam.d/system-auth`:

```
auth  [success=done default=ignore]  pam_visage.so
```

### Fedora (script — tested on Fedora 44)

```bash
sudo dnf install pam-devel clang-devel
./scripts/install-fedora.sh
# PAM stays manual (authselect owns system-auth) — 1 line in /etc/pam.d/sudo
sudo /usr/local/bin/visage onboard
```

Full guide — PAM wiring, GNOME lock screen, SELinux, suspend/resume,
3277:0055 hardware notes, troubleshooting, uninstall — see
[docs/fedora.md](docs/fedora.md).

Prefer the RPM path instead?

```bash
cargo install cargo-generate-rpm
cargo build --release --workspace
cargo generate-rpm -p crates/visaged
sudo dnf install ./target/generate-rpm/visage-*.x86_64.rpm
sudo visage onboard
```

Fedora has no `pam-auth-update` and `authselect` owns `system-auth`, so PAM is
configured manually per service — never in `system-auth` (authselect overwrites
it). See [docs/fedora.md](docs/fedora.md) for the `sudo` / `gdm-password` wiring.

Tracking a COPR repository in [#101](https://github.com/sovren-software/visage/issues/101).

### What the package does

- Installs `visaged` (daemon), `visage` (CLI), `visage-enroll` (enrollment with a live
  camera view), and `pam_visage.so` (PAM module)
- Enables the `visaged` systemd service and `visage-resume.service` (suspend/resume)
- Configures PAM (automatic on Ubuntu/NixOS, manual on Arch)

## Usage

```bash
# Set up everything — models, enrollment, verification (start here)
sudo visage onboard

# Enrol with a live view of what the camera sees, so you can tell
# "too dark" from "off-centre" instead of guessing why a capture failed
sudo visage-enroll

# Verify interactively (exits 0 on match, 1 on no-match)
visage verify

# List enrolled models
visage list

# Show daemon status
visage status

# Remove a model
sudo visage remove <model-id>
```

### Hardware discovery

```bash
# List cameras, VID:PID, and IR emitter quirk status
visage discover
```

Output example:
```
/dev/video2  VID=0x04f2 PID=0xb6d9  quirk: ASUS Zenbook 14 UM3406HA IR Camera ✓
/dev/video4  VID=0x0bda PID=0x5850  no quirk (VID=0x0bda PID=0x5850)
```

### Camera diagnostics

```bash
# Test IR camera (default /dev/video2)
visage test

# Specify device and frame count
visage test --device /dev/video0 --frames 5
```

Captures frames with the IR emitter active, applies dark-frame filtering and CLAHE
contrast enhancement, saves grayscale PGM files to `/tmp/visage-test/`, and prints
a summary. Requires the daemon to be running for emitter activation.

## Hardware Support

Visage works with **USB UVC IR cameras** — the class of IR / "Windows Hello" cameras
that appear as standard V4L2 devices under the `uvcvideo` kernel driver. No external
tools required: Visage includes built-in IR emitter activation via UVC extension unit
control, so there is no dependency on `linux-enable-ir-emitter`.

Pixel formats GREY (1 byte/pixel), YUYV (2 bytes/pixel), and Y16 (16-bit LE) are all
supported and detected automatically at device open.

### Compatibility tiers

| Tier | Camera stack | Visage support | Examples |
|------|-------------|----------------|---------|
| **Supported** | UVC IR (`uvcvideo` driver plus IR stream/emitter path) | ✅ Full support | ASUS ZenBook, ThinkPad T/X (pre-Gen 11), HP EliteBook (UVC IR configs), Dell Latitude (UVC IR configs), TUXEDO InfinityBook |
| **Not secure-compatible** | UVC RGB-only webcam | ❌ Testing only; not for PAM auth | ASUS ExpertBook B3302FEA/B5302FEA built-in `13d3:56ea` |
| **Not supported** | Intel IPU6 / MIPI / libcamera | ❌ Not yet | Newer Dell XPS, ThinkPad Gen 11+ (some configs), Intel "AI PC" cameras |
| **No IR camera** | N/A | — | Framework, System76, Purism |

**Not sure which your laptop has?** Run `visage discover` — it detects the kernel
driver for each `/dev/video*` device and warns if an IPU6 camera is found. A
normal RGB UVC webcam is not enough for secure auth; see the tested
[ASUS ExpertBook B3302FEA report](docs/hardware-reports/asus-expertbook-b3302fea-13d3-56ea.md)
for an example of an incompatible `uvcvideo` camera.

**ThinkPad note:** ThinkPad T-series and X1 Carbon laptops frequently ship with a
separate USB UVC IR camera alongside the RGB webcam. These typically appear as a
second `/dev/video*` node under `uvcvideo` and work with Visage. However, newer
ThinkPad generations (Gen 11+) may use Intel IPU6 for the integrated camera stack.

**IPU6 note:** Intel IPU6 cameras require the proprietary Intel camera HAL and
libcamera, not V4L2. Supporting them is a separate milestone (v0.4+).

### IR emitter quirks

Some cameras require a specific UVC control byte sequence to activate the IR emitter.
These are tracked in `contrib/hw/` as TOML files embedded at compile time.

Confirmed quirk entries:

| File | Device | Source |
|------|--------|--------|
| `04f2-b6d9.toml` | ASUS Zenbook 14 UM3406HA | Verified on hardware |
| `04f2-b6d0.toml` | Lenovo ThinkPad P14s Gen 2a 21A0000RMX | Verified on hardware (community) |
| `174f-2454.toml` | Lenovo ThinkPad X1 Carbon Gen 9 20XW00FPUS | Verified on hardware |
| `174f-11a8.toml` | Lenovo ThinkPad P14s Gen 4 21HF | Verified on hardware (community) |
| `30c9-00c2.toml` | Lenovo ThinkBook 14 MP2PQAZG | Verified on hardware |
| `30c9-0120.toml` | HP OmniBook X Flip | Verified on hardware |

To add support for your camera, see [contrib/hw/README.md](contrib/hw/README.md).

For the full compatibility tier table and per-model notes, see
[docs/hardware-compatibility.md](docs/hardware-compatibility.md).

## Test Results (Ubuntu 24.04.4 LTS)

End-to-end acceptance test — CCX20, USB webcam `/dev/video2`, GREY format, CPU-only ONNX.

| Test | Result |
|------|--------|
| Enroll, verify, match | ✅ similarity 0.87–0.90 |
| Daemon restart — data persists | ✅ |
| Kill daemon — `sudo` falls back to password | ✅ |
| `apt install` / `remove` / `purge` lifecycle | ✅ |
| Systemd hardening (`ProtectSystem=strict`, `char-video4linux rw`) | ✅ |
| D-Bus access control (non-root enroll rejected) | ✅ |
| PAM stack (no terminal output on failure) | ✅ |
| Suspend/resume via `visage-resume.service` | ✅ |

Latency: ~1.4s on USB webcam + CPU-only ONNX; **median 2,273 ms on the `3277:0055` IR
module**. The <500ms target is **not met** and is not close. An earlier version of this README
claimed "~200ms warm recognition" — that number was a *prediction* from the pre-build design
doc, never a measurement, and it is withdrawn.

Bugs fixed during testing: [DeviceAllow glob](docs/STATUS.md#bugs-found-during-testing),
[tokio::time::timeout panic in zbus context](docs/STATUS.md#bugs-found-during-testing).

## Documentation

- [Operations Guide](docs/operations-guide.md) ← start here: installation, configuration, troubleshooting
- [Hardware Compatibility](docs/hardware-compatibility.md) ← supported cameras, tiers, quirks
- [Release Status & Known Limitations](docs/STATUS.md)
- [Architecture](docs/architecture.md)
- [Threat Model](docs/threat-model.md)
- [Architecture Decisions](docs/decisions/) ← 13 ADRs covering implementation, security, and governance decisions
- [ADR 013 — Enrollment preview, and why the TUI ships](docs/decisions/013-enrollment-preview-and-the-tui-front-end.md)

## Security

Visage is a PAM authentication module — security vulnerabilities have direct impact.
**Report security issues privately** via [GitHub Private Vulnerability Reporting](https://github.com/sovren-software/visage/security/advisories/new).
Do not open public issues for security bugs.

Full policy, scope, and response timeline: [SECURITY.md](SECURITY.md).

## Contributing

Visage is feature-complete for facial authentication. Community contributions are
focused on **hardware validation** (IR camera quirks) and **distribution packaging**.

See [CONTRIBUTING.md](CONTRIBUTING.md) for the full guide, including:
- The **Adopt-a-Laptop** program — test on your hardware, submit a report
- **PR guidelines** — merge strategy, review timeline, DCO sign-off
- **Out-of-scope features** — what we will and will not merge
- **Packaging status** by distribution

## License

MIT
