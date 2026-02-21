//! Process loading and user mode execution.
//!
//! This module provides the infrastructure to load a flat binary into
//! user-accessible memory pages and switch the CPU to Ring 3 for execution.
//!
//! The user binary is expected to be a position-dependent flat binary linked
//! at [`USER_CODE_START`](super::USER_CODE_START). It is mapped into the current
//! address space with `USER_ACCESSIBLE` page flags, and a separate user-mode
//! stack is allocated below [`USER_STACK_TOP`](super::USER_STACK_TOP).

use core::sync::atomic::AtomicU64;

use x86_64::{
    structures::paging::{FrameAllocator, Mapper, Page, PageTableFlags, Size4KiB},
    VirtAddr,
};

use crate::{gdt, println, serial_println, userspace};

/// Saved kernel RSP before entering user mode.
///
/// When [`switch_to_user_mode`] executes `iretq`, the original kernel stack is
/// abandoned. This static stores the kernel RSP so that the `sys_exit` syscall
/// handler can restore it and effectively "return" from `switch_to_user_mode`
/// back to [`run`].
pub(crate) static KERNEL_RSP: AtomicU64 = AtomicU64::new(0);

/// Maps the user binary into memory and switches the CPU to Ring 3 execution.
///
/// This function:
/// 1. Maps the binary at [`USER_CODE_START`](super::USER_CODE_START) with user-accessible flags.
///    Pages within the first `readonly_size` bytes are mapped read-only (W^X),
///    while pages beyond that boundary are mapped writable for `.data`/`.bss`.
/// 2. Allocates a user-mode stack below [`USER_STACK_TOP`](super::USER_STACK_TOP).
/// 3. Performs an `iretq` to transition the CPU from Ring 0 to Ring 3.
///
/// # Arguments
///
/// * `binary` - The raw bytes of the flat user binary.
/// * `readonly_size` - Number of bytes from the start of the binary that should
///   be mapped read-only (`.text` + `.rodata`). Must be page-aligned (multiple
///   of 4096). Pages beyond this offset are mapped writable.
/// * `mapper` - The active page table mapper.
/// * `frame_allocator` - A physical frame allocator.
///
/// # Errors
///
/// Returns an error string if page mapping or frame allocation fails.
///
/// # Safety Considerations
///
/// This function uses `iretq` to enter user mode and never returns to the caller.
/// The caller must ensure that the GDT, TSS, and IDT (including the syscall
/// handler at `int 0x80`) are fully initialized before calling this function.
pub fn run(
    binary: &[u8],
    readonly_size: usize,
    mapper: &mut impl Mapper<Size4KiB>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) -> Result<(), &'static str> {
    serial_println!("[kernel] loading user binary ({} bytes)...", binary.len());
    println!("[kernel] loading user binary ({} bytes)...", binary.len());

    map_user_binary(binary, readonly_size, mapper, frame_allocator)?;
    map_user_stack(mapper, frame_allocator)?;

    serial_println!("[kernel] switching to user mode...");
    println!("[kernel] switching to user mode...");

    let user_cs = gdt::user_code_selector();
    let user_ds = gdt::user_data_selector();

    // SAFETY:
    //
    // The binary has been mapped at USER_CODE_START with USER_ACCESSIBLE pages.
    // The user stack has been mapped below USER_STACK_TOP.
    // The GDT contains valid Ring 3 code and data segments.
    // The TSS has a valid RSP0 for kernel re-entry on interrupts.
    //
    // This call does not return until the user process invokes `sys_exit`,
    // at which point the syscall handler restores the kernel RSP saved by
    // `switch_to_user_mode` and execution resumes here.
    unsafe {
        switch_to_user_mode(
            userspace::USER_CODE_START,
            userspace::USER_STACK_TOP,
            u64::from(user_cs.0),
            u64::from(user_ds.0),
        );
    }

    serial_println!("[kernel] user process exited, returning to kernel_main");
    println!("[kernel] user process exited, returning to kernel_main");

    Ok(())
}

