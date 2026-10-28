//! Interrupt handling module.
//! This module provides the implementation of the Interrupt Descriptor Table (IDT)
//! and the handlers for the interrupts, including the syscall handler for user mode.

use lazy_static::lazy_static;
use pc_keyboard::{layouts, HandleControl, ScancodeSet1};
use pic8259::ChainedPics;
use spin::Mutex;
use x86_64::{
    instructions::port::Port,
    structures::idt::{InterruptDescriptorTable, InterruptStackFrame},
    VirtAddr,
};

use crate::{gdt, println, task::keyboard, userspace};

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
                timer_entry as *const () as u64,
            ));
        }
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

use core::sync::atomic::{AtomicU64, Ordering};

/// Monotonic tick counter, incremented by the timer interrupt.
///
/// The PIT fires at ~18.2 Hz by default; each tick is ~54.9ms.
static TICKS: AtomicU64 = AtomicU64::new(0);

/// Returns the number of timer ticks since boot.
#[must_use]
pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: InterruptStackFrame) {
    TICKS.fetch_add(1, Ordering::Relaxed);

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

/// Naked timer interrupt entry point for preemptive scheduling.
///
/// Saves all GPRs in the canonical order (matching `syscall_entry`, forming
/// a `scheduler::SavedContext`), then calls [`timer_dispatch`]. If the
/// scheduler decides to preempt, the stack pointer is switched to the next
/// process before restoring registers and `iretq`.
///
/// Preemption only occurs when the interrupt came from user mode (checked
/// via the saved CS in `timer_dispatch`). Kernel-mode interrupts just
/// increment the tick counter, avoiding preemption of lock-holding code.
#[naked]
extern "x86-interrupt" fn timer_entry(_frame: InterruptStackFrame) {
    // SAFETY: Naked function with manual stack management.
    unsafe {
        core::arch::naked_asm!(
            "push rax",
            "push rbx",
            "push rcx",
            "push rdx",
            "push rsi",
            "push rdi",
            "push rbp",
            "push r8",
            "push r9",
            "push r10",
            "push r11",
            "push r12",
            "push r13",
            "push r14",
            "push r15",
            // rdi = rsp (points to SavedContext)
            "mov rdi, rsp",
            "call {dispatch}",
            // rax = new rsp (0 = no switch)
            "test rax, rax",
            "jz 2f",
            "mov rsp, rax",
            "2:",
            "pop r15",
            "pop r14",
            "pop r13",
            "pop r12",
            "pop r11",
            "pop r10",
            "pop r9",
            "pop r8",
            "pop rbp",
            "pop rdi",
            "pop rsi",
            "pop rdx",
            "pop rcx",
            "pop rbx",
            "pop rax",
            "iretq",
            dispatch = sym timer_dispatch,
        );
    }
}

/// Rust timer dispatch, called from [`timer_entry`] with the saved context.
///
/// # Returns
/// New kernel RSP to switch to, or 0 to resume the interrupted context.
extern "C" fn timer_dispatch(saved_rsp: u64) -> u64 {
    TICKS.fetch_add(1, Ordering::Relaxed);

    // Notify the PIC that the interrupt has been handled.
    // SAFETY: We are in the timer IRQ handler.
    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Timer.as_u8());
    }

    // Check if we interrupted user mode: CS is at saved_rsp + 0x80.
    // (15 saved regs * 8 = 0x78, then RIP at +0x78, CS at +0x80)
    let cs = unsafe { *((saved_rsp + 0x80) as *const u64) };
    if cs & 3 != 3 {
        // From kernel mode: no preemption.
        return 0;
    }

    // From user mode: maybe preempt.
    // DEBUG-BISECT: preemption disabled to isolate the timer switch path.
    let _ = saved_rsp;
    return 0;
    #[allow(unreachable_code)]
    crate::scheduler::on_timer(saved_rsp)
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

    // Wake any processes blocked waiting for keyboard input.
    crate::scheduler::wake_keyboard_waiters();

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
    let mut status_port = Port::<u8>::new(0x64);
    let mut data_port = Port::<u8>::new(0x60);

    // SAFETY: Reading the PS/2 status/data ports is side-effect free apart
    // from consuming the pending output byte, which is exactly what the
    // IRQ signals.
    unsafe {
        let status: u8 = status_port.read();
        // Only consume the byte if the output buffer is full and the byte
        // came from the mouse; a keyboard byte must never enter the mouse
        // packet assembler.
        if status & crate::mouse::STATUS_OUTPUT_READY != 0
            && status & crate::mouse::STATUS_MOUSE_DATA != 0
        {
            let byte: u8 = data_port.read();
            crate::mouse::handle_byte(byte);
        }
    }

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
