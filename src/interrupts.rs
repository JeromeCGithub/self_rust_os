//! Interrupt handling module
//! This module provides the implementation of the Interrupt Descriptor Table (IDT)
//! and the handlers for the interrupts.

use lazy_static::lazy_static;
use pc_keyboard::{layouts, HandleControl, ScancodeSet1};
use pic8259::ChainedPics;
use spin::Mutex;
use x86_64::{
    instructions::port::Port,
    structures::idt::{InterruptDescriptorTable, InterruptStackFrame},
};

use crate::{gdt, print, println, task::keyboard};

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
        idt[InterruptIndex::Timer.as_usize()]
            .set_handler_fn(timer_interrupt_handler);
        idt[InterruptIndex::Keyboard.as_usize()]
            .set_handler_fn(keyboard_interrupt_handler);
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

extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: InterruptStackFrame) {
    // Print a dot to indicate a timer interrupt has occurred.
    #[cfg(debug_assertions)]
    print!(".");

    // Notify the PICs that the interrupt has been handled.
    //
    // SAFETY:
    // We are notifying the PIC that the interrupt has been handled.
    // Can fail if the PIC is not initialized.
    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Timer.as_u8());
    }
}

extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    lazy_static! {
        static ref KEYBOARD: spin::Mutex<pc_keyboard::Keyboard<layouts::Us104Key, ScancodeSet1>> =
            Mutex::new(pc_keyboard::Keyboard::new(
                ScancodeSet1::new(),
                layouts::Us104Key,
                HandleControl::Ignore,
            ));
    }
    let mut port = Port::new(0x60);

    // Read the scancode from the keyboard port.
    //
    // SAFETY:
    // We are reading from the keyboard port which is known to be safe.
    let scancode: u8 = unsafe { port.read() };
    keyboard::add_scancode(scancode);

    // Notify the PICs that the interrupt has been handled.
    //
    // SAFETY:
    // Unsafe because accessing PICS directly can lead to undefined behavior if not done correctly.
    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Keyboard.as_u8());
    }
}

#[cfg(test)]
mod tests {
    use x86_64::instructions::interrupts;

    #[test_case]
    fn test_breakpoint_exception() {
        interrupts::int3();
    }
}
