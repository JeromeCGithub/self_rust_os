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

use spin::Mutex;
use x86_64::{structures::idt::InterruptStackFrame, VirtAddr};

use crate::{fs, print, println, serial_println, userspace};

/// Syscall number for `sys_exit`: terminates the current user process.
pub const SYS_EXIT: u64 = 0;

/// Syscall number for `sys_write`: writes a buffer to the console.
pub const SYS_WRITE: u64 = 1;

/// Syscall number for `sys_read`: reads keyboard input (stdin) into a buffer.
pub const SYS_READ: u64 = 2;

/// Syscall number for `sys_ticks`: returns the monotonic timer tick count.
pub const SYS_TICKS: u64 = 3;

/// Syscall number for non-blocking `sys_read`: returns 0 if no input, else bytes read.
pub const SYS_READ_NB: u64 = 4;

/// Syscall number for `sys_ls`: lists root directory entries.
pub const SYS_LS: u64 = 5;

/// Syscall number for `sys_read_file`: reads a file by path.
pub const SYS_READ_FILE: u64 = 6;

/// Syscall number for `sys_write_file`: writes (creates/overwrites) a file by path.
pub const SYS_WRITE_FILE: u64 = 7;

/// Syscall number for `sys_spawn`: loads and executes a program from disk.
/// Does not return on success (replaces the current process).
pub const SYS_SPAWN: u64 = 8;

/// Pending spawn request: path of the program to load after the current
/// process exits. Set by `sys_spawn`, consumed by `process::run`.
static SPAWN_REQUEST: Mutex<Option<[u8; 256]>> = Mutex::new(None);

/// Takes the pending spawn request, if any.
pub fn take_spawn_request() -> Option<[u8; 256]> {
    SPAWN_REQUEST.lock().take()
}

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
        SYS_READ => sys_read(arg1, arg2, arg3),
        SYS_TICKS => crate::interrupts::ticks(),
        SYS_READ_NB => sys_read_nb(arg1, arg2, arg3),
        SYS_LS => sys_ls(arg1, arg2),
        SYS_READ_FILE => sys_read_file(arg1, arg2, arg3),
        SYS_WRITE_FILE => sys_write_file(arg1, arg2, arg3),
        SYS_SPAWN => sys_spawn(arg1),
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

/// Reads keyboard input (stdin) into a user buffer.
///
/// Blocks until at least one byte is available or `len` bytes are read.
/// Returns the number of bytes read, or [`SYSCALL_ERROR`] on invalid args.
///
/// # Arguments
///
/// * `fd` - File descriptor (only 0 = stdin is supported).
/// * `buf_ptr` - Virtual address of the user buffer.
/// * `len` - Maximum bytes to read.
fn sys_read(fd: u64, buf_ptr: u64, len: u64) -> u64 {
    // Only stdin (fd 0) is supported.
    if fd != 0 {
        return SYSCALL_ERROR;
    }

    let len = len as usize;
    if len == 0 {
        return 0;
    }

    let buf_end = buf_ptr.saturating_add(len as u64);
    if buf_ptr < userspace::USER_CODE_START
        || buf_end > userspace::USER_STACK_TOP
        || buf_end < buf_ptr
    {
        println!("[kernel] sys_read: invalid buffer range");
        return SYSCALL_ERROR;
    }

    // SAFETY: The buffer range was validated to be within user-mapped memory.
    let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, len) };

    // Keyboard decoder state. We create a fresh decoder per call; scancode
    // streams are self-synchronizing for simple key presses.
    use pc_keyboard::{layouts, HandleControl, Keyboard, ScancodeSet1};
    let mut keyboard = Keyboard::new(
        ScancodeSet1::new(),
        layouts::Us104Key,
        HandleControl::Ignore,
    );

    let mut read = 0;
    // The syscall handler is an interrupt gate, so IF is clear here. hlt()
    // needs interrupts enabled to wake on the keyboard IRQ, so enable them
    // for the wait loop. This doesn't leak: iretq restores the userspace
    // RFLAGS (IF=1) when the syscall returns.
    x86_64::instructions::interrupts::enable();
    while read < len {
        // Try to get a scancode; halt the CPU while waiting to avoid busy-looping.
        let scancode = loop {
            if let Some(sc) = crate::task::keyboard::try_pop_scancode() {
                break sc;
            }
            x86_64::instructions::hlt();
        };

        // Decode the scancode; ignore non-character keys.
        if let Ok(Some(event)) = keyboard.add_byte(scancode) {
            if let Some(key) = keyboard.process_keyevent(event) {
                use pc_keyboard::DecodedKey;
                match key {
                    DecodedKey::Unicode(c) => {
                        // Encode as UTF-8 (ASCII fast path).
                        let mut tmp = [0u8; 4];
                        let encoded = c.encode_utf8(&mut tmp);
                        for &b in encoded.as_bytes() {
                            if read < len {
                                buf[read] = b;
                                read += 1;
                            }
                        }
                        // Return after the first character for interactive input.
                        // The terminal handles line buffering and echo.
                        break;
                    }
                    DecodedKey::RawKey(_) => {}
                }
            }
        }
    }

    read as u64
}

