# Visage Operations Guide

This guide covers installation, enrollment, day-to-day usage, hardware setup,
troubleshooting, and removal on Ubuntu 24.04.

---

## Requirements

- Ubuntu 24.04 LTS (tested: 24.04.4)
- An IR camera with a Windows Hello-compatible IR emitter, **or** any USB webcam for
  testing (IR camera strongly recommended for production use)
- Root access (`sudo`)
- ~200 MB free disk space for ONNX models

Supported cameras: see [Hardware Compatibility](#hardware-compatibility).

---

## Installation

### From a pre-built .deb

```bash
# 1. Install the package
sudo apt install ./visage_0.1.0_amd64.deb

# 2. Download ONNX models (~182 MB, requires internet)
sudo visage setup

# 3. Enroll your face (run once per user)
sudo visage enroll --label default

# 4. Test
sudo echo "face auth works"
```

After step 4, pressing Enter should authenticate via face recognition. If no face is
detected within 10 seconds, the system falls back to your password prompt.

### Build from source

```bash
# Prerequisites
sudo apt install libpam0g-dev libdbus-1-dev
cargo install cargo-deb   # one-time

# Build and package
cargo build --release --workspace
cargo deb -p visaged --no-build

sudo apt install ./target/debian/visage_*.deb
```

---

## First-Time Setup

### 1. Download models

`visage setup` downloads two ONNX models to `/var/lib/visage/models/`:

| Model | File | Size | Purpose |
|-------|------|------|---------|
| SCRFD | `det_10g.onnx` | 16 MB | Face detection |
| ArcFace | `w600k_r50.onnx` | 166 MB | Face recognition |

SHA-256 checksums are verified on download. Models are sourced from HuggingFace.

```
$ sudo visage setup
Model directory: /var/lib/visage/models
  downloading det_10g.onnx (16 MB)... verifying checksum... ok
  downloading w600k_r50.onnx (166 MB)... verifying checksum... ok

Setup complete: 2 model(s) downloaded, 0 already present.
```

### 2. Verify the daemon is running

```bash
systemctl status visaged
# Should show: active (running)
```

If not running:
```bash
sudo systemctl start visaged
journalctl -u visaged -n 30   # inspect logs
```

### 3. Enroll your face

```bash
# Enroll (requires root — enrollment modifies the face database)
sudo visage enroll --label default
```

Enrollment captures 5 frames, extracts an ArcFace embedding from each, and stores the
average in `/var/lib/visage/faces.db`. The process takes 2–5 seconds.

You can enroll multiple times (different angles, lighting conditions):
```bash
sudo visage enroll --label angled
sudo visage enroll --label glasses
```

---

## Day-to-Day Usage

### Face authentication in sudo

Face auth is automatically active after installation. When you run a command requiring
`sudo`, the system:

1. Activates your IR emitter (if supported)
2. Captures 3 frames and runs face recognition
3. On match: proceeds immediately
4. On no-match or timeout (10s): falls through to your password prompt

No extra steps required. The PAM module is configured system-wide via `pam-auth-update`.

### CLI commands

```bash
# List enrolled face models
visage list

# Verify interactively (exits 0 on match, 1 on no-match)
visage verify

# Show daemon status (models loaded, camera open, uptime)
visage status

# Remove a specific model
sudo visage remove <model-id>    # UUID from visage list
```

---

## Camera Discovery and Diagnostics

### Discover cameras

```bash
visage discover
```

Output:
```
/dev/video2  VID=0x04f2 PID=0xb6d9  quirk: ASUS Zenbook 14 UM3406HA IR Camera ✓
/dev/video0  VID=0x0bda PID=0x5850  no quirk (VID=0x0bda PID=0x5850)
```

Cameras with `✓` have an IR emitter quirk in the database — the emitter will activate
automatically during authentication. Cameras without a quirk still work for face
recognition under ambient light, but authentication quality degrades in dim environments.

### Camera test

```bash
# Test default camera (or specify with --device /dev/videoN)
visage test

# Capture 5 frames and save to /tmp/visage-test/
visage test --frames 5
```

The test command saves grayscale `.pgm` files that you can inspect with any image viewer.
A good IR frame should show a clear face with high contrast. Dark, blurry, or low-contrast
frames indicate poor lighting or emitter problems.

---

## Hardware Compatibility

### Supported pixel formats

| Format | Description | Cameras |
|--------|-------------|---------|
| `GREY` | 8-bit grayscale (native IR) | ASUS Zenbook IR cameras |
| `YUYV` | YUV 4:2:2 (Y channel extracted) | Most USB webcams |
| `Y16` | 16-bit grayscale → downsampled to 8-bit | Many Windows Hello IR cameras |

Format is detected automatically at device open. Unknown formats are rejected with a clear
error message.

### IR emitter support

The emitter quirks database lives in `contrib/hw/`. Currently supported:

| Camera | VID | PID | File |
|--------|-----|-----|------|
| ASUS Zenbook 14 UM3406HA | `0x04F2` | `0xB6D9` | `04f2-b6d9.toml` |

For unsupported cameras, run `visage discover` to get the VID:PID, then follow the
contribution guide at [contrib/hw/README.md](../contrib/hw/README.md).

### Configuring a different camera device

If your IR camera is not at `/dev/video2`, override the device:

```bash
# Find which /dev/videoN is the IR camera
visage discover

# Override via environment variable (add to /etc/default/visaged for persistence)
sudo systemctl edit visaged
```

Add under `[Service]`:
```ini
[Service]
Environment=VISAGE_CAMERA_DEVICE=/dev/video4
```

Then restart: `sudo systemctl restart visaged`

---

## Configuration

All settings are controlled by environment variables set in the service unit. To override,
use `sudo systemctl edit visaged` and add under `[Service]`:

```ini
[Service]
Environment=VARIABLE=value
```

| Variable | Default | Description |
|----------|---------|-------------|
| `VISAGE_CAMERA_DEVICE` | `/dev/video2` | V4L2 device path |
| `VISAGE_MODEL_DIR` | `/var/lib/visage/models` | ONNX model directory |
| `VISAGE_DB_PATH` | `/var/lib/visage/faces.db` | Face embedding database |
| `VISAGE_SIMILARITY_THRESHOLD` | `0.40` | Cosine similarity match threshold (0–1) |
| `VISAGE_VERIFY_TIMEOUT_SECS` | `10` | Max seconds for a verify attempt |
| `VISAGE_FRAMES_PER_VERIFY` | `3` | Frames captured per authentication |
| `VISAGE_FRAMES_PER_ENROLL` | `5` | Frames captured per enrollment |
| `VISAGE_EMITTER_ENABLED` | `1` | Set to `0` to disable IR emitter |
| `VISAGE_SESSION_BUS` | unset | Set to `1` to use session bus (development only) |

### Tuning the similarity threshold

The default threshold of 0.40 is a balanced setting for `w600k_r50`:

| Threshold | False Accept Rate | Use Case |
|-----------|-------------------|----------|
| 0.45 | ~0.01% | High security |
| 0.40 | ~0.1% | Default — home/workstation |
| 0.35 | ~1% | Accessibility (facial hair, glasses changes, aging) |

Lower values increase false accepts. If you're getting frequent false rejections (having
to fall back to password frequently), consider re-enrolling with better lighting, or lower
the threshold to 0.35.

