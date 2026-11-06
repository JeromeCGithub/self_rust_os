// Terminal user-space program for `self_rust_os`, built on `userlib`.
//
// A simple interactive shell running in Ring 3.
//
// Filesystem commands and modal editing are shell builtins; Portal only renders I/O.

extern crate alloc;
mod command;
mod editor;
mod medit;
mod network;
mod ping;
use userlib::{println, sys};

fn path_text(bytes: &[u8]) -> Result<alloc::string::String, ()> {
    use alloc::string::ToString;
    if bytes.is_empty() {
        return Ok(alloc::string::String::new());
    }
    let command = command::Command::parse(bytes).map_err(|_| ())?;
    if command.count != 1 {
        return Err(());
    }
    Ok(command.word(0).to_string())
}

fn ls(path: &[u8]) {
    let owned_path = match path_text(path) {
        Ok(path) => path,
        Err(()) => {
            println!("invalid path");
            return;
        }
    };
    let path_str = owned_path.as_str();
    match userlib::fs::read_dir(path_str) {
        Err(()) => println!("ls failed"),
        Ok(entries) if entries.is_empty() => println!("(empty)"),
        Ok(entries) => {
            for entry in entries {
                println!(
                    "{}{}",
                    entry.name,
                    if entry.is_directory { "/" } else { "" }
                );
            }
        }
    }
}

fn mkdir(path: &[u8]) {
    let owned_path = match path_text(path) {
        Ok(path) => path,
        Err(()) => {
            println!("invalid path");
            return;
        }
    };
    let path_str = owned_path.as_str();
    match sys::mkdir(path_str) {
        Ok(()) => println!("created {}", path_str),
        Err(()) => println!("mkdir failed"),
    }
}

fn cat(path: &[u8]) {
    let owned_path = match path_text(path) {
        Ok(path) => path,
        Err(()) => {
            println!("invalid path");
            return;
        }
    };
    let path_str = owned_path.as_str();
    let Ok(mut file) = userlib::fs::File::open(path_str, userlib::fs::O_RDONLY) else {
        println!("cat failed");
        return;
    };
    let mut buf = [0u8; 1024];
    loop {
        match file.read(&mut buf) {
            Ok(0) => {
                sys::console_write(b"\n");
                break;
            }
            Ok(n) => {
                sys::console_write(&buf[..n]);
            }
            Err(()) => {
                println!("cat failed");
                break;
            }
        }
    }
}

