//! Mapping-aware validation shared by kernel syscall boundaries.
/// Validate every paging level and page in a user range, using a read-only entry
/// provider. Huge pages are permitted only with effective user permissions.
pub fn validate_with(
    root: u64,
    ptr: u64,
    len: u64,
    write: bool,
    mut entry: impl FnMut(u64, usize) -> Option<u64>,
) -> bool {
    if len == 0 {
        return true;
    }
    let Some(end) = ptr.checked_add(len) else {
        return false;
    };
    if ptr < 0x400000 || end > 0x6000000 || end < ptr {
        return false;
    }
    let last = (end - 1) & !4095;
    let mut page = ptr & !4095;
    loop {
        let mut table = root;
        for shift in [39, 30, 21, 12] {
            let Some(value) = entry(table, ((page >> shift) & 511) as usize) else {
                return false;
            };
            if value & 5 != 5 || (write && value & 2 == 0) {
                return false;
            }
            if shift == 12 {
                break;
            }
            if value & 128 != 0 {
                if shift == 39 {
                    return false;
                }
                break;
            }
            table = value & 0x000f_ffff_ffff_f000;
        }
        if page == last {
            return true;
        }
        page += 4096;
    }
}

/// Inspect the current process's tables without manufacturing aliasing mutable
/// mapper references. Caller holds kernel entry through validation and copying.
#[cfg(target_os = "none")]
pub fn validate_current(offset: u64, ptr: u64, len: u64, write: bool) -> bool {
    if len == 0 {
        return true;
    }
    if offset == 0 {
        return false;
    }
    let root = x86_64::registers::control::Cr3::read()
        .0
        .start_address()
        .as_u64();
    validate_with(root, ptr, len, write, |physical, index| {
        let address = offset
            .checked_add(physical)?
            .checked_add(index as u64 * 8)?;
        // SAFETY: CR3 and every followed present non-huge entry are kernel-owned
        // page-table frames in the RAM direct map established at boot. User code
        // cannot modify these supervisor mappings. Kernel entry serializes
        // mapping mutation through the copy; no user reference is formed.
        Some(unsafe { (address as *const u64).read_volatile() })
    })
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;
    fn entry(table: u64, index: usize) -> Option<u64> {
        match (table, index) {
            (0x1000, 0) => Some(0x2000 | 7),
            (0x2000, 0) => Some(0x3000 | 7),
            (0x3000, 2) => Some(0x4000 | 7),
            (0x4000, 0) => Some(0x5000 | 7),
            (0x4000, 1) => Some(0x6000 | 5),
            _ => None,
        }
    }
    #[test]
    fn empty_ranges_do_not_touch_pointer_or_page_tables() {
        for ptr in [0, u64::MAX, 0x6000000] {
            assert!(validate_with(0, ptr, 0, true, |_, _| panic!(
                "empty range must not inspect a table"
            )));
        }
    }
    #[test]
    fn assembly_copy_preserves_unaligned_tails_and_surrounding_bytes() {
        let source: [u8; 96] = core::array::from_fn(|i| i as u8);
        for len in 0..=80 {
            for source_offset in 0..8 {
                for destination_offset in 0..8 {
                    let mut actual = [0xa5u8; 96];
                    let mut expected = actual;
                    expected[destination_offset..destination_offset + len]
                        .copy_from_slice(&source[source_offset..source_offset + len]);
                    // SAFETY: Both complete ranges lie inside distinct live
                    // arrays, destination is exclusive, and x86 permits unaligned loads.
                    unsafe {
                        super::copy_raw(
                            actual.as_mut_ptr().add(destination_offset),
                            source.as_ptr().add(source_offset),
                            len,
                        );
                    }
                    assert_eq!(actual, expected);
                }
            }
        }
        // SAFETY: A zero-byte copy performs no memory access, including when
        // either address is null or noncanonical.
        unsafe {
            super::copy_raw(core::ptr::null_mut(), u64::MAX as *const u8, 0);
        }
    }
    #[test]
    fn validates_complete_range_and_write_permissions() {
        assert!(validate_with(0x1000, 0x400000, 4096, true, entry));
        assert!(validate_with(0x1000, 0x400ffe, 4, false, entry));
        assert!(!validate_with(0x1000, 0x400ffe, 4, true, entry));
        assert!(!validate_with(0x1000, 0x401ffe, 4, false, entry));
        assert!(!validate_with(0x1000, 0, 8, false, entry));
        assert!(!validate_with(0x1000, 0x400000, u64::MAX, false, entry));
        assert!(!validate_with(0x1000, 0xffff800000000000, 8, false, entry));
    }
    #[test]
    fn parent_user_and_write_flags_are_effective() {
        for bad_table in [0x1000, 0x2000, 0x3000, 0x4000] {
            assert!(!validate_with(0x1000, 0x400000, 8, false, |t, i| entry(
                t, i
            )
            .map(|v| if t == bad_table { v & !4 } else { v })));
            assert!(!validate_with(0x1000, 0x400000, 8, true, |t, i| entry(
                t, i
            )
            .map(|v| if t == bad_table { v & !2 } else { v })));
        }
    }
}

