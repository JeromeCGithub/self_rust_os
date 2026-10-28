//! GRUB2 / Multiboot2 boot support.
//!
//! This module implements the boot path used when the `grub` cargo feature is
//! enabled. Instead of the `bootloader` crate's `cargo bootimage` flow, the
//! kernel is loaded from a FAT32 disk image by GRUB2 using the Multiboot2
//! protocol:
//!
//! ```text
//! QEMU BIOS -> GRUB (MBR) -> reads /boot/kernel.elf from FAT32
//!          -> jumps to `multiboot2_entry` in 32-bit protected mode
//!          -> stub builds page tables, switches to 64-bit long mode
//!          -> `grub_entry_rust(magic, info_ptr)`
//! ```
//!
//! The stub identity-maps the first GiB of physical memory with 2 MiB pages,
//! which is enough to reach the Rust entry point. Rust code then parses the
//! Multiboot2 information structure, maps all physical RAM at
//! [`PHYS_MEM_OFFSET`], and continues with the normal kernel initialization.

/// Multiboot2 information parsing. Compiled in tests too so the parser's unit
/// tests run under the normal `cargo test` harness (no GRUB needed).
#[cfg(any(feature = "grub", test))]
pub mod multiboot2;

/// Virtual address at which all physical memory is mapped (higher half).
///
/// Chosen to match the classic higher-half layout: the top 512 GiB of the
/// address space (`P4[511]`) maps physical `0x0..0x80_0000_0000` with 2 MiB
/// pages. GRUB itself is entered with paging disabled, so this mapping is
/// built by the early boot code before the rest of the kernel runs.
#[cfg(feature = "grub")]
pub const PHYS_MEM_OFFSET: u64 = 0xFFFF_8000_0000_0000;

/// Maximum physical memory mapped at [`PHYS_MEM_OFFSET`] (4 GiB).
///
/// The mapping uses statically allocated page tables (one P3 + four P2
/// tables = 20 KiB in `.bss`), so the cap is fixed at build time. 4 GiB is
/// far above the RAM sizes used for testing (128 MiB in QEMU).
#[cfg(feature = "grub")]
pub const MAX_MAPPED_PHYS_MEM: u64 = 4 * 1024 * 1024 * 1024;

#[cfg(feature = "grub")]
mod paging {
    use x86_64::{
        registers::control::{Cr3, Cr3Flags},
        structures::paging::PhysFrame,
    };

    use super::{MAX_MAPPED_PHYS_MEM, PHYS_MEM_OFFSET};
    use crate::memory::MemRegion;

    /// A single 4 KiB page table.
    #[repr(align(4096))]
    struct AlignedTable([u64; 512]);

    // Statically allocated page tables for the higher-half mapping.
    //
    // The boot stub already identity-maps the first GiB with its own tables;
    // here we add `P4[256] -> P3_HIGH`, and each `P3_HIGH[i]` maps one GiB
    // with 2 MiB huge pages through `P2_HIGH_TABLES[i]`. P4[256] is the
    // entry for `PHYS_MEM_OFFSET` (0xFFFF_8000_0000_0000): bits 47:39 = 0x100.
    static mut P3_HIGH: AlignedTable = AlignedTable([0; 512]);
    static mut P2_HIGH_TABLES: [AlignedTable; 4] = [
        AlignedTable([0; 512]),
        AlignedTable([0; 512]),
        AlignedTable([0; 512]),
        AlignedTable([0; 512]),
    ];

    const PTE_PRESENT: u64 = 1 << 0;
    const PTE_WRITABLE: u64 = 1 << 1;
    const PTE_HUGE: u64 = 1 << 7;

    const PAGE_2MIB: u64 = 2 * 1024 * 1024;
    const GIB: u64 = 1024 * 1024 * 1024;

    /// Returns `true` when the half-open range `[start, end)` overlaps any
    /// of the given ranges.
    fn overlaps_any(start: u64, end: u64, ranges: &[(u64, u64)]) -> bool {
        ranges
            .iter()
            .any(|&(rs, re)| start < re && rs < end)
    }

