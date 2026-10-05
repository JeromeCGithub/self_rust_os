#!/usr/bin/env bash
# E2E for the ATA PIO driver (PR: feature/ata-driver).
#
# 1. Ensures the 16 MiB test disk fixture exists.
# 2. Runs the QEMU integration tests (`cargo test --test ata`).
# 3. Prints the serial-output metrics (identify + round-trip checksum).
#
# Usage: ./e2e/ata.sh   (run from the repository root)
set -euo pipefail

cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"

FIXTURE="tests/fixtures/disk-ata.img"
if [ ! -f "$FIXTURE" ]; then
    echo "[e2e] creating $FIXTURE (16 MiB, zeroed)..."
    mkdir -p tests/fixtures
    dd if=/dev/zero of="$FIXTURE" bs=1M count=16 status=none
fi

echo "[e2e] running ATA integration tests in QEMU..."
cargo test --test ata 2>&1 | tee /tmp/ata-e2e.log

echo ""
echo "[e2e] metrics:"
grep -E "\[ata\]" /tmp/ata-e2e.log || echo "  (no metrics found - check the log above)"
