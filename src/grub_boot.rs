//! GRUB2 / Multiboot2 boot entry (only compiled with the `grub` feature).
//!
//! GRUB enters the kernel in 32-bit protected mode, paging disabled, with
//! `eax = 0x36D76289` and `ebx` = physical address of the Multiboot2
//! information structure. The [`multiboot2_entry`] stub below:
//!
//! 1. saves `eax`/`ebx` on a boot stack,
//! 2. builds P4/P3/P2 page tables identity-mapping the first GiB with 2 MiB
//!    pages,
//! 3. enables PAE, long mode (EFER.LME) and paging,
//! 4. far-jumps into 64-bit code, which calls [`grub_entry_rust`].
//!
//! [`grub_entry_rust`]: fn.grub_entry_rust.html

use core::arch::global_asm;

use x86_64::VirtAddr;

use self_rust_os::{
    boot::{self, multiboot2},
    memory::{self, MemRegion},
    serial_println,
};

// Multiboot2 header: must sit within the first 32 KiB of the image (the
// linker script places `.boot` first). Requests a 1920x1080x32 framebuffer.
global_asm!(
    r#"
    .section .multiboot2_header, "a"
    .align 8
mb2_header_start:
    .long 0xE85250D6              /* magic */
    .long 0                       /* architecture: i386 */
    .long mb2_header_end - mb2_header_start
    .long -(0xE85250D6 + 0 + (mb2_header_end - mb2_header_start))  /* checksum */
    /* framebuffer tag (type 5): request 1920x1080x32 */
    .align 8
    .short 5                      /* type */
    .short 0                      /* flags */
    .long 20                      /* size */
    .long 1920                    /* width */
    .long 1080                    /* height */
    .long 32                      /* depth (bpp) */
    /* end tag */
    .align 8
    .short 0
    .short 0
    .long 8
mb2_header_end:
    "#
);

// 32-bit entry stub.
global_asm!(
    r#"
    .section .text.boot, "ax"
    .code32

    .global multiboot2_entry
    .type multiboot2_entry, @function
multiboot2_entry:
    cli
    cld
    movl $boot_stack_top, %esp

    /* Save GRUB's eax (magic) and ebx (info pointer). */
    pushl %ebx
    pushl %eax

    /* P4[0] -> P3 */
    movl $p3_table_boot, %eax
    orl $0x3, %eax
    movl %eax, p4_table_boot

    /* P3[0] -> P2 */
    movl $p2_table_boot, %eax
    orl $0x3, %eax
    movl %eax, p3_table_boot

    /* P2: 512 x 2 MiB huge pages, identity mapped. */
    movl $0, %ecx
1:
    movl %ecx, %eax
    imull $0x200000, %eax       /* 2 MiB */
    orl $0x83, %eax             /* PRESENT | WRITABLE | HUGE */
    movl %eax, p2_table_boot(, %ecx, 8)
    incl %ecx
    cmpl $512, %ecx
    jne 1b

    /* Enable PAE. */
    movl %cr4, %eax
    orl $(1 << 5), %eax
    movl %eax, %cr4

    /* Enable long mode (EFER.LME). */
    movl $0xC0000080, %ecx
    rdmsr
    orl $(1 << 8), %eax
    wrmsr

    /* Load P4 and enable paging. */
    movl $p4_table_boot, %eax
    movl %eax, %cr3
    movl %cr0, %eax
    orl $(1 << 31), %eax
    movl %eax, %cr0

    /* Far jump into 64-bit code. */
    lgdt gdt64_ptr_boot
    ljmp $0x08, $long_mode_entry_boot

    .code64
long_mode_entry_boot:
    movw $0x10, %ax
    movw %ax, %ds
    movw %ax, %es
    movw %ax, %fs
    movw %ax, %gs
    movw %ax, %ss

    /* Fetch saved magic/info (pushed as 32-bit values). */
    movl (%rsp), %edi
    movl 4(%rsp), %esi
    addq $8, %rsp

    call grub_entry_rust

    /* Never returns; halt if it does. */
    cli
2:
    hlt
    jmp 2b

    /* 64-bit GDT: null, code (L=1), data. */
    .section .rodata.boot, "a"
    .align 16
gdt64_boot:
    .quad 0x0000000000000000
    .quad 0x00209A0000000000
    .quad 0x0000920000000000
gdt64_ptr_boot:
    .short gdt64_boot_end - gdt64_boot - 1
    .long gdt64_boot
gdt64_boot_end:

    /* Boot page tables + stack (zeroed by GRUB's ELF loader). */
    .section .bss.boot, "aw", @nobits
    .align 4096
p4_table_boot:
    .skip 4096
p3_table_boot:
    .skip 4096
p2_table_boot:
    .skip 4096
    .align 16
    .skip 65536                  /* 64 KiB boot stack */
boot_stack_top:
    "#,
    options(att_syntax)
);

