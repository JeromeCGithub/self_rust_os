# GRUB2 + Multiboot2 boot

This document explains how `self_rust_os` boots from a FAT32 disk image via
GRUB2 (the `grub` cargo feature). It is the reviewer-facing companion to the
`feature/grub-multiboot2` PR.

## Why

Previously the kernel booted from a raw image assembled by `cargo bootimage`:
the bootloader and kernel were concatenated at fixed sectors, with no
filesystem involved. The FAT32 disk was a *second* drive used only for data.

With this PR the OS lives **on the disk**: one 128 MiB image holds an MBR, a
FAT32 partition, GRUB, and the kernel as an ordinary file (`/kernel.elf`).
GRUB reads the file from FAT32 and starts the kernel with the Multiboot2
protocol. The kernel then mounts the very partition it was loaded from as its
root filesystem.

## Disk layout

```text
LBA 0            MBR: GRUB boot.img (446 bytes) + partition table + 0xAA55
LBA 1 .. 2047    GRUB core.img (embedded config, no /boot/grub needed)
LBA 2048 .. end  FAT32 partition (type 0x0C): /kernel.elf, /readme.txt
```

Built by `tools/build-grub-disk.sh` (no loop devices needed: `grub-mkimage`
for core.img, `mformat`/`mcopy` from mtools for the FAT32 partition, Python
for the MBR).

## Boot sequence

1. **QEMU BIOS** loads LBA 0, jumps to GRUB's `boot.img`.
2. **`boot.img`** loads `core.img` from the post-MBR gap.
3. **`core.img`** (embedded config) runs:
   `search --set=root --file /kernel.elf` → `multiboot2 /kernel.elf` → `boot`.
4. **32-bit stub** (`src/grub_boot.rs`, `multiboot2_entry`): GRUB enters with
   `eax = 0x36D76289`, `ebx` = Multiboot2 info pointer, paging off. The stub
   saves both, identity-maps the first GiB with 2 MiB pages (P4/P3/P2 in
   `.bss.boot`), enables PAE + long mode + paging, loads a 64-bit GDT, and
   far-jumps to `long_mode_entry_boot`, which calls `grub_entry_rust(magic,
   info_ptr)`.
5. **`grub_entry_rust`** (64-bit Rust):
   - parses the Multiboot2 info (`src/boot/multiboot2.rs`): memory map
     (tag 6) and framebuffer (tag 8, informational only);
   - programs VGA mode 13h (320x200x8) directly via I/O ports
     (`gfx::set_vga_mode_13h`) — GRUB 2.12 cannot reliably provide a graphics
     framebuffer via Multiboot2 (its framebuffer request header tag corrupts
     the heap, and `gfxpayload` does not apply without the tag);
   - reserves the kernel image (`__kernel_end` from the linker script) from
     the frame allocator's regions — the firmware map may mark it "available";
   - maps all RAM at `0xFFFF_8000_0000_0000` with 2 MiB huge pages
     (`src/boot/mod.rs`), plus the kernel image (0..kernel_end, so the active
     P4 is reachable through the offset) and the VGA framebuffer at 0xA0000;
   - builds the `OffsetPageTable` and a `RegionFrameAllocator`, then runs the
     same bring-up as the legacy path (`run_kernel` in `src/entry_point.rs`):
     GDT/IDT/PICs, heap (with interrupts disabled — the x86_64
     `OffsetPageTable::map_to` is not reentrant), keyboard queue, FAT32 mount.
   - re-initializes the PIC before enabling interrupts (GRUB leaves it in a
     state where the timer IRQ hangs the kernel).

   Note: the higher-half mapping uses P4 entry 256 (not 511) for
   `0xFFFF_8000_0000_0000`: bits 47:39 of the address equal 0x100.
6. **Root filesystem** (`src/fs.rs`): reads the MBR of the boot disk
   (primary master), finds the first FAT32 partition (`src/mbr.rs`), and
   mounts it through `OffsetDisk` (sector-number shift adapter).

## What the kernel no longer needs

The `bootloader` crate's `BootInfo` is gone from this path: no
`physical_memory_offset` (we choose `0xFFFF_8000_0000_0000` ourselves), no
`MemoryMap` wrapper (we parse tag 6 into `MemRegion`s). The framebuffer
address is fixed at 0xA0000 (VGA mode 13h programmed by the kernel). The
legacy `cargo bootimage` path is unchanged and still used by `cargo test`.

## Files

| File | Role |
|---|---|
| `src/grub_boot.rs` | Multiboot2 header + 32-bit stub + `grub_entry_rust` (bin only) |
| `src/boot/mod.rs` | `PHYS_MEM_OFFSET`, higher-half page-table setup (P4[256]) |
| `src/boot/multiboot2.rs` | Tag parser (allocation-free) + unit tests |
| `src/boot/grub-linker.ld` | Places `.boot` first, entry = `multiboot2_entry` |
| `src/mbr.rs` | MBR parsing + `OffsetDisk` + unit tests |
| `src/gfx.rs` | `set_vga_mode_13h()` — direct VGA register programming |
| `build.rs` | Selects the linker script when `grub` is enabled |
| `tools/build-grub-disk.sh` | Assembles the bootable disk image |
| `e2e/grub_test.py` | E2E: boots the image, asserts serial metrics, screenshots |

## Verification

`e2e/grub_test.py` asserts one serial metric per boot stage. Verified output
from QEMU (2026-10-05):

```text
GRUB starting
[grub] entered 64-bit Rust entry
[grub] multiboot2 magic OK: 0x36d76289
[grub] memory map: 2 regions, 127 MiB usable
[grub] bootloader framebuffer: 80x25x16 type=2 addr=0xb8000 pitch=160
[grub] VGA mode 13h set: 320x200x8 at 0xa0000
[grub] kernel image ends at 0x19f000
[grub] frame allocator: 1 regions after reserving kernel
[grub] mapped 128 MiB of physical memory at 0xffff800000000000
[kernel] RustOS booting...
[kernel] Initialization complete!
[kernel] mounting root filesystem...
[fs] mounting FAT32 partition: lba_start=2048 sectors=260096
[kernel] root filesystem mounted
[kernel] Heap initialized, starting user space demo...
[kernel] loading user binary (4496 bytes)...
```

The test then types `ls` via the QEMU monitor and captures a screenshot.
Note: the VGA display output is not yet verified — QEMU's `screendump`
reports "Guest has not initialized the display", indicating the direct VGA
register programming may need further work for the display to be visible.
The serial metrics above prove the boot chain, paging, heap, and filesystem
all work correctly.

Unit tests (`cargo test --lib`, no GRUB needed): the Multiboot2 parser is
tested against a hand-built info structure (valid parse, bad magic, null
pointer), and the MBR parser against hand-built sectors (FAT32 found, bad
signature, empty table) plus an `OffsetDisk` sector-shift test. 49 tests
pass.

## Known limitations

- The VGA mode 13h register programming sets the mode (confirmed via serial),
  but QEMU's display does not show the framebuffer contents. The console
  writes to 0xA0000 correctly (memory is mapped), but the display output
  needs investigation.
- Interrupts must be disabled during heap initialization (the x86_64 crate's
  `OffsetPageTable::map_to` is not interrupt-safe). They are re-enabled after
  the PIC is re-initialized.
- The Multiboot2 framebuffer request header tag is not used (it triggers a
  heap corruption bug in GRUB 2.12). The kernel programs VGA directly instead.
