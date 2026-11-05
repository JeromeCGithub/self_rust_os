//! User-space memory mapping primitives.
//!
//! This module provides functions to map user program binaries, stacks, and
//! the framebuffer into a page table. Used by the scheduler when creating
//! new processes with their own address spaces.
//!
//! Static ELF images preserve segment file/memory sizes and load permissions.
//! Legacy flat images remain supported at [`USER_CODE_START`](super::USER_CODE_START).

use x86_64::{
    structures::paging::{FrameAllocator, Mapper, Page, PageTableFlags, PhysFrame, Size4KiB},
    VirtAddr,
};

use crate::{println, serial_println, userspace};

/// Populate a validated unpublished image using private owned frames. ELF BSS
/// is zeroed in RAM; the file carries only initialized bytes. The caller's frame
/// transaction rolls back every allocation if any segment fails to map.
pub(crate) fn map_user_image_via_phys(
    image: &rustos_exec_image::Image<'_>,
    readonly_size: usize,
    mapper: &mut impl Mapper<Size4KiB>,
    allocator: &mut impl FrameAllocator<Size4KiB>,
    phys_offset: VirtAddr,
) -> Result<(), &'static str> {
    if !image.is_elf() {
        let segment = image.segments().next().expect("validated flat image");
        return map_user_binary_via_phys(
            segment.bytes,
            readonly_size,
            mapper,
            allocator,
            phys_offset,
        )
        .map(|_| ());
    }
    let mut pages = 0;
    let mut mapped_until = 0;
    for segment in image.segments() {
        let start = (segment.address & !4095).max(mapped_until);
        let end = (segment.address + segment.memory_size + 4095) & !4095;
        for address in (start..end).step_by(4096) {
            let frame = allocator
                .allocate_frame()
                .ok_or("ELF frame allocation failed")?;
            let destination = (phys_offset + frame.start_address().as_u64()).as_mut_ptr::<u8>();
            let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
            // SAFETY: A fresh exclusively owned, direct-mapped RAM frame is
            // initialized before publication. No application or other CPU can
            // access it; the transaction records its complete ownership.
            unsafe {
                core::ptr::write_bytes(destination, 0, 4096);
            }
            // Distinct byte ranges may share a page (e.g. rodata and GOT).
            // Map it once, union its writable permission, then copy each file
            // intersection without overwriting another segment with BSS zeros.
            for part in image.segments() {
                if part.address < address + 4096
                    && part.address + part.memory_size > address
                    && part.writable
                {
                    flags |= PageTableFlags::WRITABLE;
                }
                let first = address.max(part.address);
                let last = (address + 4096).min(part.address + part.bytes.len() as u64);
                if first < last {
                    let file_offset = (first - part.address) as usize;
                    let count = (last - first) as usize;
                    // SAFETY: The validated file intersection is fully within
                    // this immutable kernel buffer and the uniquely owned page.
                    // Both offsets and count are bounded above; the staged file
                    // and fresh physical destination are disjoint allocations.
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            part.bytes[file_offset..file_offset + count].as_ptr(),
                            destination.add((first - address) as usize),
                            count,
                        );
                    }
                }
            }
            let page = Page::<Size4KiB>::containing_address(VirtAddr::new(address));
            // SAFETY: All segment byte ranges and rounded page bounds were
            // validated before allocation. mapped_until makes this leaf unique;
            // all frames/page tables are private and unpublished. No CPU has
            // used this CR3, and dispatch reloads it before user execution, so
            // there is no cached translation to invalidate during construction.
            unsafe {
                mapper
                    .map_to(page, frame, flags, allocator)
                    .map_err(|_| "ELF mapping failed")?
                    .ignore();
            }
            pages += 1;
        }
        mapped_until = end;
    }
    serial_println!(
        "[kernel] mapped ELF image: {} pages; {} file bytes",
        pages,
        image
            .segments()
            .map(|segment| segment.bytes.len())
            .sum::<usize>()
    );
    Ok(())
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
    map_user_binary_inner(
        binary,
        readonly_size,
        mapper,
        frame_allocator,
        Some(phys_offset),
    )
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
    // These code/rodata pages are read only. Writable pages still permit
    // execution because this path does not apply NO_EXECUTE; this is not W^X.
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
/// The compositor maps the physical display. Other processes receive private,
/// zero-filled frames at the same virtual address, isolating client drawing.
/// Returns the private frames so the compositor can request snapshots.
pub(crate) fn map_user_framebuffer(
    mapper: &mut impl Mapper<Size4KiB>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
    display_owner: bool,
    phys_offset: VirtAddr,
) -> Result<alloc::vec::Vec<PhysFrame>, &'static str> {
    use x86_64::PhysAddr;

    let flags =
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;

    // The framebuffer must not be cached for correct display.
    let flags = if display_owner {
        flags | PageTableFlags::NO_CACHE
    } else {
        flags
    };
    let mut frames = alloc::vec::Vec::new();

    let num_pages = userspace::framebuffer_pages(&crate::drivers::video::framebuffer::fb_info())?;
    if !display_owner {
        frames
            .try_reserve_exact(num_pages as usize)
            .map_err(|_| "surface metadata allocation failed")?;
    }
    let fb_virt_start = VirtAddr::new(userspace::USER_FRAMEBUFFER);
    let fb_phys_start = PhysAddr::new(crate::drivers::video::framebuffer::framebuffer_phys_addr());

    for i in 0..num_pages {
        let page: Page<Size4KiB> = Page::containing_address(fb_virt_start + i * 4096);
        let frame = if display_owner {
            PhysFrame::containing_address(fb_phys_start + i * 4096)
        } else {
            let frame = frame_allocator
                .allocate_frame()
                .ok_or("surface allocation failed")?;
            // SAFETY: Newly allocated frame, mapped by the physical memory offset.
            unsafe {
                core::ptr::write_bytes(
                    (phys_offset + frame.start_address().as_u64()).as_mut_ptr::<u8>(),
                    0,
                    4096,
                );
            }
            frames.push(frame);
            frame
        };

        // SAFETY: This process owns an empty user page-table subtree. The
        // frame is either reserved display MMIO (PID 0, NO_CACHE) or freshly
        // allocated private RAM. The scheduler serializes mapping changes
        // with interrupts disabled, and each new mapping's TLB is flushed.
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

    Ok(frames)
}