---

## Suspend and Resume

Visage automatically handles suspend/resume via `visage-resume.service`. When the system
wakes from suspend or hibernate, the daemon is restarted to reinitialize the camera and
IR emitter.

To verify this is working:
```bash
systemctl status visage-resume.service
# Should show enabled and the install WantedBy targets
```

If face auth fails after resume, check:
```bash
journalctl -u visaged --since "5 minutes ago"
```

---

## Logs and Diagnostics

### Daemon logs

```bash
# Current daemon logs
journalctl -u visaged -f

# Authentication events (PAM logs go to auth.log)
sudo journalctl -u visaged --since today
sudo grep pam_visage /var/log/auth.log
```

### Enable verbose logging

```bash
sudo systemctl edit visaged
```

Add under `[Service]`:
```ini
[Service]
Environment=RUST_LOG=visaged=debug,visage_core=debug,visage_hw=debug
```

Then `sudo systemctl restart visaged`.

### Checking daemon health

```bash
visage status
```

Output:
```json
{
  "camera": "/dev/video2",
  "models_dir": "/var/lib/visage/models",
  "models": {"det_10g.onnx": true, "w600k_r50.onnx": true},
  "emitter": "active",
  "enrolled_users": ["ccross"]
}
```

---

## Troubleshooting

### `sudo` still asks for password

