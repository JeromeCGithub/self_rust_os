//! Standalone HTTP client. The current process ABI has no argv; prompt for a URL.
#![no_std]
#![no_main]
use userlib::{http, println, sys};
#[no_mangle]
pub extern "C" fn user_main() {
    println!("curl: enter an http:// or https:// URL (empty line exits)");
    loop {
        sys::console_write(b"curl> ");
        let mut line = [0; 512];
        let mut len = 0;
        let mut overflow = false;
        loop {
            let mut byte = [0];
            if sys::read(sys::STDIN_FD, &mut byte) == 0 {
                continue;
            }
            match byte[0] {
                b'\r' | b'\n' => {
                    sys::console_write(b"\n");
                    break;
                }
                8 | 127 => {
                    if len > 0 {
                        len -= 1;
                        sys::console_write(b"\x08 \x08");
                    }
                }
                b if (32..127).contains(&b) => {
                    if len < line.len() {
                        line[len] = b;
                        len += 1;
                        sys::console_write(&[b]);
                    } else {
                        overflow = true;
                    }
                }
                _ => (),
            }
        }
        if len == 0 {
            return;
        }
        if overflow {
            println!("curl: URL exceeds input limit");
            continue;
        }
        let Ok(url) = core::str::from_utf8(&line[..len]) else {
            println!("curl: invalid URL");
            continue;
        };
        let result = http::get(url, |bytes| {
            if sys::console_write(bytes) != bytes.len() {
                Err(http::Error::Output)
            } else {
                Ok(())
            }
        });
        println!();
        match result {
            Ok(response) => println!("curl: HTTP {}, {} bytes", response.status, response.bytes),
            Err(error) => println!("curl: {}", http::error_message(error)),
        }
    }
}
