# GRUB2 and Multiboot2 boot

This describes the current desktop boot path. Older mode-13h assumptions and old
GRUB E2E metrics no longer describe this path. See [architecture](architecture.md)
and [test gaps](testing.md#known-baseline-gaps).

## Build and disk layout

From the repository root:

```bash
./tools/build-grub-disk.sh
cargo run
```

The first command builds the disk. `cargo run` also rebuilds it before launching
QEMU, so these are alternative entry points rather than a required two-step boot.
[The builder](../tools/build-grub-disk.sh) rebuilds all user programs, compiles the
kernel with `--features grub`, and overwrites `target/grub-disk.img` by default.
It uses GRUB BIOS tools and mtools; it does not mount a host filesystem or need a
loop device. Never point it at a host block device or a disk attached to running QEMU.

```text
LBA 0          GRUB boot code + MBR partition table + 0xAA55 signature
LBA 1..2047    GRUB core.img in the post-MBR gap
LBA 2048..end  FAT32 partition in a 128 MiB image
               /KERNEL.ELF
               /README.TXT
               /BIN/{WM,PORTAL,FORGE,HELLO,SNAKE,SPIN}.BIN
               /ETC/WALL.PPM
               /HOME and /TMP
```

GRUB's embedded configuration searches for the kernel, selects a 1920×1080×32 graphics
mode, keeps that framebuffer as the payload, and invokes `multiboot2 /kernel.elf`.
The [Multiboot2 header](../src/grub_boot.rs) requests that geometry. Do not describe
its framebuffer tag as informational only: the current kernel validates and uses it.

## Transition into the kernel

1. BIOS loads the MBR, which loads GRUB `core.img` from the post-MBR gap.
2. GRUB loads the ELF and enters `multiboot2_entry` in 32-bit protected mode, paging
   disabled, with the Multiboot2 magic in EAX and info pointer in EBX.
3. The assembly stub saves boot arguments, identity maps the first GiB with 2 MiB
   pages, enables PAE and long mode/paging, loads a 64-bit GDT, then calls Rust.
4. [The parser](../src/boot/multiboot2.rs) reads memory and framebuffer tags. Rust
   validates the framebuffer format and reserves the kernel image from usable RAM.
5. [Boot paging](../src/boot/mod.rs) establishes the physical-memory offset at
   `0xffff800000000000`; the framebuffer is separately mapped as device memory at
   `0xffff900000000000`. Keep physical and virtual framebuffer addresses distinct.
6. [Shared bring-up](../src/entry_point.rs) initializes graphics/console, GDT/IDT/PICs,
   heap and input. Early timer IRQs must remain masked until scheduling can safely run.
7. [The root mount](../src/fs.rs) reads the primary master's MBR, selects its FAT32
   partition, and applies the partition offset through [MBR helpers](../src/mbr.rs).
8. The kernel loads `/BIN/WM.BIN`, creates PID 0 with a valid kernel/user context,
   then enters the scheduler. `iretq` restores user flags and interrupts. The WM
   loads the image wallpaper, discovers apps and presents the desktop in Ring 3.

The initial WM filename is currently boot policy embedded in the kernel. It is a
known exception, not permission to put general app discovery or shell policy there.
The kernel no longer embeds Hello or a terminal flat binary at compile time.

## Distinct legacy test path

With no `grub` feature, the entry point still uses the `bootloader` crate. Kernel
integration tests run through `bootimage runner`, configured by
[Cargo metadata](../Cargo.toml) and dispatched by [the runner](../tools/run-desktop.sh).
Their RAM framebuffer fixtures are separate from the GRUB MMIO framebuffer. Passing
those tests does not prove the GRUB path booted or the real desktop input works.

Do not run `cargo run --features grub` through the old bootimage assumptions or treat
an 8-bit VGA mode as a 32-bit display. Ordinary `cargo run` selects the correct disk
builder/runner. `RUSTOS_DISPLAY=none cargo run` exercises that path headlessly.

## Verification and debugging

Rebuild a fresh image and use `python3 e2e/desktop.py`. Look for serial stages from
GRUB entry through memory mappings, FAT32 mount, PID 0 startup, absolute-pointer
initialization where available, and desktop readiness. Then verify actual app/input
behavior; a boot banner alone is insufficient. Use screenshots for visual inspection.

A black display or corrupt pixels can arise from early faults, wrong pitch/bpp,
missing MMIO mappings, invalid user surfaces, or bad context transitions. Use serial
logs and QEMU state to distinguish these instead of changing graphics constants or
masking interrupts until the symptom disappears. Never use old `e2e/grub_test.py`
mode-13h expectations as proof that the current desktop works.