/// Writes `text` to the file at `path` (creates or overwrites).
fn write(path: &[u8], text: &[u8]) {
    let owned_path = match path_text(path) {
        Ok(path) => path,
        Err(()) => {
            println!("invalid path");
            return;
        }
    };
    let path_str = owned_path.as_str();
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
fn main() {
    println!("self_rust_os forge v1.0");
    println!("Type 'help' for commands.\n");

    let mut line_buf = [0u8; 512];
    let mut network = network::Network::new();
    let mut char_buf = [0u8; 4];

    loop {
        sys::console_write(b"> ");

        // Read a line with echo.
        let mut line_len = 0;
        loop {
            if network.tick() {
                sys::console_write(b"> ");
                sys::console_write(&line_buf[..line_len]);
            }
            let n = sys::read_nb(sys::STDIN_FD, &mut char_buf);
            if n == 0 {
                sys::wait_stdin(network.input_wait_ticks()).expect("shell input wait");
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
        if network.command(line) {
            // Networking policy and output belong to the shell process.
        } else if eq(line, b"env") {
            for (key, value) in userlib::env::vars() {
                println!("{}={}", key, value);
            }
        } else if line.starts_with(b"export ") {
            match command::Command::parse(&line[7..]) {
                Ok(command) if command.count == 1 => {
                    if let Some((key, value)) = command.word(0).split_once('=') {
                        if userlib::env::set_var(key, value).is_ok() {
                            println!("exported {}", key);
                        } else {
                            println!("invalid environment variable");
                        }
                    } else {
                        println!("usage: export NAME=value");
                    }
                }
                _ => println!("usage: export NAME=value"),
            }
        } else if line.starts_with(b"unset ") {
            match command::Command::parse(&line[6..]) {
                Ok(command) if command.count == 1 => {
                    userlib::env::remove_var(command.word(0));
                    println!("unset {}", command.word(0));
                }
                _ => println!("usage: unset NAME"),
            }
        } else if eq(line, b"pwd") {
            let mut cwd = [0; 4096];
            match sys::getcwd(&mut cwd) {
                Ok(path) => println!("{}", path),
                Err(()) => println!("pwd failed"),
            }
        } else if eq(line, b"cd") || line.starts_with(b"cd ") {
            let owned = if line.len() == 2 {
                Ok(alloc::string::String::from("/"))
            } else {
                path_text(&line[3..])
            };
            let path = owned.as_deref().unwrap_or("");
            if path.is_empty() || sys::chdir(path).is_err() {
                println!("cd failed");
            } else {
                let mut cwd = [0; 4096];
                if let Ok(path) = sys::getcwd(&mut cwd) {
                    println!("cwd: {}", path);
                }
            }
        } else if eq(line, b"medit") || line.starts_with(b"medit ") {
            let owned = if line.len() == 5 {
                Err(())
            } else {
                path_text(&line[6..])
            };
            let path = owned.as_deref().unwrap_or("");
            medit::run(path);
        } else if eq(line, b"help") {
            println!("Commands:");
            println!("  help       - show this help");
            println!("  ping [-c 1..16] IPv4");
            println!("  curl http[s]://host[:port]/path");
            println!("  pwd        - print working directory");
            println!("  env / export NAME=value / unset NAME");
            println!("  cd [path]  - change directory (default: /)");
            println!("  medit <f>  - small modal text editor");
            println!("  echo <txt> - print text");
            println!("  clear      - clear screen");
            println!("  time       - show timer ticks");
            println!("  ls [path]  - list directory (default: cwd)");
            println!("  mkdir <p>  - create directory (with parents)");
            println!("  cat <file> - show file contents");
            println!("  write <f> <txt> - write text to file");
            println!("  ./<prog> [args] - run program (quotes supported)");
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
        } else if eq(line, b"mkdir") {
            println!("usage: mkdir <path>");
        } else if eq(line, b"cat") {
            println!("usage: cat <file>");
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
        } else if line.starts_with(b"/") {
            run_program(line);
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
/// PATH resolution (shell policy, not kernel):
/// - If `name` contains '/', use it as-is (explicit path).
/// - Otherwise, try `/bin/<NAME>.BIN` (standard program directory).
///
/// Converts `name` to uppercase 8.3 FAT32 format.
fn run_program(line: &[u8]) {
    let command = match command::Command::parse(line) {
        Ok(command) if command.count > 0 => command,
        Ok(_) => {
            println!("usage: ./<name> [arguments]");
            return;
        }
        Err(error) => {
            println!("parse error: {}", error);
            return;
        }
    };
    let name = command.word(0);
    // Preserve the shell's existing ./name -> /bin/NAME.BIN shorthand. Paths
    // containing slash or an extension are explicit and resolve against cwd.
    let mut path = [0u8; 256];
    let path_len;
    if name.contains('/') || name.contains('.') {
        if name.len() >= path.len() {
            println!("program path too long");
            return;
        }
        path[..name.len()].copy_from_slice(name.as_bytes());
        path_len = name.len();
    } else {
        if name.is_empty() || name.len() > 8 {
            println!("usage: ./<name> (max 8 chars)");
            return;
        }
        path[..5].copy_from_slice(b"/bin/");
        for (index, byte) in name.bytes().enumerate() {
            path[5 + index] = byte.to_ascii_uppercase();
        }
        path[5 + name.len()..9 + name.len()].copy_from_slice(b".BIN");
        path_len = 9 + name.len();
    }
    let Ok(path) = core::str::from_utf8(&path[..path_len]) else {
        println!("invalid program path");
        return;
    };
    let mut arguments = [""; command::MAX_WORDS - 1];
    for (index, argument) in arguments.iter_mut().take(command.count - 1).enumerate() {
        *argument = command.word(index + 1);
    }
    println!("Spawning {}...", path);
    match sys::spawn_with_args(path, &arguments[..command.count - 1]) {
        Ok(pid) => println!("started pid {}", pid),
        Err(()) => println!("failed to spawn {}", path),
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
