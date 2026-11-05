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
#![cfg_attr(feature = "alloc", feature(alloc_error_handler))]
#[cfg(feature = "alloc")]
extern crate alloc;
#[cfg(all(feature = "alloc", target_os = "none"))]
mod heap;
#[cfg(feature = "tls")]
pub mod tls;

pub mod http;
pub mod io;
pub mod net;
pub mod process;
pub mod sys;
pub mod system;
pub mod ui;

mod rt;

/// Blocking process-private synchronization.
#[cfg(feature = "threads")]
pub mod sync;
/// Native OS threads (enable the threads feature).
#[cfg(feature = "threads")]
pub mod thread;
