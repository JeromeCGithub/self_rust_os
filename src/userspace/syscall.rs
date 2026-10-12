//! Syscall handler module.
//!
//! Provides the interrupt-based syscall interface for user-mode programs.
//! User programs invoke syscalls via `int 0x80` with the following register convention:
//!
//! - `rax`: syscall number
//! - `rdi`: first argument
//! - `rsi`: second argument
//! - `rdx`: third argument
//!
//! The return value is placed in `rax`.
//!
//! Besides process control (`sys_exit`) and console output (`sys_write`),
//! the interface exposes the in-memory filesystem (see [`crate::fs`]) so
//! user programs can create, read, and write files.

use core::arch::naked_asm;

use x86_64::{structures::idt::InterruptStackFrame, VirtAddr};

use crate::{fs, print, println, serial_println, userspace};

/// Syscall number for `sys_exit`: terminates the current user process.
pub const SYS_EXIT: u64 = 0;

/// Syscall number for `sys_write`: writes a buffer to the VGA text display.
pub const SYS_WRITE: u64 = 1;

/// Syscall number for `sys_open`: opens (or creates) a file, returns a file descriptor.
pub const SYS_OPEN: u64 = 2;

/// Syscall number for `sys_close`: closes an open file descriptor.
pub const SYS_CLOSE: u64 = 3;

/// Syscall number for `sys_read`: reads bytes from an open file.
pub const SYS_READ: u64 = 4;

/// Syscall number for `sys_fwrite`: writes bytes to an open file.
pub const SYS_FWRITE: u64 = 5;

/// Naked entry point for the `int 0x80` syscall interrupt.
///
/// This function saves all general-purpose registers, extracts the syscall
/// arguments from the saved register state, calls the Rust dispatch function,
/// and then either returns to user mode via `iretq` or halts if the process
/// exited.
///
/// # Register layout on the stack after all pushes
///
/// ```text
/// rsp + 0x00 : r15
/// rsp + 0x08 : r14
/// rsp + 0x10 : r13
/// rsp + 0x18 : r12
/// rsp + 0x20 : r11
/// rsp + 0x28 : r10
/// rsp + 0x30 : r9
/// rsp + 0x38 : r8
/// rsp + 0x40 : rbp
/// rsp + 0x48 : rdi  (arg1)
/// rsp + 0x50 : rsi  (arg2)
/// rsp + 0x58 : rdx  (arg3)
/// rsp + 0x60 : rcx
/// rsp + 0x68 : rbx
/// rsp + 0x70 : rax  (syscall number)
/// rsp + 0x78 : RIP  (pushed by CPU)
/// rsp + 0x80 : CS
/// rsp + 0x88 : RFLAGS
/// rsp + 0x90 : RSP
/// rsp + 0x98 : SS
/// ```
#[naked]
pub(crate) extern "x86-interrupt" fn syscall_entry(_frame: InterruptStackFrame) {
    // SAFETY:
    //
    // This naked function manually manages the entire register save/restore
    // and stack layout. The CPU has already pushed SS, RSP, RFLAGS, CS, RIP
    // before entering this handler. We save all GPRs, set up the C calling
    // convention arguments, and call the Rust handler.
    unsafe {
        naked_asm!(
            // Save all general-purpose registers.
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

            // Set up arguments for the Rust syscall dispatcher:
            //   rdi = syscall number (was in rax)
            //   rsi = arg1 (was in rdi)
            //   rdx = arg2 (was in rsi)
            //   rcx = arg3 (was in rdx)
            "mov rdi, [rsp + 0x70]",  // saved rax -> syscall number
            "mov rsi, [rsp + 0x48]",  // saved rdi -> arg1
            "mov rdx, [rsp + 0x50]",  // saved rsi -> arg2
            "mov rcx, [rsp + 0x58]",  // saved rdx -> arg3

            "call {dispatch}",

            // Check if the process should exit (magic sentinel value).
            "mov rcx, {sentinel}",
            "cmp rax, rcx",
            "je 2f",

            // Normal return: store result in the saved rax slot.
            "mov [rsp + 0x70], rax",

            // Restore all general-purpose registers.
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

            // Process exit path: restore kernel context saved by switch_to_user_mode.
            //
            // The current stack (TSS RSP0) is abandoned. We load the kernel RSP
            // that was saved before iretq, pop the callee-saved registers that
            // switch_to_user_mode pushed, restore kernel data segments, and ret
            // back into process::run.
            "2:",
            "mov rsp, [{kernel_rsp}]",

            // Pop callee-saved registers (reverse of switch_to_user_mode pushes).
            "pop r15",
            "pop r14",
            "pop r13",
            "pop r12",
            "pop rbp",
            "pop rbx",

            // Restore kernel data segments (long mode typically uses 0).
            "xor ax, ax",
            "mov ds, ax",
            "mov es, ax",
            "mov fs, ax",
            "mov gs, ax",

            // Return from switch_to_user_mode back to process::run.
            "ret",

            dispatch = sym syscall_dispatch,
            kernel_rsp = sym super::process::KERNEL_RSP,
            sentinel = const PROCESS_EXIT_SENTINEL,
        );
    }
}

