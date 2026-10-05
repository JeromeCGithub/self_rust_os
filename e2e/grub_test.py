#!/usr/bin/env python3
"""End-to-end test for the GRUB2 / Multiboot2 boot path.

Boots the GRUB disk image (built by tools/build-grub-disk.sh) in QEMU and
asserts the deterministic serial metrics emitted at each boot stage:

  GRUB -> 32-bit stub -> 64-bit Rust -> page tables -> framebuffer
  -> MBR/FAT32 mount -> terminal

Usage: grub_test.py <disk-image> <output-png>
Exit code 0 = all metrics observed, 1 = failure.
"""
import os
import re
import socket
import subprocess
import sys
import time

# (regex, description) — every one must appear in the serial log.
EXPECTED = [
    (r"GRUB starting", "GRUB loaded from disk and ran embedded config"),
    (r"\[grub\] multiboot2 magic OK: 0x36d76289",
     "32-bit stub switched to long mode, magic verified"),
    (r"\[grub\] memory map: (\d+) regions, (\d+) MiB usable",
     "multiboot2 memory map parsed"),
    (r"\[grub\] VGA mode 13h set: 320x200x8 at 0xa0000",
     "kernel programmed VGA mode 13h directly (320x200x8 indexed)"),
    (r"\[grub\] kernel image ends at 0x[0-9a-f]+",
     "kernel image bounds detected for the frame allocator"),
    (r"\[grub\] mapped (\d+) MiB of physical memory at 0xffff800000000000",
     "higher-half mapping of all RAM established"),
    (r"\[fs\] mounting FAT32 partition: lba_start=2048",
     "MBR partition table parsed, FAT32 partition found"),
    (r"\[kernel\] root filesystem mounted",
     "FAT32 root filesystem mounted from the boot disk"),
    (r"\[kernel\] loading user binary \((\d+) bytes\)",
     "Ring 3 terminal binary loaded from FAT32"),
]


def main():
    disk_image = sys.argv[1]
    out_png = sys.argv[2]

    mon_sock = "/tmp/qemu-grub-mon"
    ppm_path = "/tmp/qemu-grub.ppm"
    serial_log = "/tmp/qemu-grub-serial.log"
    for p in (mon_sock, ppm_path, serial_log):
        try:
            os.unlink(p)
        except FileNotFoundError:
            pass

    qemu_cmd = [
        "qemu-system-x86_64",
        "-drive", f"format=raw,file={disk_image}",
        "-no-reboot",
        "-serial", f"file:{serial_log}",
        "-display", "none",
        "-vga", "std",
        "-m", "128",
        "-monitor", f"unix:{mon_sock},server,nowait",
    ]
    print("[test] booting GRUB disk image in QEMU...")
    qemu = subprocess.Popen(qemu_cmd, stdout=subprocess.DEVNULL,
                            stderr=subprocess.DEVNULL)
    try:
        for _ in range(50):
            if os.path.exists(mon_sock):
                break
            time.sleep(0.1)

        mon = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        mon.settimeout(5)
        mon.connect(mon_sock)
        time.sleep(0.5)
        mon.recv(4096)

        def sendkey(key):
            mon.sendall(f"sendkey {key}\n".encode())
            time.sleep(0.15)
            try:
                mon.recv(4096)
            except socket.timeout:
                pass

        print("[test] waiting for the terminal (GRUB + kernel boot)...")
        time.sleep(14.0)

        print("[test] typing 'ls'...")
        for ch in "ls\n":
            sendkey("ret" if ch == "\n" else ch)
        time.sleep(2.0)

        print("[test] capturing screenshot...")
        mon.sendall(f"screendump {ppm_path}\n".encode())
        time.sleep(1.0)
        try:
            mon.recv(4096)
        except socket.timeout:
            pass
        mon.close()

        from PIL import Image
        Image.open(ppm_path).save(out_png)
        print(f"[test] screenshot saved to {out_png}")
    finally:
        qemu.terminate()
        qemu.wait()

    with open(serial_log, "rb") as f:
        serial = f.read().decode("utf-8", errors="replace")

    print("[test] ---- serial metrics ----")
    failed = False
    for pattern, desc in EXPECTED:
        m = re.search(pattern, serial)
        status = "OK  " if m else "FAIL"
        if not m:
            failed = True
        print(f"[test] [{status}] {desc}")
        if m and m.groups():
            print(f"[test]        captured: {m.group(0)}")
    print("[test] -------------------------")

    if failed:
        print("[test] FAILED: some boot metrics are missing. Serial log:")
        print(serial[-3000:])
        return 1
    print("[test] PASSED: all GRUB boot metrics observed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
