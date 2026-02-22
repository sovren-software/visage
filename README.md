# Visage

A modern, secure biometric authentication framework for Linux.

Visage provides IR camera-based face authentication for Linux systems via PAM,
analogous to Windows Hello. Built in Rust for memory safety in the authentication path.

## Status

| Step | Component | Status |
|------|-----------|--------|
| 1 | Camera capture pipeline (`visage-hw`) | **Complete** |
| 2 | ONNX inference — SCRFD + ArcFace (`visage-core`) | **Complete** |
| 3 | Daemon + D-Bus + SQLite model store (`visaged`) | **Complete** |
| 4 | PAM module + system bus migration (`pam-visage`) | **Complete** |
| 5 | IR emitter integration (`visage-hw`) | **Complete** |
| 6 | Ubuntu packaging & system integration | **Complete** |

Not yet suitable for production use.

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

## Installation (Ubuntu 24.04)

### Install from .deb

```bash
# Install the package
sudo apt install ./visage_0.1.0_amd64.deb

# Download face detection models (~182 MB)
sudo visage setup

# Enroll your face
sudo visage enroll --label default

# Test — should authenticate via face, falls back to password on failure
sudo echo "face auth works"
```

### What the package does

- Installs `visaged` (daemon), `visage` (CLI), and `pam_visage.so` (PAM module)
- Enables the `visaged` systemd service
- Configures PAM via `pam-auth-update` (face auth before password)

### Removal

```bash
sudo apt remove visage     # removes binaries, disables PAM and service
sudo apt purge visage      # also removes /var/lib/visage (models + face database)
```

After removal, `sudo` returns to password-only authentication.

### Build from source

```bash
cargo build --release --workspace
cargo deb -p visaged --no-build
```

Requires `cargo-deb`: `cargo install cargo-deb`

## Usage

**Prerequisites:** Download ONNX models via `visage setup` (or manually per `models/README.md`)
and start the daemon.

```bash
# Start the daemon on the system bus (required for PAM)
# Use VISAGE_SESSION_BUS=1 to run on the session bus for development without sudo
sudo visaged

# Enroll your face
visage enroll --label default

# Verify your face (exits 0 on match, 1 on no-match — shell-friendly)
visage verify

# List enrolled models
visage list

# Remove a model
visage remove <model-id>

# Show daemon status
visage status
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

Tested on ASUS Zenbook 14 UM3406HA (AMD Ryzen AI, `/dev/video2` IR camera).

The camera outputs native GREY format (640×360, 1 byte/pixel). Both GREY and
YUYV pixel formats are supported; format is detected automatically at device open.

Hardware quirks (IR emitter UVC control bytes) are tracked in `contrib/hw/`.
See [contrib/hw/README.md](contrib/hw/README.md) for the contribution process.

## Documentation

- [Strategy — v2 to v3 Growth Map](docs/STRATEGY.md) ← start here
- [Release Status & Remaining Work](docs/STATUS.md)
- [Architecture](docs/architecture.md)
- [Threat Model](docs/threat-model.md)
- [Architecture Review and Roadmap](docs/research/architecture-review-and-roadmap.md)
- [v3 Vision — Forward-Looking Architecture](docs/research/v3-vision.md)
- [Domain Audit — Technical Coverage and Knowledge Gaps](docs/research/domain-audit.md)
- [Step 1 ADR — Camera Capture Pipeline](docs/decisions/001-camera-capture-pipeline.md)
- [Step 2 ADR — ONNX Inference KB and Blocker Resolution](docs/decisions/002-onnx-inference-kb-and-blocker-resolution.md)
- [Step 3 ADR — Daemon Integration Architecture](docs/decisions/003-daemon-integration.md)
- [Step 4 ADR — ONNX Inference Pipeline Implementation](docs/decisions/004-inference-pipeline-implementation.md)
- [Step 4 ADR — PAM Module and System Bus Migration](docs/decisions/005-pam-system-bus-migration.md)
- [Step 5 ADR — IR Emitter Integration](docs/decisions/006-ir-emitter-integration.md)
- [Step 6 ADR — Ubuntu Packaging](docs/decisions/007-ubuntu-packaging.md)

## License

MIT
