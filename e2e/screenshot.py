#!/usr/bin/env python3
"""Take a screenshot of a QEMU VM via the monitor's screendump command.

Usage: screenshot.py <boot-image> <output-png> [wait-seconds] [extra-drive-args...]

Runs QEMU headless (no isa-debug-exit, so the VM stays up), waits, captures
the VGA/framebuffer content via the monitor, and converts it to PNG.
"""
import socket
import subprocess
import sys
import time
import os

def main():
    boot_image = sys.argv[1]
    out_png = sys.argv[2]
    wait_secs = float(sys.argv[3]) if len(sys.argv) > 3 else 8.0
    extra_args = sys.argv[4:]

    mon_sock = "/tmp/qemu-shot-mon"
    ppm_path = "/tmp/qemu-shot.ppm"
    for p in (mon_sock, ppm_path):
        try:
            os.unlink(p)
        except FileNotFoundError:
            pass

    qemu_cmd = [
        "qemu-system-x86_64",
        "-drive", f"format=raw,file={boot_image}",
        "-serial", "null",
        "-display", "none",
        "-monitor", f"unix:{mon_sock},server,nowait",
    ] + extra_args

    qemu = subprocess.Popen(qemu_cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        time.sleep(wait_secs)
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.settimeout(5)
        s.connect(mon_sock)
        s.recv(4096)  # banner
        s.sendall(f"screendump {ppm_path}\n".encode())
        time.sleep(1)
        s.recv(4096)
        s.close()

        from PIL import Image
        Image.open(ppm_path).save(out_png)
        print(f"screenshot saved to {out_png}")
    finally:
        qemu.terminate()
        qemu.wait()

if __name__ == "__main__":
    main()
