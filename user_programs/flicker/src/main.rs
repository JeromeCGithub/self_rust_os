// Trusted M1 daemon with explicit fault-injection arguments, never implicit failure.
use userlib::{recordln as println, sys};
fn main(argc: usize, argv: &[&str]) {
    println!("[flicker] started");
    if argc > 1 && argv[1] == "--fault" {
        // SAFETY: This trusted fixture deliberately raises #UD without touching
        // memory. Kernel process-fault isolation is the behavior under test.
        unsafe { core::arch::asm!("ud2", options(noreturn, nostack)); }
    }
    if argc > 1 && argv[1] == "--orphans" {
        sys::spawn("/BIN/FLICKER.SVC").expect("live orphan");
        sys::spawn_with_args("/BIN/FLICKER.SVC", &["--fail"]).expect("terminal orphan");
        sys::sleep_ticks(4).expect("terminal child exits before owner");
        sys::exit(17);
    }
    if argc > 1 && argv[1] == "--fail" {
        sys::sleep_ticks(2).expect("failure deadline");
        sys::exit(23);
    }
    loop {
        sys::sleep_ticks(18).expect("flicker deadline");
    }
}
