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

use crate::{print, println, serial_println, userspace};

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

/// Syscall number for `sys_ls`: lists directory entries.
/// Takes a null-terminated path ("" or "/" = root).
pub const SYS_LS: u64 = 5;

/// Syscall number for `sys_read_file`: reads a file by path.
pub const SYS_READ_FILE: u64 = 6;

/// Syscall number for `sys_write_file`: writes (creates/overwrites) a file by path.
pub const SYS_WRITE_FILE: u64 = 7;

/// Syscall number for `sys_spawn`: creates a new process from a program on disk.
/// Returns the new PID.
pub const SYS_SPAWN: u64 = 8;

/// Syscall number for `sys_kill`: terminates a process by PID.
pub const SYS_KILL: u64 = 9;

/// Syscall number for `sys_waitpid`: waits for a process to exit, reaps it.
pub const SYS_WAITPID: u64 = 10;

/// Syscall number for `sys_mouse`: returns packed mouse state.
/// Bits: `[63:32]` = buttons, `[31:16]` = y, `[15:0]` = x.
pub const SYS_MOUSE: u64 = 11;

/// Syscall number for `sys_mkdir`: creates a directory (with parents).
pub const SYS_MKDIR: u64 = 12;

/// Syscall number for `sys_fb_info`: returns framebuffer geometry.
/// Writes (width, height, pitch, bpp) as four u64 values to the user buffer.
pub const SYS_FB_INFO: u64 = 13;

/// Blocks the caller for a duration in PIT ticks.
pub const SYS_SLEEP_TICKS: u64 = 17;
/// Publishes a completed update to the caller's private drawing surface.
pub const SYS_PRESENT_SURFACE: u64 = 18;
/// Queries surface revision and exit status (display owner only).
pub const SYS_SURFACE_STATUS: u64 = 19;

/// Sentinel: the current process has exited, schedule another (don't save context).
const PROCESS_EXIT_SENTINEL: u64 = u64::MAX;

/// Sentinel: the current process is blocking, save context and schedule another.
const PROCESS_BLOCK_SENTINEL: u64 = u64::MAX - 2;

