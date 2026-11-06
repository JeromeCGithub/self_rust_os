//! Owned user page-table parents for unpublished construction and live growth.
use alloc::vec::Vec;
use x86_64::{
    structures::paging::{
        page_table::PageTableEntry, FrameAllocator, PageTable, PageTableFlags, Size4KiB,
    },
    VirtAddr,
};
/// Ensure private user parent tables, recording every newly published parent
/// link so a failed live mapping can detach them before recycling its frames.
/// # Safety
/// Caller exclusively owns root/page-table mutation under kernel entry, supplies
/// a process-private subtree and RAM direct map, and rolls back recorded links
/// in reverse order before dropping a failed frame-allocation transaction.
pub(crate) unsafe fn ensure_tables(
    root: *mut PageTable,
    address: u64,
    allocator: &mut impl FrameAllocator<Size4KiB>,
    offset: VirtAddr,
    created: &mut Vec<*mut PageTableEntry>,
) -> Result<(), ()> {
    created.try_reserve(3).map_err(|_| ())?;
    let mut table = root;
    for shift in [39, 30, 21] {
        // SAFETY: Each table is the live exclusively owned root or an aligned
        // owned child reached below. Masking bounds the entry index to 0..512.
        let entry = unsafe { &mut (*table)[((address >> shift) & 511) as usize] };
        if entry.is_unused() {
            let frame = allocator.allocate_frame().ok_or(())?;
            let next = (offset + frame.start_address().as_u64()).as_mut_ptr::<PageTable>();
            // SAFETY: Fresh transaction-owned aligned RAM frame, not yet
            // published, exactly one table in bounds, no aliases or readers.
            unsafe {
                next.write(PageTable::new());
            }
            created.push(entry as *mut PageTableEntry);
            entry.set_frame(
                frame,
                PageTableFlags::PRESENT
                    | PageTableFlags::WRITABLE
                    | PageTableFlags::USER_ACCESSIBLE,
            );
        }
        if entry.flags().contains(PageTableFlags::HUGE_PAGE) {
            return Err(());
        }
        table = (offset + entry.frame().map_err(|_| ())?.start_address().as_u64()).as_mut_ptr();
    }
    Ok(())
}
/// Detach only links created during a failed operation. Leaves have already
/// been removed; the caller performs TLB shootdown before recycling ownership.
/// # Safety
/// Every pointer was recorded by ensure_tables under the same exclusive entry
/// operation; no intervening mapping mutation or page-table frame reclamation.
pub(crate) unsafe fn rollback_parents(created: &[*mut PageTableEntry]) {
    for &entry in created.iter().rev() {
        // SAFETY: Reverse order detaches children before their parents; frames
        // remain transaction-owned and mapped in the direct map until rollback.
        unsafe {
            (*entry).set_unused();
        }
    }
}
