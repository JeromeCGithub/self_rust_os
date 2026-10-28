#!/usr/bin/env bash
# End-to-end test for the GRUB2 / Multiboot2 boot path.
#
# Builds the GRUB disk image, boots it in QEMU, asserts the deterministic
# serial metrics for every boot stage, and captures a screenshot.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

IMG="${ROOT}/target/grub-disk.img"
SCREENSHOT="${ROOT}/docs/screenshots/pr-grub-boot.png"

echo "[e2e] Building GRUB disk image..."
"${ROOT}/tools/build-grub-disk.sh" "${IMG}"

echo "[e2e] Running GRUB boot test..."
python3 "${ROOT}/e2e/grub_test.py" "${IMG}" "${SCREENSHOT}"