// Boot publishes this once before Ring 3; Acquire/Release makes publication
// explicit. The kernel entry gate stabilizes mappings through each copy.
#[cfg(target_os = "none")]
static PHYS_OFFSET: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// Publish the boot-established physical RAM mapping before user processes run.
#[cfg(target_os = "none")]
pub fn init(offset: u64) {
    PHYS_OFFSET.store(offset, core::sync::atomic::Ordering::Release);
}
/// Validate a current-process buffer against all effective paging permissions.
#[cfg(target_os = "none")]
pub fn validate(ptr: u64, len: u64, write: bool) -> bool {
    validate_current(
        PHYS_OFFSET.load(core::sync::atomic::Ordering::Acquire),
        ptr,
        len,
        write,
    )
}

/// Copy bytes across the architecture boundary without borrowing shared user RAM.
///
/// # Safety
/// Complete source/destination ranges must be mapped with appropriate access,
/// nonoverlapping, and kept mapped for the duration. Kernel destinations must
/// be exclusively owned initialized byte storage. User bytes may change on
/// sibling CPUs; the result is a byte snapshot, not an atomic typed transaction.
/// Zero length accesses neither address.
#[cfg(target_arch = "x86_64")]
pub unsafe fn copy_raw(destination: *mut u8, source: *const u8, len: usize) {
    // SAFETY: Caller supplies the range/ownership contract above. The assembly
    // has a memory clobber and creates no Rust references or typed validity
    // assumptions about concurrently modified user storage. Unaligned MOVSQ is
    // valid on x86; words plus the byte tail touch exactly len bytes. Bulk words
    // avoid per-byte TCG overhead in large surface snapshots.
    unsafe {
        core::arch::asm!(
            "cld", "rep movsq", "mov rcx,{tail}", "rep movsb",
            inout("rdi")destination=>_,inout("rsi")source=>_,
            inout("rcx")len/8=>_,tail=in(reg)len%8,options(nostack)
        );
    }
}
/// Validate and copy a complete readable user range into owned kernel bytes.
#[cfg(target_os = "none")]
pub fn from_user(source: u64, destination: &mut [u8]) -> bool {
    if !validate(source, destination.len() as u64, false) {
        return false;
    }
    // SAFETY: Entry gate keeps all validated mappings stable. Destination is a
    // unique kernel allocation, disjoint from all effective user mappings.
    unsafe {
        copy_raw(
            destination.as_mut_ptr(),
            source as *const u8,
            destination.len(),
        );
    }
    true
}
/// Validate and copy initialized kernel bytes to a complete writable user range.
#[cfg(target_os = "none")]
pub fn to_user(destination: u64, source: &[u8]) -> bool {
    if !validate(destination, source.len() as u64, true) {
        return false;
    }
    // SAFETY: Same lifetime/disjointness invariant as from_user; byte assembly
    // permits hostile overlapping application writes without kernel references.
    unsafe {
        copy_raw(destination as *mut u8, source.as_ptr(), source.len());
    }
    true
}
