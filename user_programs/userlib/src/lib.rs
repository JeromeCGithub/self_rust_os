//! Minimal user-space standard library for `self_rust_os`.
//!
//! `userlib` is the SDK for writing Ring 3 programs: it provides safe wrappers
//! around the kernel syscall ABI ([`sys`]), console formatting macros
//! ([`io`]), and the program runtime (entry point + panic handler, linked
//! automatically).
//!
//! Applications select `userlib/src/application.rs` as their Cargo binary root
//! and put ordinary `fn main()` code in their own `src/main.rs`:
//!
//! ```rust,no_run
//!
//! use userlib::{println, sys};
//!
//! fn main() {
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
pub mod ui;

mod rt;