/// Naked entry point for the `int 0x80` syscall interrupt.
///
/// Saves all general-purpose registers (in the same order as the timer
/// handler, forming a [`crate::scheduler::SavedContext`]), calls the Rust
/// dispatcher, then either returns to user mode via `iretq` or performs a
/// context switch if the syscall blocked or exited the process.
///
/// # Register layout on the stack after all pushes
///
/// ```text
/// rsp + 0x00 : r15
/// rsp + 0x08 : r14
/// ...
/// rsp + 0x70 : rax  (syscall number)
/// rsp + 0x78 : RIP  (pushed by CPU)
/// rsp + 0x80 : CS
/// rsp + 0x88 : RFLAGS
/// rsp + 0x90 : RSP
/// rsp + 0x98 : SS
/// ```
#[naked]
pub(crate) extern "x86-interrupt" fn syscall_entry(_frame: InterruptStackFrame) {
    // SAFETY: Naked function with manual stack management. The CPU has pushed
    // SS, RSP, RFLAGS, CS, RIP. We push all GPRs in the canonical order.
    unsafe {
        naked_asm!(
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

            // Args for syscall_dispatch: rdi=num, rsi=arg1, rdx=arg2, rcx=arg3.
            "mov rdi, [rsp + 0x70]",
            "mov rsi, [rsp + 0x48]",
            "mov rdx, [rsp + 0x50]",
            "mov rcx, [rsp + 0x58]",

            "call {dispatch}",

            // Check for exit sentinel: current process is dead, switch away.
            "mov rcx, {exit_sentinel}",
            "cmp rax, rcx",
            "je 2f",

            // Check for block sentinel: save context and switch away.
            "mov rcx, {block_sentinel}",
            "cmp rax, rcx",
            "je 3f",

            // Normal return: store result in saved rax, restore regs, iretq.
            "mov [rsp + 0x70], rax",
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

            // 2: Process exited. rdi=0 (don't save context).
            "2:",
            "xor edi, edi",
            "call {schedule}",
            // rax = new kernel RSP. Switch stacks and restore.
            "mov rsp, rax",
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

            // 3: Process blocked. rdi=rsp (save context).
            "3:",
            "mov rdi, rsp",
            "call {schedule}",
            "mov rsp, rax",
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

            dispatch = sym syscall_dispatch,
            schedule = sym crate::scheduler::schedule,
            exit_sentinel = const PROCESS_EXIT_SENTINEL,
            block_sentinel = const PROCESS_BLOCK_SENTINEL,
        );
    }
}
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
            crate::scheduler::exit_current(arg1);
            PROCESS_EXIT_SENTINEL
        }
        SYS_WRITE => sys_write(arg1, arg2),
        SYS_READ => sys_read(arg1, arg2, arg3),
        SYS_TICKS => crate::interrupts::ticks(),
        SYS_SLEEP_TICKS => crate::scheduler::sleep_ticks(arg1),
        SYS_PRESENT_SURFACE => crate::scheduler::present_surface(),
        SYS_SURFACE_STATUS => {
            if crate::scheduler::current_pid() != Some(0) { return SYSCALL_ERROR; }
            crate::scheduler::surface_status(arg1 as u32)
        }
        SYS_READ_NB => sys_read_nb(arg1, arg2, arg3),
        SYS_LS => sys_ls(arg1, arg2, arg3),
        SYS_READ_FILE => sys_read_file(arg1, arg2, arg3),
        SYS_WRITE_FILE => sys_write_file(arg1, arg2, arg3),
        SYS_SPAWN => sys_spawn(arg1),
        SYS_KILL => sys_kill(arg1),
        SYS_WAITPID => sys_waitpid(arg1),
        SYS_MOUSE => sys_mouse(),
        SYS_MKDIR => sys_mkdir(arg1),
        SYS_FB_INFO => sys_fb_info(arg1),
        14 => {
            let size = crate::gfx::fb_info().pitch * crate::gfx::fb_info().height;
            if crate::scheduler::current_pid() != Some(0) || (arg3 == 0 || arg3 > size as u64) || !validate_user_buffer(arg2, arg3) { return SYSCALL_ERROR; }
            // SAFETY: Caller and buffer were validated above.
            let dest = unsafe { core::slice::from_raw_parts_mut(arg2 as *mut u8, arg3 as usize) };
            crate::scheduler::snapshot(arg1 as u32, dest)
        }
        16 => { crate::scheduler::graphics_mode(); 0 }
        15 => {
            if crate::scheduler::current_pid() != Some(0) { return SYSCALL_ERROR; }
            crate::scheduler::INPUT_FOCUS.store(arg1, core::sync::atomic::Ordering::Relaxed);
            0
        }
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

    crate::scheduler::surface_write(buf);
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

    if !crate::scheduler::has_input_focus() { return crate::scheduler::block_on_keyboard(); }

    // SAFETY: The buffer range was validated to be within user-mapped memory.
    let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, len) };

    // Keyboard decoder state. We create a fresh decoder per call; scancode
    // streams are self-synchronizing for simple key presses.
    let mut keyboard = INPUT_DECODER.lock();

    // Try to read without blocking. If no input is available, block this
    // process via the scheduler (instead of hlt-looping in the kernel, which
    // would prevent preemption). The keyboard IRQ will wake us.
    // Drain queued scancodes (including key releases) until we get a character.
    loop {
        let scancode = match crate::task::keyboard::try_pop_scancode() {
            Some(sc) => sc,
            None => return crate::scheduler::block_on_keyboard(),
        };

        // Skip key release scancodes (high bit set); they never produce characters.
        // Also skip 0x61 (spurious code from QEMU sendkey, not a valid scancode).
        if scancode == 0x61 {
            continue;
        }

        let mut read = 0;
        // Decode the scancode; ignore non-character keys (including releases).
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
                    }
                    DecodedKey::RawKey(_) => {}
                }
            }
        }

        if read > 0 {
            return read as u64;
        }
        // No character from this scancode (e.g., key release); try next.
    }
}

/// Non-blocking read: tries to read one byte without blocking.
///
/// Returns 1 and fills the buffer if a key is available, 0 if not.
/// Used by games for polling input without blocking the game loop.
fn sys_read_nb(fd: u64, buf_ptr: u64, len: u64) -> u64 {
    if !crate::scheduler::has_input_focus() { return 0; }
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

    let mut keyboard = INPUT_DECODER.lock();
    // IRQs are disabled during the syscall, so at most the queue's 100
    // existing scancodes can be consumed. Releases/modifiers must not hide
    // a character already queued behind them from a nonblocking reader.
    for _ in 0..100 {
        let Some(scancode) = crate::task::keyboard::try_pop_scancode() else { return 0; };
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
    }

    0
}

