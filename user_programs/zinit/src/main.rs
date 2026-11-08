// Test-only PID 1 replacement, installed only into disposable E2E image copies.
mod abi;
use userlib::{recordln, supervision, sys};
fn main(argc: usize, argv: &[&str]) {
    if argc > 1 && argv[1] == "--intruder" {
        abi::intruder();
        return;
    }
    assert_eq!(supervision::current().expect("identity").pid, 1);
    let mut mode = [0; 32];
    let n = sys::read_file("/ETC/INIT.RUN", &mut mode).expect("test mode");
    if &mode[..n] == b"abi" {
        abi::run();
    }
    if &mode[..n] == b"lifecycle" {
        abi::lifecycle();
    }
    if &mode[..n] == b"exhaustion" {
        abi::exhaustion();
    }
    sys::spawn("/BIN/GLOW.SVC").expect("live sibling probe");
    sys::sleep_ticks(2).expect("allow sibling to run");
    recordln!("[init-probe] departing");
    match &mode[..n] {
        b"fatal-exit" => sys::exit(42),
        b"fatal-fault" => {
            // SAFETY: Deliberately raises #UD in an isolated test process. No
            // memory or Rust references are accessed; the kernel must stop it.
            unsafe {
                core::arch::asm!("ud2", options(noreturn, nostack));
            }
        }
        b"fatal-thread" => {
            // SAFETY: The native thread-exit ABI takes only zero reserved scalars.
            // This is the process's last thread; a return is a kernel test failure.
            unsafe {
                sys::syscall(99, 0, 0, 0);
            }
            panic!("last-thread exit returned")
        }
        _ => panic!("unsupported test mode"),
    }
}
