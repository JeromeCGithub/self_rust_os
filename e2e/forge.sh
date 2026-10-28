#!/usr/bin/env bash
# E2E for the forge user process (PR: feature/forge).
#
# 1. Builds the forge user program.
# 2. Builds the kernel boot image.
# 3. Boots in QEMU, sends keystrokes via monitor, captures screenshot.
#
# Usage: ./e2e/forge.sh   (run from the repository root)
set -euo pipefail

cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"

echo "[e2e] building forge user program..."
./tools/build-user.sh forge

echo "[e2e] building kernel boot image..."
cargo build 2>&1 | tail -1
cargo bootimage 2>&1 | tail -1

BOOTIMAGE="target/x86_64-self-rust-os/debug/bootimage-self_rust_os.bin"

echo "[e2e] running interactive forge test..."
python3 e2e/forge_test.py "$BOOTIMAGE" docs/screenshots/pr5-forge-interactive.png

echo ""
echo "[e2e] done. Screenshots:"
ls -lh docs/screenshots/pr5-forge*.png
