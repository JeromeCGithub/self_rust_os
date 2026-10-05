#!/usr/bin/env bash
# E2E for the framebuffer graphics driver (PR: feature/gfx-console).
#
# 1. Runs the gfx integration tests in QEMU (primitives + console).
# 2. Runs the lib unit tests for gfx and console.
# 3. Builds the graphics demo and captures a screenshot.
#
# Usage: ./e2e/gfx.sh   (run from the repository root)
set -euo pipefail

cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"

echo "[e2e] running gfx integration tests in QEMU..."
cargo test --test gfx 2>&1 | tee /tmp/gfx-e2e.log

echo ""
echo "[e2e] running gfx/console unit tests..."
cargo test --lib gfx 2>&1 | tail -5
cargo test --lib console 2>&1 | tail -5

echo ""
echo "[e2e] building graphics demo and capturing screenshot..."
cargo build --test gfx_demo 2>&1 | tail -1
# Generate the bootimage by running the demo (it loops forever; timeout is expected).
timeout 60 cargo test --test gfx_demo > /tmp/gfx-demo.log 2>&1 || true
DEMO_BIN=$(ls -t target/x86_64-self-rust-os/debug/deps/bootimage-gfx_demo-*.bin | head -1)
echo "[e2e] demo bootimage: $DEMO_BIN"
mkdir -p docs/screenshots
python3 e2e/screenshot.py "$DEMO_BIN" docs/screenshots/pr3-gfx-demo.png 6.0

echo ""
echo "[e2e] metrics:"
grep -E "\[gfx\]" /tmp/gfx-e2e.log || echo "  (no metrics found - check the log above)"
echo "[e2e] screenshot: docs/screenshots/pr3-gfx-demo.png"
