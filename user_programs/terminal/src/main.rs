//! Terminal user-space program for `self_rust_os`, built on `userlib`.
//!
//! A simple interactive shell running in Ring 3.
//!
//! Commands: help, echo <text>, clear, time, ls, cat <file>, exit

#![no_std]
#![no_main]

use userlib::{println, sys};

fn ls() {
    let mut buf = [0u8; 12 * 32]; // Max 32 entries.
    match sys::ls(&mut buf, 32) {
        None => println!("ls failed"),
        Some(0) => println!("(empty)"),
        Some(count) => {
            for i in 0..count {
                let entry = &buf[i * 12..i * 12 + 11];
                // 8.3 name: trim spaces.
                let mut name_end = 11;
                while name_end > 0 && entry[name_end - 1] == b' ' {
                    name_end -= 1;
                }
                sys::console_write(&entry[..name_end]);
                sys::console_write(b"\n");
            }
        }
    }
}

fn cat(path: &[u8]) {
    let path_str = match core::str::from_utf8(path) {
        Ok(s) => s,
        Err(_) => {
            println!("invalid path");
            return;
        }
    };
    let mut buf = [0u8; 1024];
    match sys::read_file(path_str, &mut buf) {
        None => println!("cat failed"),
        Some(n) => {
            sys::console_write(&buf[..n]);
            sys::console_write(b"\n");
        }
    }
}

/// Compares two byte slices for equality.
fn eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x == y)
}

/// Entry point called by `userlib`'s runtime after `_start`.
#[no_mangle]
pub extern "C" fn user_main() {
    println!("self_rust_os terminal v1.0");
    println!("Type 'help' for commands.\n");

    let mut line_buf = [0u8; 256];
    let mut line_len = 0;
    let mut char_buf = [0u8; 4];

    loop {
        sys::console_write(b"> ");

        // Read a line with echo.
        line_len = 0;
        loop {
            let n = sys::read(sys::STDIN_FD, &mut char_buf);
            if n == 0 {
                continue;
            }
            let c = char_buf[0];

            if c == b'\n' || c == b'\r' {
                sys::console_write(b"\n");
                break;
            } else if c == 8 || c == 127 {
                // Backspace: erase last char.
                if line_len > 0 {
                    line_len -= 1;
                    sys::console_write(b"\x08 \x08"); // Move back, erase, move back.
                }
            } else if line_len < line_buf.len() && c >= 32 && c < 127 {
                // Printable ASCII: echo and buffer.
                line_buf[line_len] = c;
                line_len += 1;
                sys::console_write(&char_buf[..1]);
            }
        }

        let line = &line_buf[..line_len];

        // Parse command.
        if eq(line, b"help") {
            println!("Commands:");
            println!("  help       - show this help");
            println!("  echo <txt> - print text");
            println!("  clear      - clear screen");
            println!("  time       - show timer ticks");
            println!("  ls         - list files");
            println!("  cat <file> - show file contents");
            println!("  exit       - exit terminal");
        } else if eq(line, b"exit") {
            println!("Goodbye!");
            sys::exit(0);
        } else if eq(line, b"clear") {
            // Clear by printing newlines (console scrolls).
            for _ in 0..30 {
                println!();
            }
        } else if eq(line, b"time") {
            println!("ticks: {}", sys::ticks());
        } else if eq(line, b"ls") {
            ls();
        } else if line.len() > 4 && eq(&line[..4], b"cat ") {
            cat(&line[4..]);
        } else if line.len() > 5 && eq(&line[..5], b"echo ") {
            sys::console_write(&line[5..]);
            sys::console_write(b"\n");
        } else if line.is_empty() {
            // Empty line, just reprompt.
        } else {
            sys::console_write(b"unknown command: ");
            sys::console_write(line);
            sys::console_write(b"\n");
        }
    }
}