/// Sentinel value returned by [`syscall_dispatch`] to signal that the current
/// process has called `sys_exit` and execution should not return to user mode.
const PROCESS_EXIT_SENTINEL: u64 = u64::MAX;

/// Error value returned to the user program when a syscall number is unknown.
const SYSCALL_ERROR: u64 = u64::MAX - 1;

/// Dispatches a syscall to the appropriate handler based on the syscall number.
///
/// This function is called from the naked [`syscall_entry`] with the System V
/// AMD64 calling convention.
///
/// # Arguments
///
/// * `num` - The syscall number (from user `rax`).
/// * `arg1` - First argument (from user `rdi`).
/// * `arg2` - Second argument (from user `rsi`).
/// * `arg3` - Third argument (from user `rdx`).
///
/// # Returns
///
/// The syscall return value, or [`PROCESS_EXIT_SENTINEL`] to signal process exit.
extern "C" fn syscall_dispatch(num: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
    match num {
        SYS_EXIT => {
            serial_println!("[kernel] user process exited with code: {}", arg1);
            println!("[kernel] user process exited with code: {}", arg1);
            PROCESS_EXIT_SENTINEL
        }
        SYS_WRITE => sys_write(arg1, arg2),
        SYS_OPEN => sys_open(arg1, arg2, arg3),
        SYS_CLOSE => sys_close(arg1),
        SYS_READ => sys_read(arg1, arg2, arg3),
        SYS_FWRITE => sys_fwrite(arg1, arg2, arg3),
        _ => {
            serial_println!("[kernel] unknown syscall number: {}", num);
            println!("[kernel] unknown syscall number: {}", num);
            SYSCALL_ERROR
        }
    }
}

/// Checks that the range `[ptr, ptr + len)` lies within user-accessible
/// memory and returns it as an immutable byte slice.
///
/// Returns an error without touching memory when the range is invalid.
fn user_slice(ptr: u64, len: u64) -> Result<&'static [u8], ()> {
    let end = ptr.saturating_add(len);
    if ptr < userspace::USER_CODE_START || end > userspace::USER_STACK_TOP || end < ptr {
        return Err(());
    }
    if len == 0 {
        return Ok(&[]);
    }
    // SAFETY:
    //
    // The range was validated to lie within the user address space, which the
    // kernel has mapped and can access. The caller must not hold the slice
    // across operations that could unmap user memory.
    Ok(unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) })
}

/// Checks that the range `[ptr, ptr + len)` lies within user-accessible
/// memory and returns it as a mutable byte slice.
///
/// Returns an error without touching memory when the range is invalid.
fn user_slice_mut(ptr: u64, len: u64) -> Result<&'static mut [u8], ()> {
    let end = ptr.saturating_add(len);
    if ptr < userspace::USER_CODE_START || end > userspace::USER_STACK_TOP || end < ptr {
        return Err(());
    }
    if len == 0 {
        return Ok(&mut []);
    }
    // SAFETY:
    //
    // The range was validated to lie within the user address space, which the
    // kernel has mapped and can access. The caller must not hold the slice
    // across operations that could unmap user memory.
    Ok(unsafe { core::slice::from_raw_parts_mut(ptr as *mut u8, len as usize) })
}

/// Writes a buffer from user memory to the VGA text display.
///
/// # Arguments
///
/// * `buf_ptr` - Virtual address of the buffer in user space.
/// * `len` - Number of bytes to write.
///
/// # Returns
///
/// The number of bytes successfully written, or [`SYSCALL_ERROR`] on failure.
fn sys_write(buf_ptr: u64, len: u64) -> u64 {
    let buf = match user_slice(buf_ptr, len) {
        Ok(buf) => buf,
        Err(()) => {
            println!("[kernel] sys_write: invalid buffer range");
            return SYSCALL_ERROR;
        }
    };

    match core::str::from_utf8(buf) {
        Ok(s) => {
            serial_println!("[kernel] sys_write: \"{}\"", s);
            print!("{}", s);
            len
        }
        Err(_) => {
            // Fall back to printing byte-by-byte for non-UTF-8 data.
            for &byte in buf {
                if byte.is_ascii_graphic() || byte == b' ' || byte == b'\n' {
                    print!("{}", byte as char);
                } else {
                    print!(".");
                }
            }
            len
        }
    }
}

