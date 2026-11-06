extern crate alloc;
use userlib::{process::Child, sys};

fn raw(number: u64, a: u64, b: u64, c: u64) -> u64 {
    // SAFETY: Test-only invalid scalar/pointer requests must be rejected at the
    // syscall boundary without dereferencing unmapped or foreign memory.
    unsafe { sys::syscall(number, a, b, c) }
}
fn main(_: usize, argv: &[&str]) {
    if argv.contains(&"--unconnected") {
        assert_eq!(raw(34, 1, 1024 * 1024, 0), sys::SYSCALL_ERROR);
        return;
    }
    if argv.contains(&"--child") {
        let mut content = [0; 128];
        assert!(sys::read_file("/README.TXT", &mut content).is_some());
        for (profile, budget, flags) in [
            (0, 1048576, 0),
            (1, 0, 0),
            (1, 65537, 0),
            (1, u64::MAX, 0),
            (1, 1048576, 1),
        ] {
            assert_eq!(raw(34, profile, budget, flags), sys::SYSCALL_ERROR);
        }
        use core::sync::atomic::{AtomicBool, Ordering};
        let done = userlib::sync::Arc::new(AtomicBool::new(false));
        let worker_done = done.clone();
        let sibling = userlib::thread::spawn(move || {
            while !worker_done.load(Ordering::Acquire) {
                userlib::thread::yield_now();
            }
        })
        .unwrap();
        assert_eq!(
            raw(34, 1, 1024 * 1024, 0),
            sys::SYSCALL_ERROR,
            "cannot race a sibling while dropping rights"
        );
        done.store(true, Ordering::Release);
        sibling.join().unwrap();
        assert_eq!(raw(34, 1, 1024 * 1024, 0), 0, "restrict connected worker");
        if argv.contains(&"--fault") {
            // SAFETY: Deliberate invalid instruction in an isolated restricted
            // regression child, with no pointer operands or Rust references.
            // The parent verifies only this process dies and resources recover.
            unsafe {
                core::arch::asm!("ud2", options(noreturn));
            }
        }
        if argv.contains(&"--block") {
            loop {
                core::hint::spin_loop();
            }
        }
        assert!(
            sys::read_file("/README.TXT", &mut content).is_none(),
            "valid file read loses authority"
        );
        assert!(
            sys::spawn("/BIN/ZSANDBOX.BIN").is_err(),
            "valid executable cannot escape restriction"
        );
        assert!(
            userlib::thread::spawn(|| 42).is_err(),
            "valid thread creation loses authority"
        );
        assert_eq!(
            raw(34, 1, 2 * 1024 * 1024, 0),
            sys::SYSCALL_ERROR,
            "irreversible principal"
        );
        for number in [
            1, 2, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 18, 19, 20, 21, 23, 24, 25, 26, 27,
            28, 29, 30, 33, 64, 65, 66, 67, 68, 69, 70, 98, 100, 101, 104, 105, 106, 107, 108, 110,
        ] {
            assert_eq!(
                raw(number, 0, 0, 0),
                sys::SYSCALL_ERROR,
                "forbidden capability {}",
                number
            );
        }
        assert_eq!(
            raw(109, 8, 1, 0),
            sys::SYSCALL_ERROR,
            "no keyboard capability"
        );
        let base = raw(31, 0, 0, 0);
        let cap = 0x1_0000_0000 + 1024 * 1024;
        assert_eq!(raw(31, cap, 0, 0), cap);
        assert_eq!(raw(31, cap + 1, 0, 0), cap);
        assert_eq!(raw(31, base, 0, 0), base);
        assert_eq!(
            raw(31, base + 1024 * 1024 + 4096, 0, 0),
            base,
            "page quota is atomic"
        );
        assert_eq!(raw(31, base + 4096, 0, 0), base + 4096);
        assert_eq!(raw(31, base, 0, 0), base);
        userlib::println!("[sandbox] child PASS");
        return;
    }
    // Establish the parent's allocator before the resource snapshot. Otherwise
    // its first spawn grows its own heap by 16 pages plus two page-table frames.
    let scratch = alloc::vec![0u8;32*1024];
    core::hint::black_box(scratch.as_ptr());
    drop(scratch);
    let before = raw(107, 0, 0, 0);
    let parent_break = raw(31, 0, 0, 0);
    assert_eq!(
        raw(34, 1, 1048576, 0),
        sys::SYSCALL_ERROR,
        "display owner cannot restrict"
    );
    let unconnected = sys::spawn_with_args("/BIN/ZSANDBOX.BIN", &["--unconnected"]).unwrap();
    assert_eq!(sys::waitpid(unconnected), Ok(0));
    for _ in 0..3 {
        let mut child = Child::spawn_with_args("/BIN/ZSANDBOX.BIN", &["--child"])
            .expect("connected ordinary child");
        let started = sys::ticks();
        let mut text = [0u8; 1024];
        let mut used = 0;
        loop {
            match child.read() {
                Ok(Some((part, n))) => {
                    assert!(used + n <= text.len());
                    text[used..used + n].copy_from_slice(&part[..n]);
                    used += n;
                    if text[..used]
                        .windows(b"[sandbox] child PASS\n".len())
                        .any(|s| s == b"[sandbox] child PASS\n")
                    {
                        break;
                    }
                }
                Ok(None) => {
                    sys::sleep_ticks(1).unwrap();
                }
                Err(_) => panic!(
                    "worker failed: {}",
                    core::str::from_utf8(&text[..used]).unwrap_or("invalid response")
                ),
            }
            assert!(sys::ticks().wrapping_sub(started) < 100, "worker deadline");
        }
        drop(child);
        // Any remaining retired stack frames must settle without allocating a
        // new process; repeated resource checks include endpoint queue teardown.
        assert_eq!(
            raw(31, 0, 0, 0),
            parent_break,
            "parent heap remains unchanged during child check"
        );
        let deadline = sys::ticks() + 50;
        while raw(107, 0, 0, 0) != before {
            assert!(sys::ticks() < deadline, "child frames reclaimed");
            sys::sleep_ticks(1).unwrap();
        }
    }
    use rustos_browser_worker::{native::Native, Error};
    for _ in 0..3 {
        let mut worker = Native::start(|| false).expect("sealed engine handshake");
        let mapped = raw(107, 0, 0, 0) - before;
        assert!(
            mapped < 8192,
            "worker footprint fits image/stack/surface/heap envelope"
        );
        userlib::println!("[sandbox] active_worker_frames={}", mapped);
        assert_eq!(
            worker
                .eval("class C { n=6; f(){return this.n*7} }; new C().f()", || {
                    false
                })
                .unwrap(),
            "42"
        );
        assert_eq!(worker.eval("for(;;){}", || false), Err(Error::Engine(2)));
        assert_eq!(
            worker.eval("'x'.repeat(10*1024*1024)", || false),
            Err(Error::Engine(3))
        );
        assert_eq!(worker.eval("6*7", || false).unwrap(), "42");
        assert_eq!(worker.eval("6*7", || true), Err(Error::Cancelled));
        drop(worker);
        let deadline = sys::ticks() + 50;
        while raw(107, 0, 0, 0) != before {
            assert!(sys::ticks() < deadline, "engine frames reclaimed");
            sys::sleep_ticks(1).unwrap();
        }
    }
    let fault = Child::spawn_with_args("/BIN/ZSANDBOX.BIN", &["--child", "--fault"]).unwrap();
    let result = rustos_browser_worker::Session::connect(Native::owned(fault), || false);
    assert!(
        matches!(result, Err(Error::Unavailable)),
        "native child fault is contained"
    );
    let blocked = Child::spawn_with_args("/BIN/ZSANDBOX.BIN", &["--child", "--block"]).unwrap();
    let result = rustos_browser_worker::Session::connect(Native::owned(blocked), || false);
    assert!(
        matches!(result, Err(Error::Deadline)),
        "hung bootstrap is killed and reaped"
    );
    let mut malformed = Child::spawn("/BIN/JWORKER.SVC").unwrap();
    let start = sys::ticks();
    let mut ready = 0;
    while ready < rustos_browser_worker::HEADER {
        match malformed.read() {
            Ok(Some((_, n))) => ready += n,
            Ok(None) => {
                sys::sleep_ticks(1).unwrap();
            }
            Err(_) => panic!("missing ready"),
        }
        assert!(sys::ticks().wrapping_sub(start) < 50);
    }
    for _ in 0..rustos_browser_worker::HEADER {
        malformed.write_byte(255).unwrap();
    }
    loop {
        match malformed.read() {
            Err(_) => break,
            Ok(Some(_)) => panic!("malformed request produced data"),
            Ok(None) => {
                sys::sleep_ticks(1).unwrap();
            }
        }
        assert!(sys::ticks().wrapping_sub(start) < 50);
    }
    drop(malformed);
    let deadline = sys::ticks() + 50;
    while raw(107, 0, 0, 0) != before {
        assert!(
            sys::ticks() < deadline,
            "fault/deadline/protocol resources recovered"
        );
        sys::sleep_ticks(1).unwrap();
    }
    userlib::println!("[sandbox] PASS restrictions, engine recovery, cancellation, crash, deadline, malformed IPC and frame recovery");
}