    /// Maps physical memory at [`PHYS_MEM_OFFSET`](super::PHYS_MEM_OFFSET).
    ///
    /// Every 2 MiB page that overlaps usable RAM (or one of `extra_ranges`,
    /// e.g. the framebuffer) is mapped with 2 MiB huge pages. The boot stub's
    /// identity mapping is left untouched; the same P4 is extended.
    ///
    /// Returns the number of bytes mapped.
    ///
    /// # Safety
    ///
    /// Must be called once, from the boot stub's 64-bit entry, while the
    /// identity mapping is active and paging is enabled. `extra_ranges` are
    /// mapped as writable device memory (used for the framebuffer).
    pub unsafe fn map_all_physical_memory(
        regions: &[MemRegion],
        extra_ranges: &[(u64, u64)],
    ) -> u64 {
        // Collect the ranges to map: usable RAM, capped at MAX_MAPPED_PHYS_MEM.
        let mut ranges: [(u64, u64); 70] = [(0, 0); 70];
        let mut n = 0_usize;
        for r in regions {
            if r.start >= MAX_MAPPED_PHYS_MEM {
                continue;
            }
            let end = r.end.min(MAX_MAPPED_PHYS_MEM);
            if r.start < end && n < ranges.len() {
                ranges[n] = (r.start, end);
                n += 1;
            }
        }
        for &extra in extra_ranges {
            if extra.0 < MAX_MAPPED_PHYS_MEM && n < ranges.len() {
                ranges[n] = (extra.0, extra.1.min(MAX_MAPPED_PHYS_MEM));
                n += 1;
            }
        }
        let ranges = &ranges[..n];

        // SAFETY: Single-threaded boot; tables are only touched here.
        // The statics live below 1 GiB (kernel image), so their virtual
        // address equals their physical address under the identity mapping.
        unsafe {
            let p3 = &mut *core::ptr::addr_of_mut!(P3_HIGH);
            let p3_phys = core::ptr::addr_of!(P3_HIGH) as u64;
            assert!(
                p3_phys < GIB,
                "higher-half page tables must live below 1 GiB"
            );

            // Read the active P4 through the identity mapping.
            let (p4_frame, _) = Cr3::read();
            let p4 = &mut *(p4_frame.start_address().as_u64() as *mut AlignedTable);

            p4.0[256] = p3_phys | PTE_PRESENT | PTE_WRITABLE;

            let p2_tables = core::ptr::addr_of_mut!(P2_HIGH_TABLES);
            let mut mapped_bytes = 0_u64;
            for i in 0..4_usize {
                let p2 = &mut (*p2_tables)[i];
                let p2_phys = core::ptr::addr_of!((*p2_tables)[i]) as u64;
                p3.0[i] = p2_phys | PTE_PRESENT | PTE_WRITABLE;
                let gib_base = i as u64 * GIB;
                for (j, entry) in p2.0.iter_mut().enumerate() {
                    let page_start = gib_base + j as u64 * PAGE_2MIB;
                    if overlaps_any(page_start, page_start + PAGE_2MIB, ranges) {
                        *entry = page_start | PTE_PRESENT | PTE_WRITABLE | PTE_HUGE;
                        mapped_bytes += PAGE_2MIB;
                    }
                }
            }

            // Full TLB flush so the new mappings take effect.
            Cr3::write(p4_frame, Cr3Flags::empty());

            // Sanity: the offset mapping must actually work.
            let probe = PHYS_MEM_OFFSET as *const u64;
            let _ = probe.read_volatile();

            mapped_bytes
        }
    }

    /// The physical frame of the active P4 (for diagnostics).
    pub fn active_p4_frame() -> PhysFrame {
        Cr3::read().0
    }
}

#[cfg(feature = "grub")]
pub use paging::{active_p4_frame, map_all_physical_memory};
