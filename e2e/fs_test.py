#!/usr/bin/env python3
"""Boot the kernel with disk images attached, send keys, capture screenshot.

Usage: fs_test.py <boot-image> <output-png>
"""
import socket
import subprocess
import sys
import time
import os

def main():
    boot_image = sys.argv[1]
    out_png = sys.argv[2]

    mon_sock = "/tmp/qemu-fs-mon"
    ppm_path = "/tmp/qemu-fs.ppm"
    for p in (mon_sock, ppm_path):
        try:
            os.unlink(p)
        except FileNotFoundError:
            pass

    qemu_cmd = [
        "qemu-system-x86_64",
        "-drive", f"format=raw,file={boot_image}",
        "-no-reboot",
        "-serial", "stdio",
        "-display", "none",
        "-monitor", f"unix:{mon_sock},server,nowait",
        # Attach disk fixtures (secondary master = FAT32).
        "-drive", "file=tests/fixtures/disk-ata.img,format=raw,file.locking=off",
        "-drive", "file=tests/fixtures/disk-fat32.img,format=raw,file.locking=off",
    ]
    print(f"[test] starting QEMU with disks...")
    qemu = subprocess.Popen(qemu_cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    for _ in range(50):
        if os.path.exists(mon_sock):
            break
        time.sleep(0.1)

    mon = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    mon.connect(mon_sock)
    time.sleep(0.5)
    mon.recv(4096)

    def sendkey(key):
        mon.sendall(f"sendkey {key}\n".encode())
        time.sleep(0.15)
        mon.recv(4096)

    def type_text(text):
        for ch in text:
            if ch == ' ':
                sendkey("spc")
            elif ch == '\n':
                sendkey("ret")
            else:
                sendkey(ch)

    print("[test] waiting for terminal...")
    time.sleep(12.0)

    print("[test] typing 'ls'...")
    type_text("ls\n")
    time.sleep(1.5)

    print("[test] capturing screenshot...")
    mon.sendall(f"screendump {ppm_path}\n".encode())
    time.sleep(0.5)
    mon.recv(4096)
    mon.close()
    qemu.terminate()

    from PIL import Image
    img = Image.open(ppm_path)
    img.save(out_png)
    print(f"[test] screenshot saved to {out_png}")

if __name__ == "__main__":
    main()
