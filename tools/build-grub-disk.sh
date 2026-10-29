#!/usr/bin/env bash
# Builds a bootable GRUB2 disk image for self_rust_os.
#
# Layout of the image (128 MiB):
#   LBA 0          : MBR with partition table (written by this script)
#   LBA 1 .. 2047  : GRUB core.img (post-MBR gap)
#   LBA 2048 ..    : FAT32 partition containing /kernel.elf
#
# GRUB's embedded config searches for /kernel.elf and boots it with the
# Multiboot2 protocol. No /boot/grub directory is needed on the disk.
#
# Requirements: grub-pc-bin (grub-mkimage), mtools (mformat, mcopy).
# No loop devices or root mounts are needed.
#
# Usage: ./tools/build-grub-disk.sh [output.img]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
IMG="${1:-${ROOT}/target/grub-disk.img}"

SIZE_MB=128
PART_START_SECTOR=2048          # 1 MiB aligned; room for core.img before it
SECTOR=512

"${ROOT}/tools/build-user.sh"

echo "[grub-disk] Building kernel ELF (grub feature)..."
cargo build --release --features grub
KERNEL_ELF="${ROOT}/target/x86_64-self-rust-os/release/self_rust_os"
test -f "${KERNEL_ELF}"

echo "[grub-disk] Creating ${SIZE_MB} MiB blank image at ${IMG}..."
dd if=/dev/zero of="${IMG}" bs=1M count=${SIZE_MB} status=none

echo "[grub-disk] Generating GRUB core.img with embedded boot config..."
cat > /tmp/grub-early.cfg <<'EOF'
serial --unit=0 --speed=115200
terminal_output serial
insmod all_video
set gfxmode=1920x1080x32
set gfxpayload=keep
terminal_output gfxterm serial
echo GRUB starting
search --set=root --file /kernel.elf
multiboot2 /kernel.elf
boot
EOF
# GRUB configures a true 32-bit linear framebuffer before entering the kernel.
grub-mkimage -O i386-pc -o /tmp/grub-core.img \
    -p "(hd0,msdos1)/boot/grub" -c /tmp/grub-early.cfg \
    biosdisk part_msdos fat search serial echo multiboot2 all_video gfxterm
CORE_SECTORS=$(( ($(stat -c%s /tmp/grub-core.img) + SECTOR - 1) / SECTOR ))
echo "[grub-disk] core.img is ${CORE_SECTORS} sectors"

echo "[grub-disk] Installing GRUB boot.img (MBR code) + core.img..."
# boot.img is exactly 512 bytes; write only the first 446 so a later step
# can lay down the partition table.
dd if=/usr/lib/grub/i386-pc/boot.img of="${IMG}" bs=446 count=1 conv=notrunc status=none
dd if=/tmp/grub-core.img of="${IMG}" bs=${SECTOR} seek=1 conv=notrunc status=none

echo "[grub-disk] Writing MBR partition table (one bootable FAT32 LBA partition)..."
python3 - "${IMG}" "${PART_START_SECTOR}" <<'EOF'
import struct, sys

img_path, part_start = sys.argv[1], int(sys.argv[2])
total_sectors = 128 * 1024 * 1024 // 512
part_sectors = total_sectors - part_start

with open(img_path, 'r+b') as f:
    mbr = bytearray(f.read(512))

    # Partition entry 1 at 0x1BE: bootable, type 0x0C (FAT32 LBA).
    # CHS fields are ignored by LBA-aware bootloaders; fill with max values.
    entry = struct.pack('<B3sB3sII',
                        0x80, b'\xff\xff\xff', 0x0C, b'\xff\xff\xff',
                        part_start, part_sectors)
    mbr[0x1BE:0x1BE + 16] = entry

    # Boot signature.
    mbr[0x1FE] = 0x55
    mbr[0x1FF] = 0xAA

    f.seek(0)
    f.write(mbr)

print(f"[grub-disk] partition: start LBA={part_start}, sectors={part_sectors}")
EOF

PART_OFFSET=$((PART_START_SECTOR * SECTOR))

echo "[grub-disk] Formatting FAT32 partition with mtools (no loop device)..."
# -F = FAT32, -v = volume label.
mformat -i "${IMG}@@${PART_OFFSET}" -F -v RUSTOS ::

echo "[grub-disk] Copying kernel.elf to the FAT32 partition..."
mcopy -i "${IMG}@@${PART_OFFSET}" "${KERNEL_ELF}" ::/kernel.elf

echo "[grub-disk] Adding a readme file (visible via the OS 'ls' command)..."
printf 'Welcome to self_rust_os!\r\nThis file lives on the FAT32 boot disk.\r\nThe kernel was loaded from this very partition by GRUB2.\r\n' \
    | mcopy -i "${IMG}@@${PART_OFFSET}" - ::/readme.txt

echo "[grub-disk] Disk image ready: ${IMG}"
ls -la "${IMG}"

# Desktop assets and every available user application.
mmd -i "${IMG}@@${PART_OFFSET}" ::/BIN ::/ETC ::/HOME ::/TMP
for binary in "${ROOT}"/user_programs/*/*.bin; do
    base="$(basename "${binary}")"
    mcopy -i "${IMG}@@${PART_OFFSET}" "${binary}" "::/BIN/${base^^}"
done
mcopy -i "${IMG}@@${PART_OFFSET}" "${ROOT}/user_programs/wm/wallpaper.ppm" ::/ETC/WALL.PPM
