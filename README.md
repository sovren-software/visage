# Visage

A modern, secure biometric authentication framework for Linux.

Visage provides IR camera-based face authentication for Linux systems via PAM,
analogous to Windows Hello. Built in Rust for memory safety in the authentication path.

## Status

| Step | Component | Status |
|------|-----------|--------|
| 1 | Camera capture pipeline (`visage-hw`) | **Complete** |
| 2 | ONNX inference — SCRFD + ArcFace (`visage-core`) | Pending |
| 3 | Daemon + D-Bus + SQLite model store (`visaged`) | Pending |
| 4 | PAM module (`pam-visage`) | Pending |
| 5 | IR emitter integration (`visage-hw`) | Pending |
| 6 | Ubuntu packaging | Pending |

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

## Usage

### Camera diagnostics

```bash
# Test IR camera (default /dev/video2)
visage test

# Specify device and frame count
visage test --device /dev/video0 --frames 5
```

Captures frames, applies dark-frame filtering and CLAHE contrast enhancement,
saves grayscale PGM files to `/tmp/visage-test/`, and prints a summary.

**Note:** Most frames will be dark until the IR emitter is activated (Step 5).

## Hardware Support

Tested on ASUS Zenbook 14 UM3406HA (AMD Ryzen AI, `/dev/video2` IR camera).

The camera outputs native GREY format (640×360, 1 byte/pixel). Both GREY and
YUYV pixel formats are supported; format is detected automatically at device open.

Hardware quirks (IR emitter UVC control bytes) are tracked in `contrib/hw/`.
See [contrib/hw/README.md](contrib/hw/README.md) for the contribution process.

## Documentation

- [Strategy — v2 to v3 Growth Map](docs/STRATEGY.md) ← start here
- [Domain Audit — Technical Coverage and Knowledge Gaps](docs/research/domain-audit.md)
- [Architecture](docs/architecture.md)
- [Threat Model](docs/threat-model.md)
- [Architecture Review and Roadmap](docs/research/architecture-review-and-roadmap.md)
- [v3 Vision — Forward-Looking Architecture](docs/research/v3-vision.md)
- [Step 1 ADR — Camera Capture Pipeline](docs/decisions/001-camera-capture-pipeline.md)

## License

MIT
