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
    let Some(end) = ptr.checked_add(len) else {
        return false;
    };
    if ptr < 0x400000 || end > 0x4000000 || end < ptr {
        return false;
    }
    if len == 0 {
        return true;
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
/// mapper references. Caller must remain in a nonpreemptible syscall until copy.
#[cfg(target_os = "none")]
pub fn validate_current(offset: u64, ptr: u64, len: u64, write: bool) -> bool {
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
        // cannot modify these supervisor mappings. Syscalls run with interrupts
        // disabled on one CPU; tables cannot change between validation and copy.
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
// explicit. Validation/copy still relies on nonpreemptible single-CPU syscalls.
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