/// Initialize argv through the RAM direct map before the child's page tables
/// or initial thread are published. All argument bytes are kernel-owned.
pub(crate) fn prepare_arguments(
    mapper: &impl x86_64::structures::paging::Translate,
    phys_offset: VirtAddr,
    argv: &[&str],
    envp: &[&str],
) -> Result<rustos_process_abi::Startup, &'static str> {
    let size =
        rustos_process_abi::startup_size(argv, envp).map_err(|_| "invalid startup vectors")?;
    let mut bytes = alloc::vec::Vec::new();
    bytes
        .try_reserve_exact(size)
        .map_err(|_| "startup staging allocation failed")?;
    bytes.resize(size, 0);
    let startup =
        rustos_process_abi::write_startup(userspace::USER_STACK_TOP, argv, envp, &mut bytes)
            .map_err(|_| "invalid startup layout")?;
    for (index, source) in bytes.chunks_exact(4096).enumerate() {
        let address = userspace::USER_STACK_TOP - size as u64 + index as u64 * 4096;
        let physical = mapper
            .translate_addr(VirtAddr::new(address))
            .ok_or("startup page missing")?;
        let destination = phys_offset + physical.as_u64();
        // SAFETY: Each destination is a fresh private aligned stack frame in
        // the unpublished transaction-owned child. Exactly one initialized page
        // is copied from distinct owned staging RAM through the direct map.
        // No user/CPU can access the child yet; failure rolls back all frames.
        unsafe {
            core::ptr::copy_nonoverlapping(source.as_ptr(), destination.as_mut_ptr::<u8>(), 4096);
        }
    }
    Ok(startup)
}