/// Opens (or creates) a file in the in-memory filesystem.
///
/// # Arguments
///
/// * `path_ptr` - Virtual address of the path string in user memory.
/// * `path_len` - Length of the path string in bytes.
/// * `flags` - Combination of [`fs::O_READ`], [`fs::O_WRITE`], [`fs::O_CREATE`],
///   [`fs::O_TRUNC`] and [`fs::O_APPEND`].
///
/// # Returns
///
/// The new file descriptor, or [`SYSCALL_ERROR`] on failure.
fn sys_open(path_ptr: u64, path_len: u64, flags: u64) -> u64 {
    let path_bytes = match user_slice(path_ptr, path_len) {
        Ok(bytes) => bytes,
        Err(()) => {
            println!("[kernel] sys_open: invalid path range");
            return SYSCALL_ERROR;
        }
    };
    let path = match core::str::from_utf8(path_bytes) {
        Ok(path) => path,
        Err(_) => {
            println!("[kernel] sys_open: path is not valid UTF-8");
            return SYSCALL_ERROR;
        }
    };
    let flags = match u32::try_from(flags) {
        Ok(flags) => flags,
        Err(_) => {
            println!("[kernel] sys_open: flags do not fit in u32");
            return SYSCALL_ERROR;
        }
    };
    match fs::open(path, flags) {
        Ok(fd) => u64::from(fd),
        Err(err) => {
            serial_println!("[kernel] sys_open failed: {:?}", err);
            SYSCALL_ERROR
        }
    }
}

/// Closes an open file descriptor.
///
/// # Returns
///
/// `0` on success, or [`SYSCALL_ERROR`] on failure.
fn sys_close(fd: u64) -> u64 {
    let fd = match u32::try_from(fd) {
        Ok(fd) => fd,
        Err(_) => return SYSCALL_ERROR,
    };
    match fs::close(fd) {
        Ok(()) => 0,
        Err(err) => {
            serial_println!("[kernel] sys_close failed: {:?}", err);
            SYSCALL_ERROR
        }
    }
}

/// Reads bytes from an open file into a user buffer.
///
/// # Arguments
///
/// * `fd` - File descriptor returned by [`sys_open`].
/// * `buf_ptr` - Virtual address of the destination buffer in user memory.
/// * `len` - Maximum number of bytes to read.
///
/// # Returns
///
/// The number of bytes read (`0` at end of file), or [`SYSCALL_ERROR`] on failure.
fn sys_read(fd: u64, buf_ptr: u64, len: u64) -> u64 {
    let fd = match u32::try_from(fd) {
        Ok(fd) => fd,
        Err(_) => return SYSCALL_ERROR,
    };
    let buf = match user_slice_mut(buf_ptr, len) {
        Ok(buf) => buf,
        Err(()) => {
            println!("[kernel] sys_read: invalid buffer range");
            return SYSCALL_ERROR;
        }
    };
    match fs::read(fd, buf) {
        Ok(n) => n as u64,
        Err(err) => {
            serial_println!("[kernel] sys_read failed: {:?}", err);
            SYSCALL_ERROR
        }
    }
}

/// Writes bytes from a user buffer to an open file.
///
/// # Arguments
///
/// * `fd` - File descriptor returned by [`sys_open`].
/// * `buf_ptr` - Virtual address of the source buffer in user memory.
/// * `len` - Number of bytes to write.
///
/// # Returns
///
/// The number of bytes written, or [`SYSCALL_ERROR`] on failure.
fn sys_fwrite(fd: u64, buf_ptr: u64, len: u64) -> u64 {
    let fd = match u32::try_from(fd) {
        Ok(fd) => fd,
        Err(_) => return SYSCALL_ERROR,
    };
    let buf = match user_slice(buf_ptr, len) {
        Ok(buf) => buf,
        Err(()) => {
            println!("[kernel] sys_fwrite: invalid buffer range");
            return SYSCALL_ERROR;
        }
    };
    match fs::write(fd, buf) {
        Ok(n) => n as u64,
        Err(err) => {
            serial_println!("[kernel] sys_fwrite failed: {:?}", err);
            SYSCALL_ERROR
        }
    }
}

