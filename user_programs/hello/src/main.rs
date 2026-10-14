//! Minimal user-space program for `self_rust_os`, built on `userlib`.
//!
//! This program runs in Ring 3 and demonstrates the userlib SDK:
//! - `println!` for console output,
//! - `sys::ticks` for the monotonic timer,
//! - `sys::exit` for termination.
//!
//! Previously this program hand-rolled the `int 0x80` syscall ABI; it now
//! uses `userlib` for everything.

#![no_std]
#![no_main]

use userlib::{println, sys};

/// Entry point called by `userlib`'s runtime after `_start`.
#[no_mangle]
pub extern "C" fn user_main() {
    println!("Hello from user space!");
    println!("This message was printed via userlib's println! macro.");
    println!("Timer ticks since boot: {}", sys::ticks());
    println!("Exiting with code 0.");
    sys::exit(0);
}
