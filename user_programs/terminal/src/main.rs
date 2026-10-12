//! Terminal user-space program for `self_rust_os`.
//!
//! A simple interactive shell running in Ring 3. Uses syscalls:
//! - SYS_WRITE (1): output to console
//! - SYS_READ (2): keyboard input (stdin)
//! - SYS_TICKS (3): monotonic timer
//! - SYS_EXIT (0): terminate
//!
//! Commands: help, echo <text>, clear, time, exit

#![no_std]
#![no_main]

use core::arch::asm;
use core::panic::PanicInfo;

/// Syscall numbers.
const SYS_EXIT: u64 = 0;
const SYS_WRITE: u64 = 1;
const SYS_READ: u64 = 2;
const SYS_TICKS: u64 = 3;

/// Invokes a syscall via `int 0x80`.
///
/// # Safety
/// The caller must ensure valid syscall number and arguments.
#[inline(always)]
unsafe fn syscall(num: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
    let result: u64;
    asm!(
        "int 0x80",
        inlateout("rax") num => result,
        in("rdi") arg1,
        in("rsi") arg2,
        in("rdx") arg3,
        lateout("rcx") _,
        lateout("r11") _,
        options(nostack),
    );
    result
}

fn write(buf: &[u8]) -> u64 {
    unsafe { syscall(SYS_WRITE, buf.as_ptr() as u64, buf.len() as u64, 0) }
}

fn read(buf: &mut [u8]) -> u64 {
    unsafe { syscall(SYS_READ, 0, buf.as_mut_ptr() as u64, buf.len() as u64) }
}

fn ticks() -> u64 {
    unsafe { syscall(SYS_TICKS, 0, 0, 0) }
}

fn exit(code: u64) -> ! {
    unsafe {
        syscall(SYS_EXIT, code, 0, 0);
    }
    loop {
        unsafe { asm!("hlt", options(nomem, nostack)) };
    }
}

/// Writes a u64 as decimal.
fn write_u64(mut n: u64) {
    if n == 0 {
        write(b"0");
        return;
    }
    let mut digits = [0u8; 20];
    let mut i = 0;
    while n > 0 {
        digits[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }
    // Reverse.
    for j in (0..i).rev() {
        write(&digits[j..j + 1]);
    }
}

/// Compares two byte slices for equality.
fn eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x == y)
}

#[no_mangle]
#[link_section = ".text.start"]
pub extern "C" fn _start() -> ! {
    write(b"self_rust_os terminal v1.0\n");
    write(b"Type 'help' for commands.\n\n");

    let mut line_buf = [0u8; 256];
    let mut line_len = 0;
    let mut char_buf = [0u8; 4];

    loop {
        write(b"> ");

        // Read a line with echo.
        line_len = 0;
        loop {
            let n = read(&mut char_buf) as usize;
            if n == 0 {
                continue;
            }
            let c = char_buf[0];

            if c == b'\n' || c == b'\r' {
                write(b"\n");
                break;
            } else if c == 8 || c == 127 {
                // Backspace: erase last char.
                if line_len > 0 {
                    line_len -= 1;
                    write(b"\x08 \x08"); // Move back, erase, move back.
                }
            } else if line_len < line_buf.len() && c >= 32 && c < 127 {
                // Printable ASCII: echo and buffer.
                line_buf[line_len] = c;
                line_len += 1;
                write(&char_buf[..1]);
            }
        }

        let line = &line_buf[..line_len];

        // Parse command.
        if eq(line, b"help") {
            write(b"Commands:\n");
            write(b"  help       - show this help\n");
            write(b"  echo <txt> - print text\n");
            write(b"  clear      - clear screen\n");
            write(b"  time       - show timer ticks\n");
            write(b"  exit       - exit terminal\n");
        } else if eq(line, b"exit") {
            write(b"Goodbye!\n");
            exit(0);
        } else if eq(line, b"clear") {
            // Clear by printing newlines (console scrolls).
            for _ in 0..30 {
                write(b"\n");
            }
        } else if eq(line, b"time") {
            write(b"ticks: ");
            write_u64(ticks());
            write(b"\n");
        } else if line.len() > 5 && eq(&line[..5], b"echo ") {
            write(&line[5..]);
            write(b"\n");
        } else if line.is_empty() {
            // Empty line, just reprompt.
        } else {
            write(b"unknown command: ");
            write(line);
            write(b"\n");
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    write(b"PANIC in terminal!\n");
    exit(1);
}
