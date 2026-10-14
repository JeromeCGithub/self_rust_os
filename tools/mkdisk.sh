#!/usr/bin/env bash
# Builds a complete bootable disk setup for self_rust_os.
#
# This script produces everything needed to boot the OS and load user
# programs from disk (instead of the kernel embedding them via include_bytes!):
#
#   1. Builds all user-space programs (hello.bin, snake.bin, terminal.bin, ...)
#      using tools/build-user.sh when available, falling back to the
#      per-program user_programs/*/build.sh scripts.
#   2. Builds the kernel boot image with `cargo bootimage`.
#   3. Creates a FAT32 data disk image (default 16 MiB).
#   4. Copies every user_programs/*/*.bin into the FAT32 filesystem with
#      8.3 uppercase names (e.g. TERMINAL.BIN).
#
# Disk layout expected by the kernel (see src/entry_point.rs, src/fs.rs):
#   - Boot image -> IDE primary master   (QEMU: -drive ...,if=ide,index=0)
#   - Data disk  -> IDE secondary master (QEMU: -drive ...,if=ide,index=2),
#                   raw FAT32 filesystem starting at sector 0.
#
# Requirements: mtools (mformat, mcopy, mdir), cargo, the kernel's Rust
# toolchain (see rust-toolchain), llvm-tools-preview (for the user builds).
#
# Usage: ./tools/mkdisk.sh [OPTIONS] [OUTPUT_IMG]
#
#   OUTPUT_IMG         Path for the FAT32 data disk image
#                      (default: target/data-disk.img)
#
# Options:
#   -s, --size MB      Data disk size in MiB (default: 16)
#   -h, --help         Show this help and exit
#
# Examples:
#   ./tools/mkdisk.sh
#   ./tools/mkdisk.sh -s 32 /tmp/data-disk.img
#
# Boot with QEMU (command printed on success):
#   qemu-system-x86_64 \
#     -drive id=boot,file=target/x86_64-self-rust-os/debug/bootimage-self_rust_os.bin,format=raw,if=ide,index=0 \
#     -drive id=data,file=<OUTPUT_IMG>,format=raw,if=ide,index=2 \
#     -serial stdio -m 128

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

SIZE_MB=16
OUTPUT_IMG="${ROOT}/target/data-disk.img"

usage() {
    sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'
}

while [ $# -gt 0 ]; do
    case "$1" in
        -h|--help)
            usage
            exit 0
            ;;
        -s|--size)
            [ $# -ge 2 ] || { echo "Error: --size needs a value." >&2; exit 1; }
            SIZE_MB="$2"
            shift 2
            ;;
        -*)
            echo "Error: unknown option '$1'. See --help." >&2
            exit 1
            ;;
        *)
            OUTPUT_IMG="$1"
            shift
            ;;
    esac
done

if ! [[ "${SIZE_MB}" =~ ^[0-9]+$ ]] || [ "${SIZE_MB}" -lt 1 ]; then
    echo "Error: --size must be a positive integer (MiB)." >&2
    exit 1
fi

echo "[mkdisk] self_rust_os disk builder"
echo "[mkdisk] root:        ${ROOT}"
echo "[mkdisk] data image:  ${OUTPUT_IMG} (${SIZE_MB} MiB)"

# ---------------------------------------------------------------------------
# 0. Prerequisites
# ---------------------------------------------------------------------------
for tool in mformat mcopy mdir cargo; do
    if ! command -v "${tool}" >/dev/null 2>&1; then
        echo "Error: required tool '${tool}' not found in PATH." >&2
        case "${tool}" in
            mformat|mcopy|mdir) echo "Install it with: sudo apt install mtools" >&2 ;;
            cargo) echo "Install the Rust toolchain (see rust-toolchain)." >&2 ;;
        esac
        exit 1
    fi
done
echo "[mkdisk] prerequisites OK (mtools, cargo)"

# ---------------------------------------------------------------------------
# 1. Build user-space programs.
# ---------------------------------------------------------------------------
echo "[mkdisk] building user-space programs..."
if [ -x "${SCRIPT_DIR}/build-user.sh" ]; then
    "${SCRIPT_DIR}/build-user.sh"
else
    # Fallback for trees without the unified builder: run each
    # per-program build.sh.
    for prog_dir in "${ROOT}"/user_programs/*/; do
        if [ -x "${prog_dir}/build.sh" ]; then
            (cd "${prog_dir}" && ./build.sh)
        fi
    done
fi

# Collect the freshly built flat binaries.
mapfile -t BIN_FILES < <(find "${ROOT}/user_programs" -maxdepth 2 -name '*.bin' | sort)
if [ "${#BIN_FILES[@]}" -eq 0 ]; then
    echo "Error: no user program binaries (*.bin) found after building." >&2
    exit 1
fi
echo "[mkdisk] found ${#BIN_FILES[@]} user program binary(ies)"

# ---------------------------------------------------------------------------
# 2. Build the kernel boot image.
# ---------------------------------------------------------------------------
echo "[mkdisk] building kernel boot image..."
export PATH="$HOME/.cargo/bin:$PATH"
(cd "${ROOT}" && cargo bootimage 2>&1 | tail -1)
BOOTIMAGE="${ROOT}/target/x86_64-self-rust-os/debug/bootimage-self_rust_os.bin"
if [ ! -f "${BOOTIMAGE}" ]; then
    echo "Error: boot image not found at ${BOOTIMAGE}." >&2
    exit 1
fi
echo "[mkdisk] boot image: ${BOOTIMAGE}"

# ---------------------------------------------------------------------------
# 3. Create the FAT32 data disk image.
# ---------------------------------------------------------------------------
if [ -f "${OUTPUT_IMG}" ]; then
    echo "[mkdisk] overwriting existing image ${OUTPUT_IMG}"
fi
mkdir -p "$(dirname "${OUTPUT_IMG}")"
echo "[mkdisk] creating ${SIZE_MB} MiB zeroed image..."
dd if=/dev/zero of="${OUTPUT_IMG}" bs=1M count="${SIZE_MB}" status=none
echo "[mkdisk] formatting as FAT32 (mformat -F)..."
mformat -F -i "${OUTPUT_IMG}" ::

# ---------------------------------------------------------------------------
# 4. Copy user binaries with 8.3 uppercase names.
# ---------------------------------------------------------------------------
echo "[mkdisk] copying user programs into the image..."
for bin in "${BIN_FILES[@]}"; do
    base="$(basename "${bin}")"
    # Uppercase; names are already 8.3-compatible (e.g. terminal.bin).
    dest="::${base^^}"
    echo "[mkdisk]   ${base} -> ${dest}"
    mcopy -i "${OUTPUT_IMG}" "${bin}" "${dest}"
done

# ---------------------------------------------------------------------------
# 5. Verify.
# ---------------------------------------------------------------------------
echo "[mkdisk] verifying image contents:"
mdir -i "${OUTPUT_IMG}" ::

echo ""
echo "[mkdisk] done."
echo "[mkdisk] Boot with:"
echo "  qemu-system-x86_64 \\"
echo "    -drive id=boot,file=${BOOTIMAGE},format=raw,if=ide,index=0 \\"
echo "    -drive id=data,file=${OUTPUT_IMG},format=raw,if=ide,index=2 \\"
echo "    -serial stdio -m 128"