/// Validates that a user buffer range is within the user address space.
fn validate_user_buffer(ptr: u64, len: u64) -> bool {
    let end = ptr.saturating_add(len);
    ptr >= userspace::USER_CODE_START && end <= userspace::USER_STACK_TOP && end >= ptr
}

/// Runs `f` with the null-terminated path string at `path_ptr`.
///
/// Returns `None` if the pointer/length is invalid or the bytes are not UTF-8.
fn with_user_path<R>(path_ptr: u64, f: impl FnOnce(&str) -> R) -> Option<R> {
    if !validate_user_buffer(path_ptr, 256) {
        return None;
    }
    // SAFETY: Buffer was validated to lie in the user address space, and the
    // slice is only used synchronously inside `f` during this syscall.
    let path_buf = unsafe { core::slice::from_raw_parts(path_ptr as *const u8, 256) };
    let path_len = path_buf.iter().position(|&b| b == 0).unwrap_or(256);
    let path = core::str::from_utf8(&path_buf[..path_len]).ok()?;
    Some(f(path))
}

/// Lists directory entries into a user buffer.
///
/// `path_ptr` is a null-terminated path ("" or "/" = root).
/// Each entry is 12 bytes: 11-byte 8.3 name + 1 byte attribute
/// (`0x10` = directory, `0x20` = file).
/// Returns the number of entries, or SYSCALL_ERROR.
fn sys_ls(path_ptr: u64, buf_ptr: u64, max_entries: u64) -> u64 {
    let max_entries = max_entries as usize;
    let buf_len = (max_entries as u64).saturating_mul(12);
    if !validate_user_buffer(buf_ptr, buf_len) {
        return SYSCALL_ERROR;
    }

    let result = with_user_path(path_ptr, |path| {
        crate::fs::with_fs(|fs| fs.list_dir(Some(path)))
    });
    let entries = match result {
        Some(Some(Ok(entries))) => entries,
        _ => return SYSCALL_ERROR,
    };

    let count = entries.len().min(max_entries);
    // SAFETY: Buffer was validated.
    let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, buf_len as usize) };

    for (i, entry) in entries.iter().take(count).enumerate() {
        buf[i * 12..i * 12 + 11].copy_from_slice(&entry.name);
        buf[i * 12 + 11] = entry.attr;
    }

    count as u64
}

/// Creates a directory (with missing parents, like `mkdir -p`).
///
/// Takes a null-terminated path in `path_ptr`. Returns 0 on success,
/// or SYSCALL_ERROR.
fn sys_mkdir(path_ptr: u64) -> u64 {
    let result = with_user_path(path_ptr, |path| {
        crate::fs::with_fs(|fs| fs.create_dir(path))
    });
    match result {
        Some(Some(Ok(()))) => 0,
        _ => {
            if let Some(Some(Err(e))) = result {
                serial_println!("[kernel] sys_mkdir failed: {:?}", e);
            }
            SYSCALL_ERROR
        }
    }
}