/// Registers the syscall interrupt handler in the IDT.
///
/// The entry at index `0x80` is configured with DPL Ring 3 so that user-mode
/// code can trigger it via `int 0x80`.
///
/// # Arguments
///
/// * `idt` - A mutable reference to the IDT being initialized.
pub fn register_syscall_handler(idt: &mut x86_64::structures::idt::InterruptDescriptorTable) {
    let entry = &mut idt[userspace::SYSCALL_INTERRUPT_INDEX as usize];

    // SAFETY:
    //
    // We set the handler address to our naked function which correctly manages
    // the full register save/restore and `iretq` return sequence.
    unsafe {
        let handler_addr = VirtAddr::new(syscall_entry as *const () as u64);
        entry
            .set_handler_addr(handler_addr)
            .set_privilege_level(x86_64::PrivilegeLevel::Ring3);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_unknown_syscall_returns_error() {
        let result = syscall_dispatch(999, 0, 0, 0);
        assert_eq!(
            result, SYSCALL_ERROR,
            "Unknown syscall should return SYSCALL_ERROR.",
        );
    }

    #[test_case]
    fn test_sys_exit_returns_sentinel() {
        let result = syscall_dispatch(SYS_EXIT, 42, 0, 0);
        assert_eq!(
            result, PROCESS_EXIT_SENTINEL,
            "sys_exit should return PROCESS_EXIT_SENTINEL.",
        );
    }

    #[test_case]
    fn test_sys_write_rejects_null_pointer() {
        let result = syscall_dispatch(SYS_WRITE, 0, 10, 0);
        assert_eq!(
            result, SYSCALL_ERROR,
            "sys_write with address 0 should fail validation.",
        );
    }

    #[test_case]
    fn test_sys_write_rejects_out_of_bounds_buffer() {
        // Buffer starting past the user stack top.
        let result = syscall_dispatch(SYS_WRITE, userspace::USER_STACK_TOP + 1, 10, 0);
        assert_eq!(
            result, SYSCALL_ERROR,
            "sys_write with out-of-bounds address should fail validation.",
        );
    }

    #[test_case]
    fn test_sys_write_rejects_overflow() {
        // Buffer that would overflow u64.
        let result = syscall_dispatch(SYS_WRITE, userspace::USER_CODE_START, u64::MAX, 0);
        assert_eq!(
            result, SYSCALL_ERROR,
            "sys_write with overflowing length should fail validation.",
        );
    }

    #[test_case]
    fn test_sys_open_rejects_null_path() {
        let result = syscall_dispatch(SYS_OPEN, 0, 10, u64::from(fs::O_READ));
        assert_eq!(
            result, SYSCALL_ERROR,
            "sys_open with address 0 should fail validation.",
        );
    }

    #[test_case]
    fn test_sys_open_rejects_out_of_bounds_path() {
        let result = syscall_dispatch(
            SYS_OPEN,
            userspace::USER_STACK_TOP + 1,
            10,
            u64::from(fs::O_READ),
        );
        assert_eq!(
            result, SYSCALL_ERROR,
            "sys_open with out-of-bounds path should fail validation.",
        );
    }

    #[test_case]
    fn test_sys_close_rejects_invalid_fd() {
        let result = syscall_dispatch(SYS_CLOSE, u64::from(u32::MAX), 0, 0);
        assert_eq!(
            result, SYSCALL_ERROR,
            "sys_close with an invalid fd should fail.",
        );
    }

    #[test_case]
    fn test_sys_read_rejects_null_buffer() {
        let result = syscall_dispatch(SYS_READ, 0, 0, 10);
        assert_eq!(
            result, SYSCALL_ERROR,
            "sys_read with address 0 should fail validation.",
        );
    }

    #[test_case]
    fn test_sys_fwrite_rejects_null_buffer() {
        let result = syscall_dispatch(SYS_FWRITE, 0, 0, 10);
        assert_eq!(
            result, SYSCALL_ERROR,
            "sys_fwrite with address 0 should fail validation.",
        );
    }

    #[test_case]
    fn test_syscall_numbers_are_stable() {
        assert_eq!(SYS_EXIT, 0, "sys_exit must stay syscall 0.");
        assert_eq!(SYS_WRITE, 1, "sys_write must stay syscall 1.");
        assert_eq!(SYS_OPEN, 2, "sys_open must be syscall 2.");
        assert_eq!(SYS_CLOSE, 3, "sys_close must be syscall 3.");
        assert_eq!(SYS_READ, 4, "sys_read must be syscall 4.");
        assert_eq!(SYS_FWRITE, 5, "sys_fwrite must be syscall 5.");
    }
}
