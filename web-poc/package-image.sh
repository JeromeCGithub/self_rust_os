#!/usr/bin/env bash
# Build the self_rust_os boot disk and stage it for the browser POC.
#
# Steps:
#   1. Build the user programs (wm.bin must exist before the kernel
#      build: the kernel embeds it with include_bytes!).
#   2. Build the GRUB2 disk image with the repo's own script.
#   3. Install the user programs under /BIN/ on the FAT32 partition.
#      tools/build-grub-disk.sh does not do this yet; the window
#      manager spawns /bin/PORTAL.BIN, so the files must be there for
#      anything beyond the initial WM boot.
#   4. Copy the image to htdocs/pack-src/grub-disk.img, where the POC
#      page fetches it and injects it into the emulator filesystem.
#
# Usage: ./package-image.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
IMG="${ROOT}/target/grub-disk.img"
PART_OFFSET=$((2048 * 512))   # FAT32 partition start (LBA 2048)

cd "${ROOT}"

echo "[package] Building user programs..."
./tools/build-user.sh wm portal snake forge hello

echo "[package] Building GRUB disk image..."
./tools/build-grub-disk.sh

echo "[package] Installing user programs under /BIN/..."
mmd -i "${IMG}@@${PART_OFFSET}" ::/BIN 2>/dev/null || true
for prog in portal forge snake hello; do
    mcopy -o -i "${IMG}@@${PART_OFFSET}" \
        "user_programs/${prog}/${prog}.bin" "::/BIN/$(echo "${prog}" | tr '[:lower:]' '[:upper:]').BIN"
done

echo "[package] Staging image into htdocs/pack-src/..."
mkdir -p "${SCRIPT_DIR}/htdocs/pack-src"
cp "${IMG}" "${SCRIPT_DIR}/htdocs/pack-src/grub-disk.img"
ls -la "${SCRIPT_DIR}/htdocs/pack-src/grub-disk.img"
echo "[package] Done."