/// Non-blocking read: tries to read one byte without blocking.
///
/// Returns 1 and fills the buffer if a key is available, 0 if not.
/// Used by games for polling input without blocking the game loop.
fn sys_read_nb(fd: u64, buf_ptr: u64, len: u64) -> u64 {
    if fd != 0 || len == 0 {
        return 0;
    }

    let buf_end = buf_ptr.saturating_add(1);
    if buf_ptr < userspace::USER_CODE_START
        || buf_end > userspace::USER_STACK_TOP
        || buf_end < buf_ptr
    {
        return 0;
    }

    // Try to get a scancode without blocking.
    let scancode = match crate::task::keyboard::try_pop_scancode() {
        Some(sc) => sc,
        None => return 0,
    };

    use pc_keyboard::{layouts, HandleControl, Keyboard, ScancodeSet1};
    let mut keyboard = Keyboard::new(
        ScancodeSet1::new(),
        layouts::Us104Key,
        HandleControl::Ignore,
    );

    if let Ok(Some(event)) = keyboard.add_byte(scancode) {
        if let Some(key) = keyboard.process_keyevent(event) {
            use pc_keyboard::DecodedKey;
            if let DecodedKey::Unicode(c) = key {
                let mut tmp = [0u8; 4];
                let encoded = c.encode_utf8(&mut tmp);
                if let Some(&b) = encoded.as_bytes().first() {
                    // SAFETY: Buffer was validated.
                    unsafe {
                        core::ptr::write(buf_ptr as *mut u8, b);
                    }
                    return 1;
                }
            }
        }
    }
    0
}

/// Validates that a user buffer range is within the user address space.
fn validate_user_buffer(ptr: u64, len: u64) -> bool {
    let end = ptr.saturating_add(len);
    ptr >= userspace::USER_CODE_START && end <= userspace::USER_STACK_TOP && end >= ptr
}

/// Lists root directory entries into a user buffer.
///
/// Each entry is 12 bytes: 11-byte 8.3 name + 1 byte attribute.
/// Returns the number of entries, or SYSCALL_ERROR.
fn sys_ls(buf_ptr: u64, max_entries: u64) -> u64 {
    let max_entries = max_entries as usize;
    let buf_len = (max_entries as u64).saturating_mul(12);
    if !validate_user_buffer(buf_ptr, buf_len) {
        return SYSCALL_ERROR;
    }

    let result = crate::fs::with_fs(|fs| fs.list_dir());
    let entries = match result {
        Some(Ok(entries)) => entries,
        _ => return SYSCALL_ERROR,
    };

    let count = entries.len().min(max_entries);
    // SAFETY: Buffer was validated.
    let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, buf_len as usize) };

    for (i, entry) in entries.iter().take(count).enumerate() {
        buf[i * 12..i * 12 + 11].copy_from_slice(entry);
        buf[i * 12 + 11] = 0; // Attribute placeholder.
    }

    count as u64
}

/// Reads a file by path into a user buffer.
///
/// Path is null-terminated. Returns bytes read, or SYSCALL_ERROR.
fn sys_read_file(path_ptr: u64, buf_ptr: u64, buf_len: u64) -> u64 {
    if !validate_user_buffer(path_ptr, 256) || !validate_user_buffer(buf_ptr, buf_len) {
        return SYSCALL_ERROR;
    }

    // SAFETY: Buffers validated.
    let path_buf = unsafe { core::slice::from_raw_parts(path_ptr as *const u8, 256) };
    let path_len = path_buf.iter().position(|&b| b == 0).unwrap_or(256);
    let path = match core::str::from_utf8(&path_buf[..path_len]) {
        Ok(s) => s,
        Err(_) => return SYSCALL_ERROR,
    };

    let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, buf_len as usize) };

    let result = crate::fs::with_fs(|fs| fs.read_file(path, buf));
    match result {
        Some(Ok(n)) => n as u64,
        _ => SYSCALL_ERROR,
    }
}

