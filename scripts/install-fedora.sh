#!/usr/bin/env bash
# Visage install for Fedora (manual install, no RPM packaging needed).
#
# Usage:
#   ./scripts/install-fedora.sh                 # build + install + enable daemon
#   ./scripts/install-fedora.sh --no-build      # install from existing target/release
#   ./scripts/install-fedora.sh --no-enroll     # skip the onboard prompt at the end
#   ./scripts/install-fedora.sh --help
#
# Requirements:
#   - Fedora 40+ (tested: 44) on x86_64
#   - Rust toolchain (rustc, cargo)
#   - sudo access, internet (ONNX models ~182 MB)
#
# What it does (idempotent, safe to re-run):
#   1. install deps: pam-devel, clang-devel (bindgen/libclang for v4l2-sys-mit)
#   2. cargo build --release (visaged, visage, visage-enroll, pam-visage)
#   3. install binaries to /usr/local/bin, PAM module to /usr/lib64/security
#   4. install D-Bus system policy + visage-has-session helper (GNOME keyring gate)
#   5. write /etc/systemd/system/visaged.service + visage-resume.service, enable them
#   6. restorecon SELinux labels, download models, print PAM + onboard next steps
#
# What it does NOT do (by design):
#   - never touches /etc/pam.d — authselect owns system-auth on Fedora and
#     overwrites edits there. PAM wiring is a manual, reviewed step (see docs/fedora.md).
set -euo pipefail

BUILD=true
ENROLL_PROMPT=true
for arg in "$@"; do
  case "$arg" in
    --no-build) BUILD=false ;;
    --no-enroll) ENROLL_PROMPT=false ;;
    --help|-h)
      sed -n '2,12p' "$0"
      exit 0
      ;;
    *)
      echo "Unknown option: $arg (try --help)" >&2
      exit 1
      ;;
  esac
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$SCRIPT_DIR/.." && pwd)"
BIN_SRC="$REPO/target/release"
cd "$REPO"

step() { echo -e "\n[$1] $2"; }

step 1 "System dependencies (Fedora)"
if command -v dnf &>/dev/null; then
  sudo dnf install -y pam-devel clang-devel
else
  echo "dnf not found — install pam-devel + clang-devel manually" >&2
fi

step 2 "Build (release)"
if [ "$BUILD" = true ]; then
  cargo build --release -p visaged -p visage-cli -p visage-tui -p pam-visage
else
  echo "skipped (--no-build)"
fi
for bin in visaged visage visage-enroll libpam_visage.so; do
  [ -f "$BIN_SRC/$bin" ] || { echo "missing $BIN_SRC/$bin — run without --no-build" >&2; exit 1; }
done

step 3 "Install files"
sudo mkdir -p /var/lib/visage/models
sudo install -m755 "$BIN_SRC/visaged" /usr/local/bin/visaged
sudo install -m755 "$BIN_SRC/visage" /usr/local/bin/visage
sudo install -m755 "$BIN_SRC/visage-enroll" /usr/local/bin/visage-enroll
sudo install -m644 "$BIN_SRC/libpam_visage.so" /usr/lib64/security/pam_visage.so
sudo install -m644 "$REPO/packaging/dbus/org.freedesktop.Visage1.conf" \
  /usr/share/dbus-1/system.d/org.freedesktop.Visage1.conf
sudo install -D -m755 "$REPO/contrib/pam/visage-has-session" \
  /usr/local/libexec/visage-has-session

step 4 "systemd units"
# NOTE: /usr/local/bin (not /usr/bin) — the RPM layout uses /usr/bin, this
# script is the no-packaging path. Liveness 0.1 is the measured default for
# the 3277:0055 strobe sensor (see the UM3406HA hardware report); the daemon
# default 0.8 rejects ~1 in 6 genuine logins on that module.
sudo tee /etc/systemd/system/visaged.service > /dev/null <<'EOF'
[Unit]
Description=Visage biometric authentication daemon
After=dbus.service
Requires=dbus.service
[Service]
Type=simple
ExecStart=/usr/local/bin/visaged
Restart=on-failure
RestartSec=5
TimeoutStopSec=10s
Environment=VISAGE_MODEL_DIR=/var/lib/visage/models
Environment=VISAGE_DB_PATH=/var/lib/visage/faces.db
Environment=VISAGE_CAMERA_DEVICE=/dev/video2
Environment=VISAGE_LIVENESS_MIN_DISPLACEMENT=0.1
Environment=RUST_LOG=visaged=info
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
DeviceAllow=char-video4linux rw
ReadWritePaths=/var/lib/visage
CapabilityBoundingSet=
SystemCallArchitectures=native
PrivateNetwork=true
MemoryDenyWriteExecute=false
[Install]
WantedBy=multi-user.target
EOF
sudo tee /etc/systemd/system/visage-resume.service > /dev/null <<'EOF'
[Unit]
Description=Restart visaged after suspend/hibernate
After=suspend.target hibernate.target hybrid-sleep.target suspend-then-hibernate.target
[Service]
Type=oneshot
ExecStart=/usr/bin/systemctl restart visaged.service
[Install]
WantedBy=suspend.target hibernate.target hybrid-sleep.target suspend-then-hibernate.target
EOF
sudo systemctl daemon-reload
sudo systemctl enable visaged.service visage-resume.service
if systemctl list-unit-files | grep -q '^dbus-broker\.service'; then
  sudo systemctl reload dbus-broker.service || true
else
  sudo systemctl reload dbus.service || true
fi

step 5 "SELinux labels"
sudo restorecon -v /usr/local/bin/visaged /usr/local/bin/visage \
  /usr/local/bin/visage-enroll /usr/lib64/security/pam_visage.so \
  /usr/local/libexec/visage-has-session || true

step 6 "Models + daemon start"
sudo /usr/local/bin/visage setup
sudo systemctl restart visaged.service
sleep 2
sudo /usr/local/bin/visage status

echo
echo "Done. PAM is intentionally NOT wired — do that by hand (see docs/fedora.md):"
echo "  1. sudo cp /etc/pam.d/sudo /etc/pam.d/sudo.bak-visage   # keep a root shell open"
echo "  2. add as line 2 of /etc/pam.d/sudo:"
echo "       auth [success=done default=ignore] pam_visage.so"
echo "  3. sudo -k && sudo true                                 # face or password fallback"
if [ "$ENROLL_PROMPT" = true ]; then
  echo "  4. sudo /usr/local/bin/visage onboard"
fi
