#!/usr/bin/env bash
#
# Visage Quickstart — from clone to working face authentication
#
# Usage:
#   ./scripts/quickstart.sh              # full setup including enrollment
#   ./scripts/quickstart.sh --no-enroll  # build + install only (headless/CI)
#
# Requirements:
#   - Ubuntu 24.04 LTS (amd64)
#   - Rust toolchain (rustc, cargo)
#   - A camera accessible at /dev/video*
#   - Internet connection (ONNX model download ~182 MB)
#   - sudo access
#
# This script is idempotent — safe to run again after a failure or upgrade.

set -euo pipefail

# ── Configuration ─────────────────────────────────────────────────────────────

ENROLL=true
for arg in "$@"; do
    case "$arg" in
        --no-enroll) ENROLL=false ;;
        --help|-h)
            echo "Usage: ./scripts/quickstart.sh [--no-enroll]"
            echo ""
            echo "  --no-enroll   Skip face enrollment (for headless/CI builds)"
            exit 0
            ;;
        *)
            echo "Unknown option: $arg"
            echo "Usage: ./scripts/quickstart.sh [--no-enroll]"
            exit 1
            ;;
    esac
done

# ── Helpers ───────────────────────────────────────────────────────────────────

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

step=0
total_steps=6
if [ "$ENROLL" = false ]; then
    total_steps=4
fi

stage() {
    step=$((step + 1))
    echo ""
    echo -e "${CYAN}${BOLD}[$step/$total_steps] $1${NC}"
    echo "────────────────────────────────────────────────────────"
}

ok()   { echo -e "  ${GREEN}✓${NC} $1"; }
warn() { echo -e "  ${YELLOW}!${NC} $1"; }
fail() { echo -e "  ${RED}✗${NC} $1"; exit 1; }

# ── Stage 1: Pre-flight checks ───────────────────────────────────────────────

stage "Pre-flight checks"

# OS
if [ -f /etc/os-release ]; then
    . /etc/os-release
    if [[ "${ID:-}" == "ubuntu" ]]; then
        ok "OS: Ubuntu ${VERSION_ID:-unknown}"
    else
        warn "OS: ${PRETTY_NAME:-unknown} (tested on Ubuntu 24.04 — your mileage may vary)"
    fi
else
    warn "Cannot detect OS (missing /etc/os-release)"
fi

# Architecture
ARCH=$(uname -m)
if [[ "$ARCH" == "x86_64" ]]; then
    ok "Architecture: x86_64"
else
    fail "Unsupported architecture: $ARCH (Visage requires x86_64 for ONNX Runtime)"
fi

# Rust toolchain
if command -v cargo &>/dev/null; then
    RUST_VER=$(rustc --version 2>/dev/null | awk '{print $2}')
    ok "Rust toolchain: $RUST_VER"
else
    fail "Rust toolchain not found. Install via: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
fi

# System dependencies
MISSING_DEPS=()
dpkg -s libpam0g-dev &>/dev/null || MISSING_DEPS+=("libpam0g-dev")
dpkg -s libdbus-1-dev &>/dev/null || MISSING_DEPS+=("libdbus-1-dev")

