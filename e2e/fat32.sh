#!/usr/bin/env bash
# E2E for the FAT32 filesystem (PR: feature/fat32).
#
# 1. Ensures the 16 MiB test disk fixture exists.
# 2. Runs the QEMU integration tests (`cargo test --test fat32`).
# 3. Prints the serial-output metrics.
#
# Usage: ./e2e/fat32.sh   (run from the repository root)
set -euo pipefail

cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"

FIXTURE="tests/fixtures/disk-fat32.img"
if [ ! -f "$FIXTURE" ]; then
    echo "[e2e] creating $FIXTURE (16 MiB, zeroed)..."
    mkdir -p tests/fixtures
    dd if=/dev/zero of="$FIXTURE" bs=1M count=16 status=none
fi

echo "[e2e] running FAT32 integration tests in QEMU..."
cargo test --test fat32 2>&1 | tee /tmp/fat32-e2e.log

echo ""
echo "[e2e] metrics:"
grep -E "\[fat32\]" /tmp/fat32-e2e.log || echo "  (no metrics found - check the log above)"
