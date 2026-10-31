//! Interrupt handling module.
//! This module provides the implementation of the Interrupt Descriptor Table (IDT)
//! and the handlers for the interrupts, including the syscall handler for user mode.

use lazy_static::lazy_static;
use pic8259::ChainedPics;
use x86_64::{
    structures::idt::{InterruptDescriptorTable, InterruptStackFrame},
    VirtAddr,
};

use crate::{drivers::input::keyboard, gdt, println, userspace};

/// The offset for the Programmable Interrupt Controller (PIC) 1 (starting after interrupt table
/// max offset).
pub const PIC_1_OFFSET: u8 = 32;
/// The offset for the Programmable Interrupt Controller (PIC) 2.
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

/// The Programmable Interrupt Controller (PIC) used for handling hardware interrupts.
pub static PICS: spin::Mutex<ChainedPics> =
    spin::Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
/// Enum for the interrupt indices.
pub enum InterruptIndex {
    /// Timer interrupt index.
    Timer = PIC_1_OFFSET,
    /// Keyboard interrupt index.
    Keyboard,
    /// PS/2 mouse interrupt index (IRQ12 on the slave PIC).
    Mouse = PIC_2_OFFSET + 4,
}

impl InterruptIndex {
    /// Returns the index as a `u8`.
    #[must_use]
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    /// Returns the index as a `usize`.
    #[must_use]
    pub fn as_usize(self) -> usize {
        usize::from(self.as_u8())
    }
}

lazy_static! {
    static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();
        idt.breakpoint.set_handler_fn(breakpoint_handler);

        // SAFETY:
        // The double fault handler is safe to set as the index is valid.
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
        }
        // Timer uses a naked handler for preemptive context switches.
        // SAFETY: timer_entry is a naked function that preserves the
        // interrupt frame layout and returns via iretq.
        unsafe {
            idt[InterruptIndex::Timer.as_usize()].set_handler_addr(VirtAddr::new(
                crate::arch::x86_64::entry::timer_entry as *const () as u64,
            ));
        }
        // SAFETY: Architecture entries save/restore the canonical frame.
        // NMI has a CPU-private IST and never enters the ordinary kernel gate.
        unsafe {
            idt.page_fault.set_handler_addr(VirtAddr::new(crate::arch::x86_64::entry::page_fault_entry as *const () as u64));
            idt.general_protection_fault.set_handler_addr(VirtAddr::new(crate::arch::x86_64::entry::general_protection_entry as *const () as u64));
            idt[0x40].set_handler_addr(VirtAddr::new(crate::arch::x86_64::entry::local_timer_entry as *const () as u64));
            idt[0x41].set_handler_addr(VirtAddr::new(crate::arch::x86_64::entry::reschedule_entry as *const () as u64));
            idt.non_maskable_interrupt.set_handler_fn(nmi_handler).set_stack_index(gdt::NMI_IST_INDEX);
        }
        idt[0xff].set_handler_fn(spurious_handler);
        idt[InterruptIndex::Keyboard.as_usize()]
            .set_handler_fn(keyboard_interrupt_handler);
        idt[InterruptIndex::Mouse.as_usize()].set_handler_fn(mouse_interrupt_handler);

        // Register the syscall handler at interrupt vector 0x80.
        // DPL is set to Ring 3 so user-mode code can invoke it via `int 0x80`.
        userspace::syscall::register_syscall_handler(&mut idt);

        idt
    };
}

/// Initialize the Interrupt Descriptor Table (IDT).
pub fn init_idt() {
    IDT.load();
}

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    println!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    _error_code: u64,
) -> ! {
    println!("EXCEPTION: DOUBLE FAULT\n{:#?}", stack_frame);
    panic!("EXCEPTION: DOUBLE FAULT\n{:#?}", stack_frame);
}

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
static KEYBOARD_PENDING: AtomicBool = AtomicBool::new(false);

/// Consume bounded IRQ work before taking scheduler locks in ordinary entry.
pub fn take_pending_keyboard() -> bool {
    KEYBOARD_PENDING.swap(false, Ordering::AcqRel)
}

/// Monotonic tick counter, incremented by the timer interrupt.
///
/// The PIT fires at ~18.2 Hz by default; each tick is ~54.9ms.
static TICKS: AtomicU64 = AtomicU64::new(0);

/// Returns the number of timer ticks since boot.
#[must_use]
pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Advance the BSP PIT timebase, including while ordinary kernel code runs.
pub(crate) fn advance_ticks() {
    TICKS.fetch_add(1, Ordering::Relaxed);
    // SAFETY: Only the BSP receives IRQ0. Local IRQs are masked and all other
    // PIC access is likewise BSP-only, so acknowledgement cannot race itself.
    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Timer.as_u8());
    }
}
extern "x86-interrupt" fn nmi_handler(_frame: InterruptStackFrame) {
    crate::arch::x86_64::smp::invalidate_nmi();
}
extern "x86-interrupt" fn spurious_handler(_frame: InterruptStackFrame) {}

extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    keyboard::handle_interrupt();
    KEYBOARD_PENDING.store(true, Ordering::Release);

    // Notify the PICs that the interrupt has been handled.
    //
    // SAFETY:
    // Unsafe because accessing PICS directly can lead to undefined behavior if not done correctly.
    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Keyboard.as_u8());
    }
}

extern "x86-interrupt" fn mouse_interrupt_handler(_stack_frame: InterruptStackFrame) {
    crate::drivers::input::mouse::handle_interrupt();

    // Notify the PICs that the interrupt has been handled.
    //
    // SAFETY: We are in the mouse IRQ handler; the vector is valid.
    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Mouse.as_u8());
    }
}

#[cfg(test)]
mod tests {
    use x86_64::instructions::interrupts;

    use super::ticks;

    #[test_case]
    fn test_breakpoint_exception() {
        interrupts::int3();
    }

    #[test_case]
    fn test_ticks_is_monotonic() {
        let t1 = ticks();
        let t2 = ticks();
        assert!(t2 >= t1, "ticks() should be monotonic");
    }
}