/// Returns framebuffer geometry to user space.
///
/// Writes four u64 values to the user buffer at `buf_ptr`:
/// (width, height, pitch, bpp). Returns 0 on success, SYSCALL_ERROR on failure.
fn sys_fb_info(buf_ptr: u64) -> u64 {
    if !validate_user_buffer(buf_ptr, 32) {
        return SYSCALL_ERROR;
    }
    let info = crate::gfx::fb_info();
    // SAFETY: Buffer validated above; 32 bytes for 4 u64 values.
    unsafe {
        let buf = buf_ptr as *mut u64;
        buf.write(info.width as u64);
        buf.add(1).write(info.height as u64);
        buf.add(2).write(info.pitch as u64);
        buf.add(3).write(info.bpp as u64);
    }
    0
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

/// Creates a new process running the program at the given path.
///
/// Takes a null-terminated path (max 255 chars) in `path_ptr`. Loads the
/// binary from disk, creates a new address space and PCB, and enqueues the
/// process. Returns the new PID. The caller keeps running; the timer will
/// preempt to the new process.
///
/// Returns `SYSCALL_ERROR` if the path is invalid or the program can't be loaded.
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
    let path = match core::str::from_utf8(&path_buf[..path_len]) {
        Ok(s) => s,
        Err(_) => return SYSCALL_ERROR,
    };

    // Load the binary from disk.
    // The kernel takes the path as-is. It does NOT do PATH resolution —
    // that is the shell's job (policy, not mechanism). Callers must pass
    // a full path like "/bin/SNAKE.BIN".
    let binary = match crate::scheduler::load_program_from_disk(path) {
        Ok(b) => b,
        Err(e) => {
            serial_println!("[kernel] sys_spawn: failed to load '{}': {}", path, e);
            return SYSCALL_ERROR;
        }
    };

    // Create the process (read-only size 0 = all writable for now).
    match crate::scheduler::spawn_process(&binary, 0) {
        Ok(pid) => {
            serial_println!("[kernel] sys_spawn: '{}' -> pid {}", path, pid);
            pid as u64
        }
        Err(e) => {
            serial_println!("[kernel] sys_spawn: failed to create process: {}", e);
            SYSCALL_ERROR
        }
    }
}

/// Terminates the process with the given PID.
///
/// Returns 0 on success, `SYSCALL_ERROR` if the PID is invalid.
/// If killing the current process, returns `PROCESS_EXIT_SENTINEL` so the
/// syscall handler schedules away (never returns to user).
fn sys_kill(pid: u64) -> u64 {
    match crate::scheduler::kill(pid as u32) {
        Some(crate::scheduler::PROCESS_BLOCK_SENTINEL) => PROCESS_EXIT_SENTINEL,
        Some(_) => 0,
        None => SYSCALL_ERROR,
    }
}

/// Waits for the process with the given PID to exit.
///
/// If the target is already a zombie, reaps it and returns its exit code.
/// Otherwise blocks the current process until the target exits.
/// Returns `SYSCALL_ERROR` for an invalid PID.
fn sys_waitpid(pid: u64) -> u64 {
    crate::scheduler::waitpid(pid as u32)
}

/// Returns the packed mouse state.
///
/// Bits: `[63:32]` = buttons (bit 0 = left, 1 = right, 2 = middle),
/// `[31:16]` = y (0-199), `[15:0]` = x (0-319).
fn sys_mouse() -> u64 {
    let (x, y) = crate::mouse::position();
    let buttons = crate::mouse::buttons();
    // Clamp to u16 ranges (driver already clamps to screen, but be safe).
    let x = x.clamp(0, 0xFFFF) as u64;
    let y = y.clamp(0, 0xFFFF) as u64;
    ((buttons as u64) << 32) | (y << 16) | x
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
    fn test_timed_wait_and_surface_metadata_reject_invalid_calls() {
        assert_eq!(syscall_dispatch(17, 0, 0, 0), 0);
        assert_eq!(syscall_dispatch(17, u64::MAX, 0, 0), SYSCALL_ERROR);
        assert_eq!(syscall_dispatch(17, 1, 0, 0), SYSCALL_ERROR, "no current process");
        assert_eq!(syscall_dispatch(18, 0, 0, 0), SYSCALL_ERROR);
        assert_eq!(syscall_dispatch(19, 1, 0, 0), SYSCALL_ERROR, "metadata is compositor-only");
    }

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
    fn test_sys_mkdir_rejects_invalid_path() {
        let result = syscall_dispatch(SYS_MKDIR, userspace::USER_STACK_TOP + 1, 0, 0);
        assert_eq!(
            result, SYSCALL_ERROR,
            "sys_mkdir with out-of-bounds path should fail validation.",
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

// Preserve modifier and extended-key state across read syscalls, including releases.
lazy_static::lazy_static! {
    static ref INPUT_DECODER: spin::Mutex<pc_keyboard::Keyboard<pc_keyboard::layouts::Us104Key, pc_keyboard::ScancodeSet1>> =
        spin::Mutex::new(pc_keyboard::Keyboard::new(pc_keyboard::ScancodeSet1::new(), pc_keyboard::layouts::Us104Key, pc_keyboard::HandleControl::Ignore));
}
