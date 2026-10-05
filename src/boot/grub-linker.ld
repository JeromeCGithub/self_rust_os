/* Linker script for the GRUB2 / Multiboot2 boot path (`grub` cargo feature).
 *
 * Differences from the default kernel link:
 * - The entry point is the 32-bit `multiboot2_entry` stub (GRUB enters in
 *   32-bit protected mode with paging disabled).
 * - The `.boot` section (Multiboot2 header) is placed FIRST so GRUB finds the
 *   magic within the first 32 KiB of the image.
 * - The 32-bit stub (`.text.boot`) follows immediately.
 * - `__kernel_start` / `__kernel_end` delimit the image so the frame
 *   allocator can reserve it.
 */

OUTPUT_FORMAT(elf64-x86-64)
OUTPUT_ARCH(i386:x86-64)
ENTRY(multiboot2_entry)

SECTIONS
{
    . = 0x100000;

    __kernel_start = .;

    .boot :
    {
        KEEP(*(.multiboot2_header))
    }

    .text :
    {
        *(.text.boot)
        *(.text .text.*)
    }

    .rodata :
    {
        *(.rodata .rodata.*)
    }

    .data :
    {
        *(.data .data.*)
    }

    .bss :
    {
        *(COMMON)
        *(.bss .bss.*)
        *(.bss.boot)
    }

    __kernel_end = .;

    /DISCARD/ :
    {
        *(.comment)
        *(.note*)
        *(.eh_frame*)
    }
}