/// Maps the flat binary bytes into user-accessible pages starting at
/// [`USER_CODE_START`](super::USER_CODE_START).
///
/// All pages are initially mapped writable so the binary content can be
/// copied in. After copying, pages within the first `readonly_size` bytes
/// are remapped as read-only to enforce W^X (Write XOR Execute).
///
/// # Arguments
///
/// * `binary` - The raw bytes of the flat user binary.
/// * `readonly_size` - Page-aligned byte count of the read-only region
///   (`.text` + `.rodata`). Pages beyond this offset keep `WRITABLE`.
/// * `mapper` - The active page table mapper.
/// * `frame_allocator` - A physical frame allocator.
fn map_user_binary(
    binary: &[u8],
    readonly_size: usize,
    mapper: &mut impl Mapper<Size4KiB>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) -> Result<(), &'static str> {
    assert!(
        readonly_size % 4096 == 0,
        "readonly_size must be page-aligned"
    );

    let start_addr = VirtAddr::new(userspace::USER_CODE_START);
    let num_pages = (binary.len() as u64 + 4095) / 4096;
    let readonly_pages = (readonly_size as u64) / 4096;

    // Phase 1: Map all pages as writable and copy binary content.
    let writable_flags =
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;

    for i in 0..num_pages {
        let page: Page<Size4KiB> = Page::containing_address(start_addr + i * 4096);
        let frame = frame_allocator
            .allocate_frame()
            .ok_or("failed to allocate frame for user binary")?;

        // SAFETY:
        //
        // The page is in the user address range and is not already mapped.
        // The frame was freshly allocated by the frame allocator.
        unsafe {
            mapper
                .map_to(page, frame, writable_flags, frame_allocator)
                .map_err(|_| "failed to map user binary page")?
                .flush();
        }

        // Copy binary content into the mapped page.
        let page_start = (i * 4096) as usize;
        let page_end = core::cmp::min(page_start + 4096, binary.len());
        let dest_ptr = page.start_address().as_u64() as *mut u8;

        if page_start < binary.len() {
            let chunk = &binary[page_start..page_end];

            // SAFETY:
            //
            // We just mapped this page as writable. The destination pointer is
            // valid for `chunk.len()` bytes and does not overlap with `chunk`.
            unsafe {
                core::ptr::copy_nonoverlapping(chunk.as_ptr(), dest_ptr, chunk.len());
            }

            // Zero the remainder of the page if the chunk is smaller than 4 KiB.
            if chunk.len() < 4096 {
                // SAFETY:
                //
                // The rest of the page (after the copied bytes) is within the
                // same mapped frame and is valid for writing.
                unsafe {
                    core::ptr::write_bytes(dest_ptr.add(chunk.len()), 0, 4096 - chunk.len());
                }
            }
        } else {
            // SAFETY:
            //
            // The entire page is mapped and writable; zero-fill it.
            unsafe {
                core::ptr::write_bytes(dest_ptr, 0, 4096);
            }
        }
    }

    // Phase 2: Remap read-only pages (code + rodata) without WRITABLE.
    // This enforces W^X: these pages remain executable but can no longer be
    // written to, preventing the program from modifying its own code.
    let readonly_flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;

    for i in 0..core::cmp::min(readonly_pages, num_pages) {
        let page: Page<Size4KiB> = Page::containing_address(start_addr + i * 4096);

        // SAFETY:
        //
        // The page was mapped in Phase 1 and is still valid. We are only
        // changing the flags to remove WRITABLE.
        unsafe {
            mapper
                .update_flags(page, readonly_flags)
                .map_err(|_| "failed to update flags for read-only page")?
                .flush();
        }
    }

    serial_println!(
        "[kernel] mapped {} pages for user binary at {:#x} ({} read-only, {} writable)",
        num_pages,
        userspace::USER_CODE_START,
        core::cmp::min(readonly_pages, num_pages),
        num_pages.saturating_sub(readonly_pages),
    );
    println!(
        "[kernel] mapped {} pages for user binary at {:#x} ({} read-only, {} writable)",
        num_pages,
        userspace::USER_CODE_START,
        core::cmp::min(readonly_pages, num_pages),
        num_pages.saturating_sub(readonly_pages),
    );

    Ok(())
}

