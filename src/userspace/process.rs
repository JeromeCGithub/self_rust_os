//! User-space memory mapping primitives.
//!
//! This module provides functions to map user program binaries, stacks, and
//! the framebuffer into a page table. Used by the scheduler when creating
//! new processes with their own address spaces.
//!
//! The user binary is expected to be a position-dependent flat binary linked
//! at [`USER_CODE_START`](super::USER_CODE_START).

use x86_64::{
    structures::paging::{FrameAllocator, Mapper, Page, PageTableFlags, PhysFrame, Size4KiB},
    VirtAddr,
};

use crate::{println, serial_println, userspace};

/// Maps the flat binary bytes into user-accessible pages starting at
/// [`USER_CODE_START`](super::USER_CODE_START).
///
/// All pages are initially mapped writable so the binary content can be
/// copied in. After copying, pages within the first `readonly_size` bytes
/// are remapped as read-only to enforce W^X.
///
/// Returns the number of pages mapped.
///
/// # Arguments
/// * `binary` - The raw bytes of the flat user binary.
/// * `readonly_size` - Page-aligned byte count of the read-only region.
/// * `mapper` - The page table mapper for the target address space.
/// * `frame_allocator` - A physical frame allocator.
pub(crate) fn map_user_binary(
    binary: &[u8],
    readonly_size: usize,
    mapper: &mut impl Mapper<Size4KiB>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) -> Result<u64, &'static str> {
    map_user_binary_inner(binary, readonly_size, mapper, frame_allocator, None)
}

/// Maps the user binary, copying content via physical addresses (no CR3 switch needed).
///
/// `phys_offset` is the virtual address that maps physical memory (e.g., 0x18000000000).
/// The binary content is written to `phys_offset + frame.phys_addr` instead of the
/// virtual page address, so the caller does not need to switch to the new address space.
pub(crate) fn map_user_binary_via_phys(
    binary: &[u8],
    readonly_size: usize,
    mapper: &mut impl Mapper<Size4KiB>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
    phys_offset: VirtAddr,
) -> Result<u64, &'static str> {
    map_user_binary_inner(binary, readonly_size, mapper, frame_allocator, Some(phys_offset))
}

fn map_user_binary_inner(
    binary: &[u8],
    readonly_size: usize,
    mapper: &mut impl Mapper<Size4KiB>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
    phys_offset: Option<VirtAddr>,
) -> Result<u64, &'static str> {
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
                .map_err(|e| {
                    // Log the specific error for debugging. The `e` is a
                    // MapToError; we match on it to give a useful message.
                    use x86_64::structures::paging::mapper::MapToError;
                    let reason = match e {
                        MapToError::FrameAllocationFailed => "frame allocation failed",
                        MapToError::PageAlreadyMapped(_) => "page already mapped",
                        MapToError::ParentEntryHugePage => "parent entry is huge page",
                    };
                    crate::serial_println!(
                        "[process] map_user_binary: failed to map page {} (virt {:#x}): {}",
                        i,
                        page.start_address().as_u64(),
                        reason
                    );
                    "failed to map user binary page"
                })?
                .flush();
        }

        // Copy binary content into the mapped page.
        // If phys_offset is Some, write via physical address (no CR3 switch needed).
        // Otherwise, write via virtual address (caller must have switched CR3).
        let page_start = (i * 4096) as usize;
        let page_end = core::cmp::min(page_start + 4096, binary.len());
        let dest_ptr = if let Some(po) = phys_offset {
            (po.as_u64() + frame.start_address().as_u64()) as *mut u8
        } else {
            page.start_address().as_u64() as *mut u8
        };

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

    Ok(num_pages)
}

/// Allocates and maps user-accessible stack pages in the range
/// [`USER_STACK_BOTTOM`](super::USER_STACK_BOTTOM) to
/// [`USER_STACK_TOP`](super::USER_STACK_TOP).
pub(crate) fn map_user_stack(
    mapper: &mut impl Mapper<Size4KiB>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) -> Result<(), &'static str> {
    map_user_stack_inner(mapper, frame_allocator, None)
}

/// Maps the user stack, zeroing pages via physical addresses (no CR3 switch needed).
pub(crate) fn map_user_stack_via_phys(
    mapper: &mut impl Mapper<Size4KiB>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
    phys_offset: VirtAddr,
) -> Result<(), &'static str> {
    map_user_stack_inner(mapper, frame_allocator, Some(phys_offset))
}

fn map_user_stack_inner(
    mapper: &mut impl Mapper<Size4KiB>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
    phys_offset: Option<VirtAddr>,
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
        // If phys_offset is Some, write via physical address (no CR3 switch needed).
        let dest_ptr = if let Some(po) = phys_offset {
            (po.as_u64() + frame.start_address().as_u64()) as *mut u8
        } else {
            page.start_address().as_u64() as *mut u8
        };

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

/// Maps the framebuffer into user space.
///
/// Maps the physical framebuffer (whose address the bootloader reported) to
/// [`USER_FRAMEBUFFER`](super::USER_FRAMEBUFFER) with USER_ACCESSIBLE |
/// WRITABLE so user programs can draw directly to the screen.
pub(crate) fn map_user_framebuffer(
    mapper: &mut impl Mapper<Size4KiB>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) -> Result<(), &'static str> {
    use x86_64::PhysAddr;

    let flags =
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;

    // The framebuffer must not be cached for correct display.
    let flags = flags | PageTableFlags::NO_CACHE;

    let num_pages = userspace::FRAMEBUFFER_SIZE / 4096;
    let fb_virt_start = VirtAddr::new(userspace::USER_FRAMEBUFFER);
    let fb_phys_start = PhysAddr::new(crate::gfx::framebuffer_phys_addr());

    for i in 0..num_pages {
        let page: Page<Size4KiB> = Page::containing_address(fb_virt_start + i * 4096);
        let frame = PhysFrame::containing_address(fb_phys_start + i * 4096);

        // SAFETY:
        // The framebuffer physical pages are reserved by the hardware.
        // We map them with NO_CACHE for correct VGA behavior.
        unsafe {
            mapper
                .map_to(page, frame, flags, frame_allocator)
                .map_err(|_| "failed to map framebuffer page")?
                .flush();
        }
    }

    serial_println!(
        "[kernel] mapped framebuffer at {:#x} ({} pages)",
        userspace::USER_FRAMEBUFFER,
        num_pages,
    );

    Ok(())
}
