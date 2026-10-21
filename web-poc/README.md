# self_rust_os in the browser — POC (QEMU Wasm, 64-bit guest)

Branch: `poc/browser-qemu-wasm`, based on `feature/grub-stable-480p`
(the branch Jérôme confirmed booting on 2026-10-16).

## Goal

Run the **unmodified 64-bit** self_rust_os inside a web browser tab —
no 32-bit port, no kernel changes. The vehicle is
[QEMU Wasm](https://github.com/ktock/qemu-wasm): the real QEMU
`qemu-system-x86_64` compiled to WebAssembly with its TCG JIT enabled.
v86 was ruled out (32-bit guests only) and CheerpX/WebVM was ruled out
(it emulates the Linux syscall ABI, not a whole machine, and its
64-bit "system mode" is not shipped).

## How it works

```
Browser tab
  index.html ── xterm.js terminal (serial console)
       │
  out.js / qemu-system-x86_64.wasm ── QEMU compiled to Wasm
       │  guest sees a stock PC: SeaBIOS, std VGA, IDE disk, PS/2
       ▼
  /pack/grub-disk.img ── the exact image built by tools/build-grub-disk.sh
       SeaBIOS → GRUB2 (MBR) → Multiboot2 → kernel.elf → WM
```

- `htdocs/module.js` holds the QEMU command line. The disk is attached
  as IDE (`if=ide,index=0`) because the kernel uses ATA PIO. 128 MiB
  RAM, `-nic none`, boot order `c`.
- `htdocs/index.html` fetches the BIOS ROMs and `grub-disk.img` from
  `htdocs/pack-src/` and injects them into the Emscripten filesystem
  at `/pack/` before QEMU starts (upstream examples use a
  file_packager `.data` bundle instead; runtime injection keeps OS
  rebuilds cheap — no repacking after each kernel change).
- This milestone runs QEMU with `-nographic`: COM1 is wired to the
  page terminal. The kernel mirrors its boot log and panics to COM1
  and the WM reports its startup there, so the serial log validates
  the entire boot chain. The page also mirrors the terminal into a
  plain DOM node (`#serial-log`) once a second for easy reading.
- `serve.py` is a stdlib-only static server that sends the
  `Cross-Origin-Opener-Policy: same-origin` and
  `Cross-Origin-Embedder-Policy: require-corp` headers. Without them
  the page is not cross-origin isolated, `SharedArrayBuffer` is
  unavailable, and QEMU's multi-threaded TCG cannot start.

## Quick start

Requirements: Rust nightly (repo toolchain), grub-pc-bin, mtools,
python3; plus Docker for the from-source emulator build (or use the
prebuilt path, which needs neither Docker nor a C toolchain).

```bash
cd web-poc

# 1. Emulator artifacts into htdocs/ — pick ONE:
./build-qemu-wasm.sh        # from source (Docker), pinned qemu-wasm commit
./fetch-prebuilt-qemu.sh    # prebuilt, from the upstream demo images (pinned commit)

# 2. OS disk image into htdocs/pack-src/ (also installs /BIN/*.BIN):
./package-image.sh

# 3. Serve (COOP/COEP headers included):
./serve.py 8088
# open http://127.0.0.1:8088/
```

BIOS ROMs (`bios-256k.bin`, `vgabios-stdvga.bin`, `kvmvapic.bin`,
`linuxboot_dma.bin`) are staged into `htdocs/pack-src/` by either
emulator script; they come from the qemu-wasm tree's `pc-bios/`.

Generated artifacts (emulator binaries, disk image) are git-ignored;
only the page, scripts and vendored xterm files are committed.

## Test results

Native reference run of the exact staged image
(`htdocs/pack-src/grub-disk.img`, QEMU 8.2.2, `-display none`,
30 s timeout kill) — full boot, no panic:

- `GRUB starting`, `[grub] multiboot2 magic OK: 0x36d76289`
- `[grub] bootloader framebuffer: 640x480x32 type=1 addr=0xfd000000`
- `[mouse] initialized (IRQ12, 3-byte packets)`
- `[fs] mounting FAT32 partition: lba_start=2048`, root mounted
- `[sched] spawned process 0 (12792 bytes)`
- `[wm] Window manager starting...`, `[wm] Framebuffer: 640x480 pitch=2560 bpp=32`
- Still running when the timeout killed QEMU (no double fault).

Browser run: **not yet confirmed — environment limitation, help
wanted.** The sandbox used to develop this POC ships a managed
Chromium that refuses to load *any* loopback page
(`ERR_BLOCKED_BY_LOCAL_NETWORK_ACCESS_CHECKS`, even for
browser-initiated navigations and with the Local Network Access
disable flags), and the delegated browser tool rejects loopback URLs
outright — so the in-tab boot could not be observed from there. The
server, headers, page and emulator artifacts were all verified
individually (HTTP 200s, COOP/COEP present, image identical to the
native run above). To confirm the in-browser boot on a normal
browser, run the Quick start, open the page, and check the serial
log for the same markers as the native run, ending with
`[wm] Window manager starting...` and the framebuffer line.

## Known limitations / next steps

1. **No visible desktop yet.** `-nographic` means the VGA
   framebuffer is emulated but not displayed. Next milestone:
   investigate a display path for QEMU Wasm (Emscripten display
   backend or VNC + noVNC over websocket) so the 640×480 WM desktop
   becomes visible and the PS/2 keyboard/mouse can be exercised.
2. **Performance.** TCG JIT in a tab is much slower than native
   QEMU; fine for this OS, but boot takes noticeably longer.
3. **Disk writes** go to the in-memory Emscripten FS and are lost
   on page reload. Persistence (IndexedDB) is out of scope.
4. **Prebuilt vs from-source.** The prebuilt emulator comes from the
   upstream Alpine demo build; the from-source script is the
   canonical reproducible path.