**Check the PAM configuration:**
```bash
grep pam_visage /etc/pam.d/common-auth
```

Should show: `auth [success=end default=ignore] pam_visage.so`

If missing, run: `sudo pam-auth-update` and enable Visage.

**Check the daemon is running:**
```bash
systemctl is-active visaged
```

If not active: `sudo systemctl start visaged`

**Check enrollment:**
```bash
visage list   # should show your enrolled models
```

If empty, re-enroll: `sudo visage enroll --label default`

---

### `visage enroll` fails: `ServiceUnknown`

The daemon isn't registered on D-Bus yet. Wait 3–5 seconds after `systemctl start visaged`
before enrolling, then try again.

---

### Authentication is slow (>5 seconds)

- CPU-only ONNX inference takes ~60–80ms per frame on a modern CPU.
- Slow authentication usually means many dark frames are being discarded.
- Check: does `visage test` show mostly dark frames? If so, the IR emitter may not be
  activating.

```bash
# Check if your camera has a quirk entry
visage discover

# Test emitter explicitly
visage test --frames 5
# Open /tmp/visage-test/*.pgm — frames should show a well-lit face
```

If the emitter isn't activating, the camera may need a quirk entry.
See [contrib/hw/README.md](../contrib/hw/README.md).

---

### Face auth broken after software update

If a package update caused PAM issues:

**Recover with pkexec (works without going through sudo's PAM):**
```bash
pkexec visage list        # verify daemon is accessible
sudo pam-auth-update      # re-run PAM configuration
```

**If sudo is completely broken:**
```bash
pkexec bash               # open a root shell via polkit
```

---

### Camera not found at /dev/video2

```bash
# List available cameras
visage discover
ls /dev/video*

# Override the device path
sudo systemctl edit visaged
# Add: Environment=VISAGE_CAMERA_DEVICE=/dev/video0
sudo systemctl restart visaged
```

---

## Multi-User Enrollment

Each system user enrolls their own face. Enrollment requires root; verification does not.

```bash
# Enroll as root on behalf of user 'alice'
sudo visage enroll --user alice --label default

# List models for user 'alice'
sudo visage list --user alice
```

The face database stores per-user embeddings; cross-user access is prevented at the
database level (`WHERE user = ?` on all mutations).

---

## Removal

```bash
# Remove binaries, disable service and PAM integration
# Face database and models are PRESERVED
sudo apt remove visage

# Remove everything including face database and models (~182 MB models)
sudo apt purge visage
```

After `apt remove`, `sudo` returns to password-only authentication immediately.
The face database is preserved in case you reinstall.

After `apt purge`, `/var/lib/visage/` is deleted. You will need to re-download models
and re-enroll after reinstalling.

---

## Security Notes

- The face database (`/var/lib/visage/faces.db`) is root-readable only.
  Embeddings are stored as raw float32 vectors and are not encrypted. Full-disk
  encryption (e.g., LUKS) is recommended for sensitive environments.
- The daemon runs as root with a restrictive systemd sandbox (`ProtectSystem=strict`,
  `NoNewPrivileges=true`, `PrivateTmp=true`).
- PAM integration always falls back to password on any error or timeout (`PAM_IGNORE`).
  Visage cannot lock you out of your system.
- For the full threat model, see [threat-model.md](threat-model.md).