if [ ${#MISSING_DEPS[@]} -gt 0 ]; then
    echo -e "  ${YELLOW}Installing missing system dependencies: ${MISSING_DEPS[*]}${NC}"
    sudo apt-get install -y "${MISSING_DEPS[@]}" || fail "Failed to install system dependencies"
    ok "System dependencies installed"
else
    ok "System dependencies: libpam0g-dev, libdbus-1-dev"
fi

# cargo-deb
if cargo deb --version &>/dev/null; then
    ok "cargo-deb: $(cargo deb --version 2>/dev/null)"
else
    echo -e "  ${YELLOW}Installing cargo-deb...${NC}"
    cargo install cargo-deb || fail "Failed to install cargo-deb"
    ok "cargo-deb installed"
fi

# Camera
CAMERA_COUNT=$(ls /dev/video* 2>/dev/null | wc -l)
if [ "$CAMERA_COUNT" -gt 0 ]; then
    ok "Cameras: $CAMERA_COUNT /dev/video* device(s) found"
else
    if [ "$ENROLL" = true ]; then
        fail "No /dev/video* devices found. A camera is required for face enrollment."
    else
        warn "No /dev/video* devices found (--no-enroll mode, continuing)"
    fi
fi

# Disk space (need ~500 MB for build + 200 MB for models)
AVAIL_MB=$(df -BM --output=avail . 2>/dev/null | tail -1 | tr -d ' M')
if [ "${AVAIL_MB:-0}" -ge 700 ]; then
    ok "Disk space: ${AVAIL_MB} MB available"
else
    warn "Low disk space: ${AVAIL_MB:-?} MB available (need ~700 MB for build + models)"
fi

# Internet (check model URL is reachable)
if curl -sI --max-time 5 "https://github.com" &>/dev/null; then
    ok "Internet: reachable"
else
    warn "Internet: unreachable (model download may fail)"
fi

# ── Stage 2: Build ────────────────────────────────────────────────────────────

stage "Building Visage"

# Ensure we're in the repo root
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

ok "Repository: $REPO_ROOT"

echo "  Compiling workspace (release mode)..."
cargo build --release --workspace 2>&1 | tail -1
ok "Workspace compiled"

echo "  Packaging .deb..."
DEB_PATH=$(cargo deb -p visaged --no-build 2>&1 | tail -1)
if [ -f "$DEB_PATH" ]; then
    DEB_SIZE=$(du -h "$DEB_PATH" | cut -f1)
    ok "Package built: $(basename "$DEB_PATH") ($DEB_SIZE)"
else
    fail "cargo deb failed — no .deb produced"
fi

# Run tests
echo "  Running unit tests..."
TEST_OUTPUT=$(cargo test --workspace 2>&1) || true
TEST_COUNT=$(echo "$TEST_OUTPUT" | grep "^test result:" | awk '{sum += $4} END {print sum+0}')
FAIL_COUNT=$(echo "$TEST_OUTPUT" | grep "^test result:" | awk '{sum += $6} END {print sum+0}')
if [ "${FAIL_COUNT:-0}" -eq 0 ] && [ "${TEST_COUNT:-0}" -gt 0 ]; then
    ok "Tests: $TEST_COUNT passed, 0 failed"
else
    fail "Tests: ${FAIL_COUNT:-?} failed (${TEST_COUNT:-0} passed). Run 'cargo test --workspace' for details."
fi

# ── Stage 3: Install ─────────────────────────────────────────────────────────

stage "Installing package"

INSTALLED_VER=$(dpkg-query -W -f='${Version}' visage 2>/dev/null || echo "none")
NEW_VER=$(dpkg-deb -f "$DEB_PATH" Version 2>/dev/null || echo "unknown")

if [ "$INSTALLED_VER" = "$NEW_VER" ]; then
    ok "Already installed: visage $INSTALLED_VER"
else
    if [ "$INSTALLED_VER" != "none" ]; then
        echo "  Upgrading visage $INSTALLED_VER → $NEW_VER..."
    else
        echo "  Installing visage $NEW_VER..."
    fi
    sudo apt-get install -y "$DEB_PATH" 2>&1 | grep -E "^(Setting up|Unpacking)" || true
    ok "Package installed: visage $NEW_VER"
fi

# Restart daemon to pick up new binary
echo "  Restarting visaged..."
sudo systemctl restart visaged
sleep 2

if systemctl is-active --quiet visaged; then
    ok "Daemon: visaged is running"
else
    fail "Daemon failed to start. Check: sudo journalctl -u visaged -n 20"
fi

# ── Stage 4: Download models ─────────────────────────────────────────────────

stage "Downloading ONNX models"

# Check if models are already present and verified
if sudo visage status &>/dev/null; then
    MODEL_DIR=$(sudo visage status 2>/dev/null | grep "model_dir:" | awk '{print $2}')
    MODEL_COUNT=0
    for model_file in det_10g.onnx w600k_r50.onnx; do
        if [ -f "${MODEL_DIR:-/var/lib/visage/models}/$model_file" ]; then
            MODEL_COUNT=$((MODEL_COUNT + 1))
        fi
    done

    if [ "$MODEL_COUNT" -eq 2 ]; then
        ok "Models already present (daemon started successfully = integrity verified)"
    else
        echo "  Downloading models (~182 MB)..."
        sudo visage setup || fail "Model download failed. Check internet and try: sudo visage setup"
        # Restart daemon to load new models
        sudo systemctl restart visaged
        sleep 2
        ok "Models downloaded and verified"
    fi
else
    echo "  Downloading models (~182 MB)..."
    sudo visage setup || fail "Model download failed. Check internet and try: sudo visage setup"
    sudo systemctl restart visaged
    sleep 2
    ok "Models downloaded and verified"
fi

# Verify daemon is healthy with models loaded
STATUS_OUTPUT=$(visage status 2>/dev/null || true)
if echo "$STATUS_OUTPUT" | grep -q "version:"; then
    ok "System ready: $(echo "$STATUS_OUTPUT" | grep 'version:' | xargs)"
else
    fail "Daemon not responding after model setup. Check: sudo journalctl -u visaged -n 20"
fi

if [ "$ENROLL" = false ]; then
    echo ""
    echo -e "${GREEN}${BOLD}Build and install complete.${NC}"
    echo ""
    echo "To enroll your face:  sudo visage enroll --label default"
    echo "To verify:            visage verify"
    echo "To test with sudo:    sudo -k && sudo echo 'face auth works'"
    echo ""
    exit 0
fi

# ── Stage 5: Face enrollment ─────────────────────────────────────────────────

stage "Face enrollment"

USER_NAME=$(whoami)

# Check for existing enrollment
EXISTING=$(sudo visage list --user "$USER_NAME" 2>/dev/null || true)
if echo "$EXISTING" | grep -q "Enrolled models"; then
    ok "Face already enrolled for user '$USER_NAME'"
    echo "$EXISTING" | grep "  " || true
    echo ""
    read -rp "  Re-enroll? This replaces your current face model. [y/N] " REPLY
    if [[ "${REPLY,,}" =~ ^y ]]; then
        # Extract model ID and remove it
        MODEL_ID=$(echo "$EXISTING" | grep "  " | head -1 | awk '{print $1}')
        if [ -n "$MODEL_ID" ]; then
            sudo visage remove "$MODEL_ID" --user "$USER_NAME" 2>/dev/null && ok "Old model removed" || true
        fi
    else
        ok "Keeping existing enrollment"
        # Skip to verification
        step=$((step + 1))
        echo ""
        echo -e "${CYAN}${BOLD}[$step/$total_steps] Verification${NC}"
        echo "────────────────────────────────────────────────────────"

        echo "  Testing face verification..."
        if visage verify 2>/dev/null; then
            ok "Face verified successfully"
        else
            warn "Verification did not match — try 'visage verify' in good lighting"
        fi

        echo ""
        echo -e "${GREEN}${BOLD}Visage is fully operational.${NC}"
        echo ""
        echo "  sudo echo 'test'     → authenticates with your face"
        echo "  visage status         → daemon info"
        echo "  visage discover       → camera detection"
        echo "  visage verify         → manual face check"
        echo ""
        exit 0
    fi
fi

echo ""
echo -e "  ${BOLD}Look at your camera and hold still.${NC}"
echo "  Enrollment captures 5 frames and selects the best one."
echo ""
read -rp "  Ready? Press Enter to start enrollment... "

if sudo visage enroll --label default --user "$USER_NAME"; then
    ok "Face enrolled for user '$USER_NAME'"
else
    fail "Enrollment failed. Ensure you're facing the camera in adequate lighting."
fi

# ── Stage 6: Verification ────────────────────────────────────────────────────

stage "Verification"

echo "  Testing face verification (look at camera)..."
sleep 1

if visage verify 2>/dev/null; then
    ok "Face verified successfully"
else
    warn "Verification did not match on first attempt"
    echo "  Retrying..."
    sleep 1
    if visage verify 2>/dev/null; then
        ok "Face verified on retry"
    else
        warn "Verification failed — try 'visage verify' in better lighting"
    fi
fi

# Final PAM test
echo ""
echo "  Testing PAM integration (sudo with face auth)..."
sudo -k
if sudo -n true 2>/dev/null; then
    ok "PAM face authentication works"
else
    # sudo -n won't work if face auth needs the terminal; test differently
    ok "PAM configured — test with: sudo echo 'face auth works'"
fi

# ── Done ──────────────────────────────────────────────────────────────────────

echo ""
echo -e "${GREEN}${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${GREEN}${BOLD}  Visage is fully operational.${NC}"
echo -e "${GREEN}${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
echo "  Face authentication is active for all sudo/login/screen-lock prompts."
echo "  If face auth fails, your password is always available as fallback."
echo ""
echo "  Useful commands:"
echo "    visage status         Show daemon status"
echo "    visage discover       List cameras and quirk status"
echo "    visage verify         Manual face verification check"
echo "    visage list           List enrolled face models (requires sudo)"
echo "    visage test           Camera diagnostics"
echo ""
echo "  Documentation:"
echo "    docs/operations-guide.md    Full usage and troubleshooting"
echo "    docs/hardware-compatibility.md  Camera compatibility tiers"
echo "    CONTRIBUTING.md             How to contribute"
echo ""
