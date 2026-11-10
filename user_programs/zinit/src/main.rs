// Test-only PID 1 replacement, installed only into disposable E2E image copies.
extern crate alloc;
mod abi;
mod ipc_probe;
mod principal_probe;
mod capabilities_probe;
use userlib::{recordln, supervision, sys};
fn main(argc: usize, argv: &[&str]) {
    if argc > 1 && argv[1].starts_with("--cap-") {
        capabilities_probe::child(argv[1]);
        return;
    }
    if argc > 1 && matches!(argv[1], "--principal-child" | "--principal-grandchild") {
        principal_probe::child(argv[1] == "--principal-child");
        return;
    }
    if argc > 1 && argv[1] == "--halt-intruder" {
        abi::halt_intruder();
        return;
    }
    if argc > 1 && argv[1] == "--stop-peer" {
        ipc_probe::stop_peer();
        return;
    }
    if argc > 1 && argv[1] == "--ipc-quick" {
        ipc_probe::quick_peer();
        return;
    }
    if argc > 1 && argv[1] == "--ipc-peer" {
        ipc_probe::peer();
        return;
    }
    if argc > 1 && argv[1] == "--unit-probe" {
        assert_eq!(
            argv,
            &[
                "/BIN/ZINIT.SVC",
                "--unit-probe",
                "two words",
                "$(literal)*",
                "",
                "café"
            ]
        );
        assert_eq!(
            userlib::env::var("UNIT_EXAMPLE").as_deref(),
            Some("value=literal")
        );
        assert_eq!(userlib::env::var("UNIT_EMPTY").as_deref(), Some(""));
        assert_eq!(
            userlib::env::var("PATH"),
            None,
            "manager environment must not leak into services"
        );
        recordln!("[unit-probe] vectors PASS");
        loop {
            sys::wait_events(sys::Events::CHILD, 0).expect("probe event wait");
        }
    }
    if argc > 1 && argv[1] == "--intruder" {
        abi::intruder();
        return;
    }
    let identity = supervision::current().expect("identity");
    let mut mode = [0; 32];
    let n = sys::read_file("/ETC/INIT.RUN", &mut mode).expect("test mode");
    if &mode[..n] == b"principal" { principal_probe::run(identity.pid); }
    assert_eq!(identity.pid, 1);
    if &mode[..n] == b"capabilities" { capabilities_probe::run(); }
    if matches!(&mode[..n], b"halt" | b"halt-audio" | b"halt-no-devices") {
        abi::halt(&mode[..n] == b"halt-audio");
    }
    if &mode[..n] == b"halt-abi" {
        abi::halt_abi();
    }
    if &mode[..n] == b"stop-protocol" {
        ipc_probe::stop_protocol();
    }
    if &mode[..n] == b"ipc-lifecycle" {
        ipc_probe::lifecycle();
    }
    if &mode[..n] == b"ipc" {
        ipc_probe::run();
    }
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