/// Writes (creates or overwrites) a file by path from a user buffer.
///
/// Returns bytes written, or SYSCALL_ERROR.
fn sys_write_file(path_ptr: u64, buf_ptr: u64, buf_len: u64) -> u64 {
    if !validate_user_buffer(path_ptr, 256) || !validate_user_buffer(buf_ptr, buf_len) {
        return SYSCALL_ERROR;
    }

    // SAFETY: Buffers validated.
    let path_buf = unsafe { core::slice::from_raw_parts(path_ptr as *const u8, 256) };
    let path_len = path_buf.iter().position(|&b| b == 0).unwrap_or(256);
    let path = match core::str::from_utf8(&path_buf[..path_len]) {
        Ok(s) => s,
        Err(_) => return SYSCALL_ERROR,
    };

    let buf = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, buf_len as usize) };

    let result = crate::fs::with_fs(|fs| {
        // Create if not exists, then write.
        let _ = fs.create_file(path);
        fs.write_file(path, buf)
    });
    match result {
        Some(Ok(n)) => n as u64,
        _ => SYSCALL_ERROR,
    }
}

/// Loads and executes a program from disk, replacing the current process.
///
/// Takes a null-terminated path (max 255 chars) in `path_ptr`. On success,
/// records the spawn request and returns `PROCESS_EXIT_SENTINEL`, causing
/// the current process to exit. `process::run` will then load and execute
/// the requested program. Does not return to the caller on success.
///
/// Returns `SYSCALL_ERROR` if the path is invalid.
fn sys_spawn(path_ptr: u64) -> u64 {
    if !validate_user_buffer(path_ptr, 256) {
        return SYSCALL_ERROR;
    }

    // SAFETY: Buffer validated.
    let path_buf = unsafe { core::slice::from_raw_parts(path_ptr as *const u8, 256) };
    let path_len = path_buf.iter().position(|&b| b == 0).unwrap_or(256);
    if path_len == 0 || path_len >= 255 {
        return SYSCALL_ERROR;
    }

    let mut request = [0u8; 256];
    request[..path_len].copy_from_slice(&path_buf[..path_len]);

    *SPAWN_REQUEST.lock() = Some(request);

    // Exit the current process; process::run will load the new program.
    PROCESS_EXIT_SENTINEL
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
    fn test_sys_ticks_returns_value() {
        let t1 = syscall_dispatch(SYS_TICKS, 0, 0, 0);
        let t2 = syscall_dispatch(SYS_TICKS, 0, 0, 0);
        // Ticks are monotonic (non-decreasing).
        assert!(t2 >= t1, "sys_ticks should be monotonic: {} >= {}", t2, t1);
    }

    #[test_case]
    fn test_sys_read_rejects_invalid_fd() {
        // Only fd 0 (stdin) is supported.
        let result = syscall_dispatch(SYS_READ, 1, userspace::USER_CODE_START, 10);
        assert_eq!(result, SYSCALL_ERROR, "sys_read with fd != 0 should fail.",);
    }

    #[test_case]
    fn test_sys_read_rejects_invalid_buffer() {
        let result = syscall_dispatch(SYS_READ, 0, userspace::USER_STACK_TOP + 1, 10);
        assert_eq!(
            result, SYSCALL_ERROR,
            "sys_read with out-of-bounds buffer should fail validation.",
        );
    }

    #[test_case]
    fn test_sys_read_zero_len_returns_zero() {
        let result = syscall_dispatch(SYS_READ, 0, userspace::USER_CODE_START, 0);
        assert_eq!(result, 0, "sys_read with len=0 should return 0.");
    }

    #[test_case]
    fn test_sys_ls_rejects_invalid_buffer() {
        let result = syscall_dispatch(SYS_LS, userspace::USER_STACK_TOP + 1, 10, 0);
        assert_eq!(
            result, SYSCALL_ERROR,
            "sys_ls with out-of-bounds buffer should fail validation.",
        );
    }

    #[test_case]
    fn test_sys_read_file_rejects_invalid_buffer() {
        let result = syscall_dispatch(
            SYS_READ_FILE,
            userspace::USER_CODE_START,
            userspace::USER_STACK_TOP + 1,
            10,
        );
        assert_eq!(
            result, SYSCALL_ERROR,
            "sys_read_file with out-of-bounds buffer should fail.",
        );
    }

    #[test_case]
    fn test_sys_write_file_rejects_invalid_buffer() {
        let result = syscall_dispatch(
            SYS_WRITE_FILE,
            userspace::USER_CODE_START,
            userspace::USER_STACK_TOP + 1,
            10,
        );
        assert_eq!(
            result, SYSCALL_ERROR,
            "sys_write_file with out-of-bounds buffer should fail.",
        );
    }
}
