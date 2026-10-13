//! Terminal user-space program for `self_rust_os`, built on `userlib`.
//!
//! A simple interactive shell running in Ring 3.
//!
//! Commands: help, echo <text>, clear, time, ls [path], mkdir <path>, cat <file>, exit

#![no_std]
#![no_main]

use userlib::{println, sys};

fn ls(path: &[u8]) {
    let path_str = match core::str::from_utf8(path) {
        Ok(s) => s,
        Err(_) => {
            println!("invalid path");
            return;
        }
    };
    let mut buf = [0u8; 12 * 32]; // Max 32 entries.
    match sys::ls(path_str, &mut buf, 32) {
        None => println!("ls failed"),
        Some(0) => println!("(empty)"),
        Some(count) => {
            for i in 0..count {
                let entry = &buf[i * 12..i * 12 + 11];
                let attr = buf[i * 12 + 11];
                // 8.3 name: trim spaces.
                let mut name_end = 11;
                while name_end > 0 && entry[name_end - 1] == b' ' {
                    name_end -= 1;
                }
                sys::console_write(&entry[..name_end]);
                if attr & 0x10 != 0 {
                    sys::console_write(b"/");
                }
                sys::console_write(b"\n");
            }
        }
    }
}

fn mkdir(path: &[u8]) {
    let path_str = match core::str::from_utf8(path) {
        Ok(s) => s,
        Err(_) => {
            println!("invalid path");
            return;
        }
    };
    match sys::mkdir(path_str) {
        Ok(()) => println!("created {}", path_str),
        Err(()) => println!("mkdir failed"),
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

/// Writes `text` to the file at `path` (creates or overwrites).
fn write(path: &[u8], text: &[u8]) {
    let path_str = match core::str::from_utf8(path) {
        Ok(s) => s,
        Err(_) => {
            println!("invalid path");
            return;
        }
    };
    match sys::write_file(path_str, text) {
        None => println!("write failed"),
        Some(n) => println!("wrote {} bytes", n),
    }
}

/// Compares two byte slices for equality.
fn eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x == y)
}

/// Entry point called by `userlib`'s runtime after `_start`.
#[no_mangle]
pub extern "C" fn user_main() {
    println!("self_rust_os forge v1.0");
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
            println!("  ls [path]  - list directory (default: root)");
            println!("  mkdir <p>  - create directory (with parents)");
            println!("  cat <file> - show file contents");
            println!("  write <f> <txt> - write text to file");
            println!("  ./<prog>   - run program from disk");
            println!("  kill <pid> - terminate a process");
            println!("  exit       - exit forge");
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
            ls(b"");
        } else if line.len() > 3 && eq(&line[..3], b"ls ") {
            ls(&line[3..]);
        } else if line.len() > 6 && eq(&line[..6], b"mkdir ") {
            mkdir(&line[6..]);
        } else if line.len() > 4 && eq(&line[..4], b"cat ") {
            cat(&line[4..]);
        } else if line.len() > 6 && eq(&line[..6], b"write ") {
            // write <path> <text>: split on the first space after "write ".
            let rest = &line[6..];
            if let Some(sp) = rest.iter().position(|&c| c == b' ') {
                write(&rest[..sp], &rest[sp + 1..]);
            } else {
                println!("usage: write <file> <text>");
            }
        } else if line.len() > 5 && eq(&line[..5], b"echo ") {
            sys::console_write(&line[5..]);
            sys::console_write(b"\n");
        } else if line.len() > 2 && eq(&line[..2], b"./") {
            // Launch a program from disk: ./name -> /NAME.BIN
            run_program(&line[2..]);
        } else if line.len() > 5 && eq(&line[..5], b"kill ") {
            kill_program(&line[5..]);
        } else if line.is_empty() {
            // Empty line, just reprompt.
        } else {
            sys::console_write(b"unknown command: ");
            sys::console_write(line);
            sys::console_write(b"\n");
        }
    }
}

/// Launches a user program from disk via the spawn syscall.
///
/// Converts `name` to `/NAME.BIN` (FAT32 8.3 uppercase) and spawns it.
/// On success, this never returns (the process is replaced).
fn run_program(name: &[u8]) {
    if name.is_empty() || name.len() > 8 {
        println!("usage: ./<name> (max 8 chars)");
        return;
    }
    // Build "NAME.BIN" path (FAT32 8.3 uppercase, no leading slash).
    let mut path = [0u8; 11];
    for (i, &c) in name.iter().enumerate() {
        // Uppercase ASCII.
        path[i] = if c >= b'a' && c <= b'z' { c - 32 } else { c };
    }
    let name_len = name.len();
    path[name_len] = b'.';
    path[name_len + 1] = b'B';
    path[name_len + 2] = b'I';
    path[name_len + 3] = b'N';
    
    let path_str = match core::str::from_utf8(&path[..4 + name_len]) {
        Ok(s) => s,
        Err(_) => {
            println!("invalid program name");
            return;
        }
    };
    
    println!("Spawning {}...", path_str);
    match sys::spawn(path_str) {
        Ok(pid) => {
            println!("started pid {}", pid);
        }
        Err(()) => {
            println!("failed to spawn {}", path_str);
        }
    }
}

/// Kills a process by PID.
fn kill_program(pid_str: &[u8]) {
    let pid_str = match core::str::from_utf8(pid_str) {
        Ok(s) => s.trim(),
        Err(_) => {
            println!("invalid pid");
            return;
        }
    };
    let mut pid: u32 = 0;
    for c in pid_str.bytes() {
        if !c.is_ascii_digit() {
            println!("invalid pid");
            return;
        }
        pid = pid * 10 + (c - b'0') as u32;
    }
    match sys::kill(pid) {
        Ok(()) => println!("killed {}", pid),
        Err(()) => println!("kill failed"),
    }
}
