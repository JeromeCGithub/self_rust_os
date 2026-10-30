//! Shared Cargo binary root for RustOS applications.
//!
//! Each application selects this file as its [[bin]] path. Its own src/main.rs
//! supplies ordinary Rust code and fn main(); the SDK owns the target boilerplate.
#![no_std]
#![no_main]

include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs"));

// Preserve the runtime's private linkage contract without exposing it to apps.
#[no_mangle]
pub extern "C" fn user_main() {
    main();
}
