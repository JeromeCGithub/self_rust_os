//! Test for stack overflow handling in the kernel.
//! Should trigger a double fault and be handled by the kernel.

#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

use core::panic::PanicInfo;
use lazy_static::lazy_static;
use self_rust_os::{exit_qemu, gdt, serial_print, serial_println, QemuExitCode};
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame};

/// This test check if stack overflow is handled with a double fault.
/// # Panics
/// This test should panic if the stack overflows.
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    serial_print!("stack_overflow::stack_overflow...\t");

    gdt::init();
    init_test_idt();

    stack_overflow();

    panic!("Execution should not reach here");
}

#[expect(
    unconditional_recursion,
    reason = "This function is expected to cause a stack overflow."
)]
fn stack_overflow() {
    stack_overflow();
    volatile::Volatile::new(0).read();
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    self_rust_os::test_panic_handler(info)
}

lazy_static! {
    static ref TEST_IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();

        // SAFETY:
        // The double fault handler is safe to set as the index is valid.
        unsafe {
            idt.double_fault
                .set_handler_fn(test_double_fault_handler)
                .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
        }
        idt
    };
}

/// Initialize the test Interrupt Descriptor Table (IDT) for the test.
pub fn init_test_idt() {
    TEST_IDT.load();
}

extern "x86-interrupt" fn test_double_fault_handler(
    _stack_frame: InterruptStackFrame,
    _error_code: u64,
) -> ! {
    serial_println!("[ok]");
    exit_qemu(QemuExitCode::Success);

    #[expect(
        clippy::empty_loop,
        reason = "Endless loop when test finish. It should not be possible as we exit_qemu before."
    )]
    loop {}
}
