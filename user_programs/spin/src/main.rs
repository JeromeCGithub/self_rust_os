//! spin: CPU-bound test program for scheduler preemption testing.
//!
//! Prints a counter in a tight loop. If the scheduler preempts correctly,
//! the shell remains responsive while this runs.

#![no_std]
#![no_main]

use userlib::{println, sys};

#[no_mangle]
pub extern "C" fn user_main() {
    println!("spin: starting (pid will be shown by shell)");
    let mut counter: u64 = 0;
    loop {
        counter += 1;
        // Print every ~10 million iterations to avoid flooding serial.
        if counter % 10_000_000 == 0 {
            println!("spin: counter={}", counter);
        }
        // Prevent the loop from being optimized away.
        core::hint::black_box(counter);
    }
}