// Linker-provided end of the kernel image (see `grub-linker.ld`).
unsafe extern "C" {
    static __kernel_end: u8;
}

/// Static storage for the parsed memory regions (no heap at this point).
static mut SAVED_REGIONS: [MemRegion; multiboot2::MAX_REGIONS] =
    [MemRegion { start: 0, end: 0 }; multiboot2::MAX_REGIONS];
static mut SAVED_REGION_COUNT: usize = 0;

/// 64-bit Rust entry point, called from the boot stub.
///
/// `magic` is GRUB's `eax`, `info_ptr` is GRUB's `ebx` (physical address of
/// the Multiboot2 information structure).
#[no_mangle]
pub extern "C" fn grub_entry_rust(magic: u64, info_ptr: u64) -> ! {
    // Interrupts are still disabled; the serial port works via lazy init.
    serial_println!("[grub] entered 64-bit Rust entry");

    let boot_info = match unsafe { multiboot2::parse(magic, info_ptr) } {
        Ok(info) => info,
        Err(e) => {
            serial_println!("[grub] FATAL: multiboot2 parse error: {:?}", e);
            self_rust_os::hlt_loop();
        }
    };

    serial_println!("[grub] multiboot2 magic OK: 0x{:x}", magic as u32);
    serial_println!(
        "[grub] memory map: {} regions, {} MiB usable",
        boot_info.region_count,
        boot_info.total_usable_bytes() / (1024 * 1024)
    );

    let fb = &boot_info.framebuffer;
    serial_println!(
        "[grub] bootloader framebuffer: {}x{}x{} type={} addr={:#x} pitch={}",
        fb.width,
        fb.height,
        fb.bpp,
        fb.fb_type,
        fb.addr,
        fb.pitch
    );

    // Validate the framebuffer: we need 32bpp RGB.
    if fb.bpp != 32 {
        serial_println!("[grub] FATAL: framebuffer bpp={} (expected 32)", fb.bpp);
        self_rust_os::hlt_loop();
    }
    if fb.fb_type != 1 {
        serial_println!("[grub] FATAL: framebuffer type={} (expected 1=RGB)", fb.fb_type);
        self_rust_os::hlt_loop();
    }

    // Construct FbInfo from the Multiboot2 framebuffer tag.
    let fb_info = self_rust_os::gfx::FbInfo {
        addr: fb.addr,
        width: fb.width as usize,
        height: fb.height as usize,
        pitch: fb.pitch as usize,
        bpp: fb.bpp,
    };
    serial_println!(
        "[grub] FbInfo: {}x{} pitch={} addr={:#x}",
        fb_info.width,
        fb_info.height,
        fb_info.pitch,
        fb_info.addr
    );

    // Reserve the kernel image itself: the firmware memory map may mark it
    // as available RAM, so clamp every region to start after `__kernel_end`.
    let kernel_end = core::ptr::addr_of!(__kernel_end) as u64;
    let kernel_end = kernel_end.next_multiple_of(4096);
    serial_println!("[grub] kernel image ends at {:#x}", kernel_end);

    let mut count = 0_usize;
    for region in boot_info.regions() {
        let start = region.start.max(kernel_end);
        if start < region.end && count < multiboot2::MAX_REGIONS {
            // SAFETY: Single-threaded boot; no other code touches these.
            unsafe {
                SAVED_REGIONS[count] = MemRegion {
                    start,
                    end: region.end,
                };
            }
            count += 1;
        }
    }
    // SAFETY: Single-threaded boot; no other code touches these.
    unsafe {
        SAVED_REGION_COUNT = count;
    }
    serial_println!(
        "[grub] frame allocator: {} regions after reserving kernel",
        count
    );

    // Map all RAM at the higher-half offset, plus the kernel image.
    // The kernel image (0..kernel_end) must be mapped because
    // the active P4 page table lives inside it; `memory::init` accesses the
    // P4 through the offset.
    //
    // NOTE: The linear framebuffer (e.g. at 0xE0000000) is NOT mapped here
    // because it's MMIO above RAM top. It will be mapped explicitly below.
    // SAFETY: Identity mapping is active; tables live below 1 GiB.
    let mapped = unsafe {
        boot::map_all_physical_memory(
            &SAVED_REGIONS[..SAVED_REGION_COUNT],
            &[(0, kernel_end)],
        )
    };
    serial_println!(
        "[grub] mapped {} MiB of physical memory at {:#x}",
        mapped / (1024 * 1024),
        boot::PHYS_MEM_OFFSET
    );

    let phys_mem_offset = VirtAddr::new(boot::PHYS_MEM_OFFSET);

    // SAFETY: The offset mapping established above covers all RAM.
    let mut mapper = unsafe { memory::init(phys_mem_offset) };

    // SAFETY: SAVED_REGIONS describes usable RAM above the kernel image.
    let regions: &'static [MemRegion] =
        unsafe { &SAVED_REGIONS[..SAVED_REGION_COUNT] };
    let mut frame_allocator = unsafe { memory::RegionFrameAllocator::new(regions) };

    // Explicitly map the linear framebuffer. It's at a high MMIO address
    // (e.g. 0xE0000000), not covered by the RAM mapping above.
    // Include scanline padding and the final partial page.
    let fb_size = (fb_info.height * fb_info.pitch) as u64;
    let fb_pages = (fb_size + 4095) / 4096;
    serial_println!(
        "[grub] mapping framebuffer: {} pages at phys {:#x}",
        fb_pages,
        fb_info.addr
    );

    // Choose a virtual address for the framebuffer: use the physical memory
    // offset + a high address, or a dedicated region. For simplicity, map it
    // at PHYS_MEM_OFFSET + fb.addr (if within mapped range) OR allocate
    // a new virtual region.
    //
    // Actually, the simplest: map it at a fixed high virtual address.
    // Let's use 0xFFFF_9000_0000_0000 as the framebuffer virtual base.
    const FB_VIRT_BASE: u64 = 0xFFFF_9000_0000_0000;
    let fb_virt_addr = VirtAddr::new(FB_VIRT_BASE);

    {
        use x86_64::structures::paging::{Page, PhysFrame, Mapper, Size4KiB, PageTableFlags};
        let mut page = Page::<Size4KiB>::containing_address(fb_virt_addr);
        let mut frame = PhysFrame::<Size4KiB>::containing_address(
            x86_64::PhysAddr::new(fb_info.addr)
        );
        let flags = PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::NO_CACHE;
        for _ in 0..fb_pages {
            // SAFETY: Mapping MMIO framebuffer; not RAM, so no allocator needed.
            unsafe {
                mapper
                    .map_to(page, frame, flags, &mut frame_allocator)
                    .expect("failed to map framebuffer page")
                    .flush();
            }
            page += 1;
            frame += 1;
        }
    }
    serial_println!(
        "[grub] framebuffer mapped at virt {:#x}",
        FB_VIRT_BASE
    );

    // From here on, this is the same bring-up as the legacy boot path.
    // SAFETY: run_kernel is defined in entry_point.rs (crate root for grub feature).
    crate::run_kernel(
        &mut mapper,
        frame_allocator,
        phys_mem_offset,
        fb_info,
        fb_virt_addr,
    )
}