/// Allocates and maps user-accessible stack pages in the range
/// [`USER_STACK_BOTTOM`](super::USER_STACK_BOTTOM) to
/// [`USER_STACK_TOP`](super::USER_STACK_TOP).
fn map_user_stack(
    mapper: &mut impl Mapper<Size4KiB>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) -> Result<(), &'static str> {
    let flags =
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;

    let num_pages = userspace::USER_STACK_SIZE / 4096;
    let stack_start = VirtAddr::new(userspace::USER_STACK_BOTTOM);

    for i in 0..num_pages {
        let page: Page<Size4KiB> = Page::containing_address(stack_start + i * 4096);
        let frame = frame_allocator
            .allocate_frame()
            .ok_or("failed to allocate frame for user stack")?;

        // SAFETY:
        //
        // The page is in the user stack range and is not already mapped.
        // The frame was freshly allocated by the frame allocator.
        unsafe {
            mapper
                .map_to(page, frame, flags, frame_allocator)
                .map_err(|_| "failed to map user stack page")?
                .flush();
        }

        // Zero-initialize the stack page.
        let dest_ptr = page.start_address().as_u64() as *mut u8;

        // SAFETY:
        //
        // The page is mapped and writable; zero-fill for a clean stack.
        unsafe {
            core::ptr::write_bytes(dest_ptr, 0, 4096);
        }
    }

    serial_println!(
        "[kernel] mapped {} stack pages at {:#x}-{:#x}",
        num_pages,
        userspace::USER_STACK_BOTTOM,
        userspace::USER_STACK_TOP,
    );
    println!(
        "[kernel] mapped {} stack pages at {:#x}-{:#x}",
        num_pages,
        userspace::USER_STACK_BOTTOM,
        userspace::USER_STACK_TOP,
    );

    Ok(())
}

/// Performs the actual transition from Ring 0 to Ring 3 via `iretq`.
///
/// Before executing `iretq`, this function saves all callee-saved registers
/// and the kernel RSP into [`KERNEL_RSP`]. When the user process calls
/// `sys_exit`, the syscall handler restores the saved RSP, pops the
/// callee-saved registers, and executes `ret`, causing this function to
/// return normally to its caller.
///
/// # Arguments
///
/// Uses the System V AMD64 calling convention (naked function):
/// * `rdi` - The virtual address of the user binary entry point.
/// * `rsi` - The top of the user-mode stack (initial RSP in Ring 3).
/// * `rdx` - The user code segment selector (with RPL=3).
/// * `rcx` - The user data segment selector (with RPL=3).
///
/// # Safety
///
/// The caller must guarantee that:
/// - `entry_point` points to valid, executable, user-accessible code.
/// - `user_stack` points to a valid, writable, user-accessible stack region.
/// - `user_cs` and `user_ds` are valid Ring 3 segment selectors in the GDT.
/// - The TSS `RSP0` is set to a valid kernel stack for interrupt re-entry.
///
/// This function blocks until the user process calls `sys_exit`.
#[naked]
unsafe extern "C" fn switch_to_user_mode(
    _entry_point: u64,
    _user_stack: u64,
    _user_cs: u64,
    _user_ds: u64,
) {
    // SAFETY:
    //
    // This naked function manually manages the entire stack layout.
    // On entry (System V AMD64 ABI):
    //   rdi = entry_point
    //   rsi = user_stack
    //   rdx = user_cs
    //   rcx = user_ds
    //   [rsp] = return address to caller (process::run)
    //
    // We save all callee-saved registers so the caller's state is preserved
    // when sys_exit restores the kernel RSP and executes `ret`.
    unsafe {
        core::arch::naked_asm!(
            // Save callee-saved registers (System V ABI).
            "push rbx",
            "push rbp",
            "push r12",
            "push r13",
            "push r14",
            "push r15",

            // Save the kernel RSP so sys_exit can restore it later.
            // After this point, KERNEL_RSP points to the saved r15 on
            // the original kernel stack.
            "mov [{kernel_rsp}], rsp",

            // Load user data segment (rcx) into all data segment registers.
            "mov ax, cx",
            "mov ds, ax",
            "mov es, ax",
            "mov fs, ax",
            "mov gs, ax",

            // Build an iretq frame on the stack:
            //   push SS      (user data segment)
            //   push RSP     (user stack pointer)
            //   push RFLAGS  (with IF set to enable interrupts)
            //   push CS      (user code segment)
            //   push RIP     (user entry point)
            "push rax",       // SS = user data selector
            "push rsi",       // RSP = user stack top
            "pushfq",
            "pop r11",
            "or r11, 0x200",  // set IF (Interrupt Flag) so interrupts work in user mode
            "push r11",       // RFLAGS
            "push rdx",       // CS = user code selector
            "push rdi",       // RIP = entry point

            "iretq",

            // Execution never reaches here via iretq.
            // When sys_exit fires, the syscall handler restores RSP from
            // KERNEL_RSP, pops r15..rbx, restores kernel segments, and
            // executes `ret` — which returns to the caller of this function.

            kernel_rsp = sym KERNEL_RSP,
        );
    }
}
