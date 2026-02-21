//! GDT and TSS initialization.
//! This module sets up the Global Descriptor Table (GDT) and the Task State Segment (TSS).
//! It includes both kernel and user mode segment descriptors to support Ring 3 execution.

use lazy_static::lazy_static;
use x86_64::{
    instructions::tables::load_tss,
    registers::segmentation::{Segment, CS},
    structures::{
        gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector},
        tss::TaskStateSegment,
    },
    VirtAddr,
};

/// The index of the IST entry for the double fault handler.
pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

/// Size of the kernel stack used when transitioning from Ring 3 to Ring 0.
const KERNEL_STACK_SIZE: usize = 4096 * 5;

/// Size of the double fault handler stack.
const DOUBLE_FAULT_STACK_SIZE: usize = 4096 * 5;

/// Initialize the Global Descriptor Table (GDT).
pub fn init() {
    GDT.0.load();

    // SAFETY:
    // The GDT is loaded and the CS register is set to the code selector.
    unsafe {
        CS::set_reg(GDT.1.code_selector);
        load_tss(GDT.1.tss_selector);
    }
}

/// Returns the user code segment selector with Ring 3 privilege level.
#[must_use]
pub fn user_code_selector() -> SegmentSelector {
    GDT.1.user_code_selector
}

/// Returns the user data segment selector with Ring 3 privilege level.
#[must_use]
pub fn user_data_selector() -> SegmentSelector {
    GDT.1.user_data_selector
}

lazy_static! {
    static ref TSS: TaskStateSegment = {
        let mut tss = TaskStateSegment::new();

        // Set up a dedicated stack for double fault exceptions.
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
            static mut STACK: [u8; DOUBLE_FAULT_STACK_SIZE] = [0; DOUBLE_FAULT_STACK_SIZE];

            // SAFETY:
            // We only take a const pointer to compute the stack bounds.
            let stack_start = VirtAddr::from_ptr(&raw const STACK);
            stack_start + DOUBLE_FAULT_STACK_SIZE as u64
        };

        // Set up the kernel stack pointer (RSP0) used when the CPU transitions
        // from Ring 3 to Ring 0 on interrupts or exceptions.
        tss.privilege_stack_table[0] = {
            static mut KERNEL_STACK: [u8; KERNEL_STACK_SIZE] = [0; KERNEL_STACK_SIZE];

            // SAFETY:
            // We only take a const pointer to compute the stack bounds.
            let stack_start = VirtAddr::from_ptr(&raw const KERNEL_STACK);
            stack_start + KERNEL_STACK_SIZE as u64
        };

        tss
    };
}

/// Holds the GDT segment selectors used during initialization and user mode transitions.
struct Selectors {
    code_selector: SegmentSelector,
    tss_selector: SegmentSelector,
    user_code_selector: SegmentSelector,
    user_data_selector: SegmentSelector,
}

lazy_static! {
    static ref GDT: (GlobalDescriptorTable, Selectors) = {
        let mut gdt = GlobalDescriptorTable::new();
        let code_selector = gdt.add_entry(Descriptor::kernel_code_segment());
        let tss_selector = gdt.add_entry(Descriptor::tss_segment(&TSS));

        // Add user mode segments. The selectors returned by `add_entry` have RPL=0,
        // so we create new selectors with RPL=3 for Ring 3 execution.
        let user_data_entry = gdt.add_entry(Descriptor::user_data_segment());
        let user_code_entry = gdt.add_entry(Descriptor::user_code_segment());

        let user_data_selector = SegmentSelector(user_data_entry.0 | 3);
        let user_code_selector = SegmentSelector(user_code_entry.0 | 3);

        (
            gdt,
            Selectors {
                code_selector,
                tss_selector,
                user_code_selector,
                user_data_selector,
            },
        )
    };
}
