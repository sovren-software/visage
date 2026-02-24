# Visage

**Linux face authentication via PAM — persistent daemon, IR camera support, ONNX inference.**

The Windows Hello equivalent for Linux. Visage authenticates `sudo`, login, and any
PAM-gated service using your face — with sub-second response and no subprocess overhead.

> Built in Rust by [Sovren Software](https://sovren.software). Ships standalone on any Linux system.

---

Visage runs as a persistent daemon: SCRFD face detection and ArcFace recognition are loaded
once at startup via ONNX Runtime, and camera ownership is held across auth requests. Compare
to [Howdy](https://github.com/boltgolt/howdy) — Python subprocess per auth attempt, 2–3s
cold start, no IR emitter integration. Visage completes a warm recognition in ~200ms.

Built in Rust for memory safety throughout the authentication path. Integrates via standard
Linux-PAM — no kernel patches, no modified sudo.

## Status

**v0.3.0 — feature-complete, end-to-end tested on Ubuntu 24.04.4 LTS.**

All 6 implementation steps complete. Verified: enroll, verify, PAM/sudo integration,
systemd hardening, D-Bus access control, install/remove/purge lifecycle, suspend/resume.

| Step | Component | Status |
|------|-----------|--------|
| 1 | Camera capture pipeline (`visage-hw`) | **Complete** |
| 2 | ONNX inference — SCRFD + ArcFace (`visage-core`) | **Complete** |
| 3 | Daemon + D-Bus + SQLite model store (`visaged`) | **Complete** |
| 4 | PAM module + system bus migration (`pam-visage`) | **Complete** |
| 5 | IR emitter integration (`visage-hw`) | **Complete** |
| 6 | Ubuntu packaging & system integration | **Complete** |

Not yet suitable for production use — see [Known Limitations](docs/STATUS.md#known-limitations-at-v03).

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
sudo apt install ./visage_0.3.0_amd64.deb
sudo visage setup                          # download ONNX models (~182 MB)
sudo visage enroll --label default         # enroll your face
sudo echo "face auth works"                # test — face first, password fallback
```

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

Then download models and enroll:

```bash
sudo visage setup
sudo visage enroll --label default
```

The NixOS module handles systemd, D-Bus policy, and PAM integration declaratively.
See `packaging/nix/module.nix` for all options (`modelDir`, `camera`, `similarityThreshold`, etc.).

### Arch Linux (AUR)

```bash
git clone https://aur.archlinux.org/visage.git
cd visage && makepkg -si
sudo visage setup
sudo visage enroll --label default
```

PAM requires a manual one-line edit on Arch — add before `pam_unix.so` in
`/etc/pam.d/system-auth`:

```
auth  [success=end default=ignore]  pam_visage.so
```

### What the package does

- Installs `visaged` (daemon), `visage` (CLI), and `pam_visage.so` (PAM module)
- Enables the `visaged` systemd service and `visage-resume.service` (suspend/resume)
- Configures PAM (automatic on Ubuntu/NixOS, manual on Arch)

## Usage

```bash
# Enroll your face
sudo visage enroll --label default

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
| **Supported** | UVC (`uvcvideo` driver) | ✅ Full support | ASUS ZenBook, ThinkPad T/X (pre-Gen 11), HP EliteBook (UVC configs), Dell Latitude (UVC configs), TUXEDO InfinityBook |
| **Not supported** | Intel IPU6 / MIPI / libcamera | ❌ Not yet | Newer Dell XPS, ThinkPad Gen 11+ (some configs), Intel "AI PC" cameras |
| **No IR camera** | N/A | — | Framework, System76, Purism |

**Not sure which your laptop has?** Run `visage discover` — it detects the kernel
driver for each `/dev/video*` device and warns if an IPU6 camera is found.

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

Latency: ~1.4s on USB webcam + CPU-only ONNX. Expected <500ms with IR camera + GPU.

Bugs fixed during testing: [DeviceAllow glob](docs/STATUS.md#bugs-found-during-testing),
[tokio::time::timeout panic in zbus context](docs/STATUS.md#bugs-found-during-testing).

## Documentation

- [Operations Guide](docs/operations-guide.md) ← start here: installation, configuration, troubleshooting
- [Hardware Compatibility](docs/hardware-compatibility.md) ← supported cameras, tiers, quirks
- [Release Status & Known Limitations](docs/STATUS.md)
- [Architecture](docs/architecture.md)
- [Threat Model](docs/threat-model.md)
- [Architecture Decisions](docs/decisions/) ← 10 ADRs covering implementation, security, and governance decisions

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
