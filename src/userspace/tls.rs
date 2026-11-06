//! Static x86-64 ELF TLS variant II: private template followed by self-pointer TCB.
use crate::process::TlsImage;
use x86_64::{
    structures::paging::{FrameAllocator, Mapper, Page, PageTableFlags, Size4KiB},
    VirtAddr,
};
pub(crate) const ARENA: u64 = 0x4_4000_0000;
const STRIDE: u64 = 2 * 1024 * 1024;
pub(crate) fn region(template: &TlsImage, slot: usize) -> (u64, u64, usize, usize) {
    let block = (template.memory_size + template.alignment - 1) & !(template.alignment - 1);
    let padding = (16 - block % 16) % 16;
    let base = ARENA + slot as u64 * STRIDE + 4096;
    (
        base,
        base + (padding + block) as u64,
        (padding + block + 16).div_ceil(4096),
        padding,
    )
}
/// Map one thread's static TLS into owned zero-filled pages. Caller preallocates
/// live parent tables, or constructs an entirely unpublished child transaction.
/// mapped counts successful leaves even on error, so callers can roll them back.
pub(crate) fn map(
    template: &TlsImage,
    slot: usize,
    mapper: &mut impl Mapper<Size4KiB>,
    allocator: &mut impl FrameAllocator<Size4KiB>,
    offset: VirtAddr,
    mapped: &mut usize,
) -> Result<(u64, u64), ()> {
    let (base, pointer, pages, padding) = region(template, slot);
    for index in 0..pages {
        let frame = allocator.allocate_frame().ok_or(())?;
        let physical = (offset + frame.start_address().as_u64()).as_mut_ptr::<u8>();
        let from = (index * 4096).max(padding);
        let to = ((index + 1) * 4096).min(padding + template.bytes.len());
        // SAFETY: Fresh uniquely owned frame in valid direct-mapped RAM, zeroed
        // before publication. Source is immutable kernel-owned template data;
        // intersections bound each copy to this page and the template slice.
        unsafe {
            core::ptr::write_bytes(physical, 0, 4096);
            if from < to {
                core::ptr::copy_nonoverlapping(
                    template.bytes[from - padding..to - padding].as_ptr(),
                    physical.add(from - index * 4096),
                    to - from,
                );
            }
            let start = base + index as u64 * 4096;
            if pointer >= start && pointer < start + 4096 {
                // Thread pointer is 16-byte aligned; this complete u64 is in
                // bounds of one page. Its self-pointer supports compiler fs:0.
                physical
                    .add((pointer - start) as usize)
                    .cast::<u64>()
                    .write_unaligned(pointer);
            }
        }
        // SAFETY: Distinct reserved TLS page, private owned zeroed frame, under
        // entry/scheduler ownership. Parents are private and allocated first.
        unsafe {
            mapper.map_to(
                Page::<Size4KiB>::containing_address(VirtAddr::new(base + index as u64 * 4096)),
                frame,
                PageTableFlags::PRESENT
                    | PageTableFlags::WRITABLE
                    | PageTableFlags::USER_ACCESSIBLE,
                allocator,
            )
        }
        .map_err(|_| ())?
        .ignore();
        *mapped += 1;
    }
    Ok((pointer, base))
}
