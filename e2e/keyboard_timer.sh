#!/usr/bin/env bash
# E2E for keyboard + timer syscalls (PR: feature/keyboard-timer).
#
# 1. Runs the keyboard_timer integration tests in QEMU.
# 2. Runs the lib unit tests for syscalls, ticks, and keyboard queue.
#
# Usage: ./e2e/keyboard_timer.sh   (run from the repository root)
set -euo pipefail

cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"

echo "[e2e] running keyboard/timer integration tests in QEMU..."
cargo test --test keyboard_timer 2>&1 | tee /tmp/kb-e2e.log

echo ""
echo "[e2e] running syscall/timer/keyboard unit tests..."
cargo test --lib syscall 2>&1 | tail -8
cargo test --lib ticks 2>&1 | tail -3
cargo test --lib keyboard 2>&1 | tail -3

echo ""
echo "[e2e] metrics:"
grep -E "\[timer\]|\[keyboard\]" /tmp/kb-e2e.log || echo "  (no metrics found - check the log above)"
