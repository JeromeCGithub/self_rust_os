//! CPU-owned GDT/TSS and exception stacks. Only BSP setup precedes the heap.
use super::{acpi::MAX_CPUS, smp};
use core::{
    cell::UnsafeCell,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};
use x86_64::{
    instructions::tables::load_tss,
    registers::segmentation::{Segment, CS, DS, ES, SS},
    structures::{
        gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector},
        tss::TaskStateSegment,
    },
    VirtAddr,
};

/// Double-fault IST entry.
pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;
/// Lock-independent TLB-invalidation NMI IST entry.
pub const NMI_IST_INDEX: u16 = 1;
#[repr(align(16))]
struct Stack([u8; 16384]);
struct CpuTables {
    gdt: GlobalDescriptorTable,
    tss: TaskStateSegment,
    fault: Stack,
    nmi: Stack,
    boot: Stack,
}
struct Tables(UnsafeCell<CpuTables>);
// SAFETY: Each element is written only by its owning CPU, with local IRQs off.
// NMI only reads its established IST stack pointer. Elements never move.
unsafe impl Sync for Tables {}
static BSP_TABLES: Tables = Tables(UnsafeCell::new(CpuTables {
    gdt: GlobalDescriptorTable::new(),
    tss: TaskStateSegment::new(),
    fault: Stack([0; 16384]),
    nmi: Stack([0; 16384]),
    boot: Stack([0; 16384]),
}));
static POINTERS: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static INITIALIZED: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];

/// Install this CPU's immutable descriptor table and owned mutable TSS.
pub fn init() {
    let cpu = smp::current_cpu();
    assert!(
        !INITIALIZED[cpu].swap(true, Ordering::AcqRel),
        "GDT initialized twice"
    );
    let pointer = if cpu == 0 {
        BSP_TABLES.0.get()
    } else {
        // AP CPU-lifetime tables live on the higher-half heap. Keeping all 32
        // exception stacks in low static data would overlap USER_CODE_START in
        // the legacy debug link, corrupting kernel mappings during process spawn.
        let layout = alloc::alloc::Layout::new::<CpuTables>();
        // SAFETY: Fresh, correctly aligned allocation. Initialize fields in
        // place to avoid a 48 KiB temporary on the AP's startup stack. This
        // allocation is retained for the CPU lifetime, bounded by MAX_CPUS.
        unsafe {
            let p = alloc::alloc::alloc(layout).cast::<CpuTables>();
            assert!(!p.is_null(), "AP descriptor allocation");
            core::ptr::write_bytes(p.cast::<u8>(), 0, layout.size());
            core::ptr::addr_of_mut!((*p).gdt).write(GlobalDescriptorTable::new());
            core::ptr::addr_of_mut!((*p).tss).write(TaskStateSegment::new());
            p
        }
    };
    POINTERS[cpu].store(pointer as u64, Ordering::Release);
    // SAFETY: The owning CPU initializes its stable element once before loading
    // it. No reference to the mutable TSS survives this initialization.
    unsafe {
        let tables = &mut *pointer;
        tables.tss.interrupt_stack_table[0] =
            VirtAddr::from_ptr(tables.fault.0.as_ptr()) + 16384u64;
        tables.tss.interrupt_stack_table[1] = VirtAddr::from_ptr(tables.nmi.0.as_ptr()) + 16384u64;
        tables.tss.privilege_stack_table[0] = VirtAddr::from_ptr(tables.boot.0.as_ptr()) + 16384u64;
        tables.gdt.add_entry(Descriptor::kernel_code_segment());
        tables
            .gdt
            .add_entry(Descriptor::tss_segment(&*core::ptr::addr_of!(tables.tss)));
        tables.gdt.add_entry(Descriptor::user_data_segment());
        tables.gdt.add_entry(Descriptor::user_code_segment());
        tables.gdt.add_entry(Descriptor::kernel_data_segment());
        tables.gdt.load();
        CS::set_reg(SegmentSelector(8));
        SS::set_reg(SegmentSelector(0x30));
        DS::set_reg(SegmentSelector(0x30));
        ES::set_reg(SegmentSelector(0x30));
        load_tss(SegmentSelector(16));
    }
}
/// Shared selector layout, installed independently on every CPU.
pub fn user_code_selector() -> SegmentSelector {
    SegmentSelector(0x2b)
}
/// Shared Ring 3 data selector.
pub fn user_data_selector() -> SegmentSelector {
    SegmentSelector(0x23)
}
/// Update this CPU's RSP0 with IRQs disabled during the switch.
pub fn set_kernel_stack(top: VirtAddr) {
    assert!(!x86_64::instructions::interrupts::are_enabled());
    // SAFETY: CPU-exclusive TSS in UnsafeCell; top is an owned live thread stack
    // supplied by the scheduler. NMI uses a separate IST, not RSP0.
    unsafe {
        (*(POINTERS[smp::current_cpu()].load(Ordering::Acquire) as *mut CpuTables))
            .tss
            .privilege_stack_table[0] = top;
    }
}
