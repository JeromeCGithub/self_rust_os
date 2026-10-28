#!/usr/bin/env bash
# Cargo runner: integration tests use bootimage, normal runs boot the desktop.
set -euo pipefail
case "$1" in
    */deps/*) exec bootimage runner "$@" ;;
esac
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "${ROOT}"
./tools/build-grub-disk.sh
# QEMU vmmouse supplies absolute host coordinates; PS/2 remains a fallback.
DESKTOP_DISPLAY="${RUSTOS_DISPLAY:-gtk,show-cursor=off}"
echo "[desktop] Absolute mouse integration enabled; move the pointer over the display."
exec qemu-system-x86_64 -machine pc,i8042=on,vmport=on -drive "format=raw,file=${ROOT}/target/grub-disk.img" \
    -vga std -m 256 -serial stdio -no-reboot -display "${DESKTOP_DISPLAY}"
