//! Minimal user-space standard library for `self_rust_os`.
//!
//! `userlib` is the SDK for writing Ring 3 programs: it provides safe wrappers
//! around the kernel syscall ABI ([`sys`]), console formatting macros
//! ([`io`]), and the program runtime (entry point + panic handler, linked
//! automatically).
//!
//! A program using `userlib` looks like this:
//!
//! ```rust,no_run
//! #![no_std]
//! #![no_main]
//!
//! use userlib::{println, sys};
//!
//! #[no_mangle]
//! pub extern "C" fn user_main() {
//!     println!("Hello from user space!");
//!     sys::exit(0);
//! }
//! ```
//!
//! The crate defines `#[no_mangle] _start` and `#[panic_handler]`, so programs
//! must not define their own.

#![no_std]

pub mod io;
pub mod sys;

mod rt;
