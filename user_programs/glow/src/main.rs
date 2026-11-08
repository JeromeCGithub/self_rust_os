// Trusted M1 daemon: steady observable progress while its sibling crashes.
use userlib::{recordln as println, sys};
fn main() {
    let mut beat = 0u64;
    loop {
        beat += 1;
        println!("[glow] beat={}", beat);
        sys::sleep_ticks(9).expect("glow deadline");
    }
}
