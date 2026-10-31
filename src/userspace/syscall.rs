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

use x86_64::VirtAddr;

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
/// Sends an opaque u32 message to/from the display owner.
pub const SYS_SEND_MESSAGE: u64 = 20;
/// Receives an authenticated scalar message without blocking.
pub const SYS_RECEIVE_MESSAGE: u64 = 21;
/// Register-only connected-child standard-I/O operations.
pub const SYS_STDIO: u64 = 22;

/// Sentinel: the current process has exited, schedule another (don't save context).
const PROCESS_EXIT_SENTINEL: u64 = u64::MAX;

/// Error value returned to the user program when a syscall number is unknown.
const SYSCALL_ERROR: u64 = u64::MAX - 1;

/// Dispatches a syscall to the appropriate handler based on the syscall number.
///
/// This function is called from [`crate::arch::x86_64::entry`] with the System V
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
pub(crate) extern "C" fn syscall_dispatch(num: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
    crate::scheduler::reap_retired();
    if (96..=108).contains(&num) {
        return thread_dispatch(num, arg1, arg2, arg3);
    }
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
        SYS_SEND_MESSAGE => {
            if arg1 > u32::MAX as u64 || arg2 > u32::MAX as u64 || arg3 != 0 {
                return SYSCALL_ERROR;
            }
            crate::scheduler::send_message(arg1 as u32, arg2 as u32)
        }
        SYS_RECEIVE_MESSAGE => {
            if arg1 != 0 || arg2 != 0 || arg3 != 0 {
                return SYSCALL_ERROR;
            }
            crate::scheduler::receive_message()
        }
        SYS_SURFACE_STATUS => {
            if crate::scheduler::current_pid() != Some(0) {
                return SYSCALL_ERROR;
            }
            crate::scheduler::surface_status(arg1 as u32)
        }
        64..=70 => crate::net::syscall(num, arg1, arg2, arg3),
        80..=81 => crate::system::syscall(num, arg1, arg2, arg3),
        SYS_READ_NB => sys_read_nb(arg1, arg2, arg3),
        SYS_LS => sys_ls(arg1, arg2, arg3),
        SYS_READ_FILE => sys_read_file(arg1, arg2, arg3),
        SYS_WRITE_FILE => sys_write_file(arg1, arg2, arg3),
        SYS_SPAWN => {
            if arg2 > 1 || arg3 != 0 {
                return SYSCALL_ERROR;
            }
            if arg2 == 1 && !crate::scheduler::can_connect_stdio() {
                return SYSCALL_ERROR;
            }
            sys_spawn(arg1, arg2 == 1)
        }
        SYS_STDIO => {
            if arg1 > 2 || arg2 > u32::MAX as u64 || arg3 != 0 || (arg1 != 1 && arg2 != 0) {
                return SYSCALL_ERROR;
            }
            crate::scheduler::stdio(arg1, arg2 as u32)
        }
        SYS_KILL => sys_kill(arg1),
        SYS_WAITPID => sys_waitpid(arg1),
        SYS_MOUSE => sys_mouse(),
        SYS_MKDIR => sys_mkdir(arg1),
        SYS_FB_INFO => sys_fb_info(arg1),
        14 => {
            let size = crate::drivers::video::framebuffer::fb_info().pitch
                * crate::drivers::video::framebuffer::fb_info().height;
            if crate::scheduler::current_pid() != Some(0) || (arg3 == 0 || arg3 > size as u64) {
                return SYSCALL_ERROR;
            }
            crate::scheduler::snapshot_user(arg1 as u32, arg2, arg3 as usize)
        }
        16 => {
            crate::scheduler::graphics_mode();
            0
        }
        15 => {
            if crate::scheduler::current_pid() != Some(0) {
                return SYSCALL_ERROR;
            }
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

fn thread_dispatch(number: u64, first: u64, second: u64, third: u64) -> u64 {
    use crate::{arch::x86_64::smp, scheduler as s};
    use rustos_thread_abi as abi;
    match number {
        abi::SYS_CPU_COUNT
        | abi::SYS_CURRENT_CPU
        | abi::SYS_THREAD_CURRENT
        | abi::SYS_THREAD_YIELD
        | abi::SYS_THREAD_EXIT
        | abi::SYS_THREAD_PAGES
        | abi::SYS_FRAME_COUNT
            if first != 0 || second != 0 || third != 0 =>
        {
            SYSCALL_ERROR
        }
        abi::SYS_CPU_COUNT => smp::online_count() as u64,
        abi::SYS_CURRENT_CPU => smp::current_cpu() as u64,
        abi::SYS_THREAD_CURRENT => s::current_tid().unwrap_or(SYSCALL_ERROR),
        abi::SYS_THREAD_YIELD => s::yield_current(),
        abi::SYS_THREAD_EXIT => {
            s::exit_thread();
            PROCESS_EXIT_SENTINEL
        }
        abi::SYS_THREAD_PAGES => s::thread_pages(),
        abi::SYS_FRAME_COUNT => s::frame_count(),
        abi::SYS_THREAD_JOIN | abi::SYS_THREAD_DETACH if second != 0 || third != 0 => SYSCALL_ERROR,
        abi::SYS_THREAD_JOIN => s::join_thread(first),
        abi::SYS_THREAD_DETACH => s::detach_thread(first),
        abi::SYS_THREAD_WAIT => s::wait_word(first, second, third),
        abi::SYS_THREAD_WAKE if third == 0 => s::wake_word(first, second),
        abi::SYS_MESSAGE_WAIT if second == 0 && third == 0 => s::wait_message(first),
        abi::SYS_THREAD_CREATE => {
            if second != core::mem::size_of::<abi::Create>() as u64 || third != 0 {
                return SYSCALL_ERROR;
            }
            let mut request = abi::Create {
                version: 0,
                flags: 0,
                entry: 0,
                argument: 0,
                stack_size: 0,
                affinity: 0,
            };
            // SAFETY: Create is padding-free integers, already initialized; the
            // temporary byte slice aliases only this exclusively owned record.
            let bytes = unsafe {
                core::slice::from_raw_parts_mut(
                    core::ptr::addr_of_mut!(request).cast::<u8>(),
                    core::mem::size_of::<abi::Create>(),
                )
            };
            if !userspace::usercopy::from_user(first, bytes) {
                return SYSCALL_ERROR;
            }
            s::create_thread(request)
        }
        _ => SYSCALL_ERROR,
    }
}

fn user_slice(ptr: u64, len: u64) -> Result<alloc::vec::Vec<u8>, ()> {
    if !userspace::usercopy::validate(ptr, len, false) {
        return Err(());
    }
    let mut bytes = alloc::vec::Vec::new();
    bytes.try_reserve_exact(len as usize).map_err(|_| ())?;
    bytes.resize(len as usize, 0);
    if !userspace::usercopy::from_user(ptr, &mut bytes) {
        return Err(());
    }
    Ok(bytes)
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

    crate::scheduler::surface_write(&buf);
    match core::str::from_utf8(&buf) {
        Ok(s) => {
            serial_println!("[kernel] sys_write: \"{}\"", s);
            print!("{}", s);
            len
        }
        Err(_) => {
            // Fall back to printing byte-by-byte for non-UTF-8 data.
            for &byte in &buf {
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

    if !userspace::usercopy::validate(buf_ptr, len as u64, true) {
        return SYSCALL_ERROR;
    }
    if !crate::scheduler::has_input_focus() {
        return crate::scheduler::block_on_keyboard();
    }
    let mut buf = [0u8; 4];
    // Keyboard decoder state. We create a fresh decoder per call; scancode
    // streams are self-synchronizing for simple key presses.
    let mut keyboard = INPUT_DECODER.lock();

    // Try to read without blocking. If no input is available, block this
    // process via the scheduler (instead of hlt-looping in the kernel, which
    // would prevent preemption). The keyboard IRQ will wake us.
    // Drain queued scancodes (including key releases) until we get a character.
    loop {
        let scancode = match crate::drivers::input::keyboard::try_pop_scancode() {
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
                            if read < len.min(4) {
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
            if !userspace::usercopy::to_user(buf_ptr, &buf[..read]) {
                return SYSCALL_ERROR;
            }
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
    if !crate::scheduler::has_input_focus() {
        return 0;
    }
    if fd != 0 || len == 0 {
        return 0;
    }

    if !userspace::usercopy::validate(buf_ptr, 1, true) {
        return SYSCALL_ERROR;
    }
    let mut keyboard = INPUT_DECODER.lock();
    // IRQs are disabled during the syscall, so at most the queue's 100
    // existing scancodes can be consumed. Releases/modifiers must not hide
    // a character already queued behind them from a nonblocking reader.
    for _ in 0..100 {
        let Some(scancode) = crate::drivers::input::keyboard::try_pop_scancode() else {
            return 0;
        };
        if let Ok(Some(event)) = keyboard.add_byte(scancode) {
            if let Some(key) = keyboard.process_keyevent(event) {
                use pc_keyboard::DecodedKey;
                if let DecodedKey::Unicode(c) = key {
                    let mut tmp = [0u8; 4];
                    let encoded = c.encode_utf8(&mut tmp);
                    if let Some(&b) = encoded.as_bytes().first() {
                        if !userspace::usercopy::to_user(buf_ptr, &[b]) {
                            return SYSCALL_ERROR;
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
    userspace::usercopy::validate(ptr, len, true)
}

/// Copy the path before invoking filesystem mechanisms.
fn with_user_path<R>(ptr: u64, f: impl FnOnce(&str) -> R) -> Option<R> {
    let mut bytes = [0u8; 256];
    if !userspace::usercopy::from_user(ptr, &mut bytes) {
        return None;
    }
    let len = bytes.iter().position(|&b| b == 0)?;
    if len >= 255 {
        return None;
    }
    Some(f(core::str::from_utf8(&bytes[..len]).ok()?))
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
    for (i, entry) in entries.iter().take(count).enumerate() {
        let mut record = [0u8; 12];
        record[..11].copy_from_slice(&entry.name);
        record[11] = entry.attr;
        if !userspace::usercopy::to_user(buf_ptr + i as u64 * 12, &record) {
            return SYSCALL_ERROR;
        }
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
    let info = crate::drivers::video::framebuffer::fb_info();
    let mut bytes = [0u8; 32];
    for (i, value) in [info.width, info.height, info.pitch, usize::from(info.bpp)]
        .iter()
        .enumerate()
    {
        bytes[i * 8..i * 8 + 8].copy_from_slice(&(*value as u64).to_le_bytes());
    }
    if !userspace::usercopy::to_user(buf_ptr, &bytes) {
        return SYSCALL_ERROR;
    }
    0
}

/// Reads a file by path into a user buffer.
///
/// Path is null-terminated. Returns bytes read, or SYSCALL_ERROR.
fn sys_read_file(path_ptr: u64, buf_ptr: u64, len: u64) -> u64 {
    if !validate_user_buffer(buf_ptr, len) {
        return SYSCALL_ERROR;
    }
    let mut bytes = alloc::vec::Vec::new();
    if bytes.try_reserve_exact(len as usize).is_err() {
        return SYSCALL_ERROR;
    }
    bytes.resize(len as usize, 0);
    match with_user_path(path_ptr, |path| {
        crate::fs::with_fs(|fs| fs.read_file(path, &mut bytes))
    }) {
        Some(Some(Ok(n))) if userspace::usercopy::to_user(buf_ptr, &bytes[..n]) => n as u64,
        _ => SYSCALL_ERROR,
    }
}
fn sys_write_file(path_ptr: u64, buf_ptr: u64, len: u64) -> u64 {
    let Ok(bytes) = user_slice(buf_ptr, len) else {
        return SYSCALL_ERROR;
    };
    match with_user_path(path_ptr, |path| {
        crate::fs::with_fs(|fs| {
            let _ = fs.create_file(path);
            fs.write_file(path, &bytes)
        })
    }) {
        Some(Some(Ok(n))) => n as u64,
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
fn sys_spawn(path_ptr: u64, connected: bool) -> u64 {
    let mut path_bytes = [0u8; 256];
    if !userspace::usercopy::from_user(path_ptr, &mut path_bytes) {
        return SYSCALL_ERROR;
    }
    let Some(path_len) = path_bytes.iter().position(|&b| b == 0) else {
        return SYSCALL_ERROR;
    };
    if path_len == 0 || path_len >= 255 {
        return SYSCALL_ERROR;
    }
    let Ok(path) = core::str::from_utf8(&path_bytes[..path_len]) else {
        return SYSCALL_ERROR;
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

    // Loading may release CPU entry while holding the filesystem's service
    // lock. A concurrent kill must not let this dead caller publish new work.
    if !crate::scheduler::current_is_live() {
        return PROCESS_EXIT_SENTINEL;
    }
    // Create the process (read-only size 0 = all writable for now).
    match crate::scheduler::spawn_process(&binary, 0) {
        Ok(pid) => {
            if connected && !crate::scheduler::connect_stdio(pid) {
                let _ = crate::scheduler::kill(pid);
                let _ = crate::scheduler::waitpid(pid);
                return SYSCALL_ERROR;
            }
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
    let (x, y) = crate::drivers::input::mouse::position();
    let buttons = crate::drivers::input::mouse::buttons();
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
        let handler_addr =
            VirtAddr::new(crate::arch::x86_64::entry::syscall_entry as *const () as u64);
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
        assert_eq!(
            syscall_dispatch(17, 1, 0, 0),
            SYSCALL_ERROR,
            "no current process"
        );
        assert_eq!(syscall_dispatch(18, 0, 0, 0), SYSCALL_ERROR);
        assert_eq!(
            syscall_dispatch(19, 1, 0, 0),
            SYSCALL_ERROR,
            "metadata is compositor-only"
        );
        assert_eq!(
            syscall_dispatch(20, 0, 0, 0),
            SYSCALL_ERROR,
            "messages need a calling process"
        );
        assert_eq!(
            syscall_dispatch(20, 1u64 << 32, 0, 0),
            SYSCALL_ERROR,
            "target must not truncate"
        );
        assert_eq!(
            syscall_dispatch(20, 0, 1u64 << 32, 0),
            SYSCALL_ERROR,
            "payload must not truncate"
        );
        assert_eq!(
            syscall_dispatch(20, 0, 0, 1),
            SYSCALL_ERROR,
            "reserved args must be zero"
        );
        assert_eq!(syscall_dispatch(21, 0, 0, 0), SYSCALL_ERROR);
        assert_eq!(syscall_dispatch(21, 1, 0, 0), SYSCALL_ERROR);
        assert_eq!(syscall_dispatch(21, 0, 1, 0), SYSCALL_ERROR);
        assert_eq!(syscall_dispatch(21, 0, 0, 1), SYSCALL_ERROR);
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
    fn stdio_rejects_reserved_and_truncated_arguments() {
        for (op, payload, reserved) in
            [(3, 0, 0), (1, u64::MAX, 0), (0, 1, 0), (2, 1, 0), (0, 0, 1)]
        {
            assert_eq!(
                syscall_dispatch(SYS_STDIO, op, payload, reserved),
                SYSCALL_ERROR
            );
        }
        assert_eq!(syscall_dispatch(SYS_SPAWN, 0, 2, 0), SYSCALL_ERROR);
        assert_eq!(syscall_dispatch(SYS_SPAWN, 0, 0, 1), SYSCALL_ERROR);
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
