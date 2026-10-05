#!/usr/bin/env bash
# E2E for the terminal user process (PR: feature/terminal).
#
# 1. Builds the terminal user program.
# 2. Builds the kernel boot image.
# 3. Boots in QEMU, sends keystrokes via monitor, captures screenshot.
#
# Usage: ./e2e/terminal.sh   (run from the repository root)
set -euo pipefail

cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"

echo "[e2e] building terminal user program..."
(cd user_programs/terminal && ./build.sh)

echo "[e2e] building kernel boot image..."
cargo build 2>&1 | tail -1
cargo bootimage 2>&1 | tail -1

BOOTIMAGE="target/x86_64-self-rust-os/debug/bootimage-self_rust_os.bin"

echo "[e2e] running interactive terminal test..."
python3 e2e/terminal_test.py "$BOOTIMAGE" docs/screenshots/pr5-terminal-interactive.png

echo ""
echo "[e2e] done. Screenshots:"
ls -lh docs/screenshots/pr5-terminal*.png
