#!/usr/bin/env bash
# E2E for the Snake game (PR: feature/snake).
#
# 1. Builds the snake user program.
# 2. Builds the kernel boot image.
# 3. Boots in QEMU and captures gameplay screenshots.
#
# Usage: ./e2e/snake.sh   (run from the repository root)
set -euo pipefail

cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"

echo "[e2e] building snake user program..."
./tools/build-user.sh snake

echo "[e2e] building kernel boot image..."
cargo build 2>&1 | tail -1
cargo bootimage 2>&1 | tail -1

BOOTIMAGE="target/x86_64-self-rust-os/debug/bootimage-self_rust_os.bin"

echo "[e2e] capturing gameplay screenshot (4s)..."
python3 e2e/screenshot.py "$BOOTIMAGE" docs/screenshots/pr6-snake-game.png 4.0

echo "[e2e] capturing game-over screenshot (8s)..."
python3 e2e/screenshot.py "$BOOTIMAGE" docs/screenshots/pr6-snake.png 8.0

echo ""
echo "[e2e] done. Screenshots:"
ls -lh docs/screenshots/pr6-snake*.png
