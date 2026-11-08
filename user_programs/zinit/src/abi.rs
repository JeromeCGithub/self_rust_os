//! Actual Ring 3 assertions; all raw syscall errors are intentional boundary probes.
use userlib::{
    recordln,
    supervision::{self, Cause, ChildEvent, ProcessKey},
    sys,
};
fn raw(number: u64, a: u64, b: u64, c: u64) -> u64 {
    // SAFETY: This test intentionally supplies scalar or invalid pointer values.
    // Valid output pointers name this process's live exclusive stack buffers;
    // invalid ranges must be rejected by the kernel without access or retention.
    unsafe { sys::syscall(number, a, b, c) }
}
fn completion() -> ChildEvent {
    loop {
        if let Some(event) = supervision::next_child().expect("child consume") {
            return event;
        }
        sys::wait_events(sys::Events::CHILD, 0).expect("atomic child wait");
    }
}
pub fn intruder() {
    for pid in [1, 2] {
        assert_eq!(raw(9, pid, 0, 0), sys::SYSCALL_ERROR, "foreign kill");
        assert_eq!(raw(10, pid, 0, 0), sys::SYSCALL_ERROR, "foreign wait");
        assert!(supervision::terminate(ProcessKey {
            pid: pid as u32,
            generation: 1
        })
        .is_err());
    }
    assert_eq!(supervision::next_child(), Ok(None));
}
pub fn run() {
    assert_eq!(
        supervision::current(),
        Ok(ProcessKey {
            pid: 1,
            generation: 1
        })
    );
    let child = sys::spawn_with_args("/BIN/FLICKER.SVC", &["--fail"]).expect("child");
    assert_eq!(child, 2);
    let mut denied_output = [0u8; 40];
    for (number, a, b, c) in [(14, 1, denied_output.as_mut_ptr() as u64, 40),
        (15, 1, 0, 0), (19, 1, 0, 0), (109, 16, 1, 0),
        (37, (1u64 << 32) | 1, 0, 0)] {
        assert_eq!(raw(number, a, b, c), sys::SYSCALL_ERROR, "init has no display or self-child authority");
    }
    for number in [9, 10] {
        assert_eq!(
            raw(number, (1u64 << 32) | u64::from(child), 0, 0),
            sys::SYSCALL_ERROR,
            "PID arguments must never truncate high bits"
        );
        assert_eq!(
            raw(number, u64::from(child), 1, 0),
            sys::SYSCALL_ERROR,
            "reserved word"
        );
        assert_eq!(
            raw(number, u64::from(child), 0, 1),
            sys::SYSCALL_ERROR,
            "reserved word"
        );
    }
    let intruder = sys::spawn_with_args("/BIN/ZINIT.SVC", &["--intruder"]).expect("intruder");
    for (number, a, b, c) in [
        (35, 1, 0, 0),
        (35, 0, 1, 0),
        (35, 0, 0, 1),
        (37, (2u64 << 32) | u64::from(child), 0, 0),
        (109, 0, 0, 0),
        (109, 64, 0, 0),
        (109, 32, u64::MAX, 0),
    ] {
        assert_eq!(raw(number, a, b, c), sys::SYSCALL_ERROR);
    }
    sys::sleep_ticks(4).expect("both children become terminal");
    for (pointer, size, reserved) in [
        (0, 40, 0),
        (0x400000, 40, 0),
        (0x60000000, 40, 0),
        (0x3fffffff8, 40, 0),
        (u64::MAX - 10, 40, 0),
        (0, 0, 0),
        (0, 40, 1),
    ] {
        assert_eq!(raw(36, pointer, size, reserved), sys::SYSCALL_ERROR);
    }
    // A valid output spans two writable pages; guard bytes must survive.
    let mut storage = [0xa5u8; 8192];
    let base = storage.as_mut_ptr() as usize;
    let address = ((base + 4095) & !4095) + 4090;
    let offset = address - base;
    assert!(offset > 0 && offset + 40 < storage.len());
    assert_eq!(raw(36, address as u64, 40, 0), 1);
    assert_eq!(storage[offset - 1], 0xa5);
    assert_eq!(storage[offset + 40], 0xa5);
    let first = ChildEvent::decode(storage[offset..offset + 40].try_into().unwrap()).unwrap();
    let second = completion();
    for pid in [child, intruder] {
        let expected = ChildEvent::Completed {
            process: ProcessKey { pid, generation: 1 },
            cause: Cause::Exited,
            status: if pid == child { 23 } else { 0 },
        };
        assert!(
            first == expected || second == expected,
            "missing {:?}: {:?}, {:?}",
            expected,
            first,
            second
        );
        assert_eq!(
            sys::waitpid(pid),
            Err(()),
            "a reaped child cannot be consumed twice"
        );
        assert!(supervision::terminate(ProcessKey { pid, generation: 1 }).is_err());
    }
    assert_eq!(supervision::next_child(), Ok(None));
    assert!(sys::spawn("/BIN/ABSENT.SVC").is_err());
    assert!(sys::spawn("/BIN/BAD.SVC").is_err());
    recordln!("[init-probe] ABI PASS");
    loop {
        sys::wait_events(sys::Events::CHILD, 0).expect("finished probe stays live");
    }
}

fn frames() -> u64 {
    let count = raw(107, 0, 0, 0);
    assert!(count < u32::MAX as u64, "frame counter unavailable");
    count
}
fn cycle(iteration: usize) {
    let pid = sys::spawn_with_env("/BIN/FLICKER.SVC", &[], &[]).expect("repeated child");
    if iteration % 4 == 0 {
        sys::sleep_ticks(1).expect("exercise child execution");
    }
    assert!(sys::spawn("/BIN/BAD.SVC").is_err());
    let key = ProcessKey { pid, generation: 1 };
    supervision::terminate(key).expect("owned termination");
    assert_eq!(
        completion(),
        ChildEvent::Completed {
            process: key,
            cause: Cause::Killed,
            status: 9
        }
    );
    assert_eq!(supervision::next_child(), Ok(None));
}
pub fn lifecycle() {
    // Warm SDK allocation and kernel construction paths before the fixed baseline.
    cycle(0);
    cycle(1);
    sys::sleep_ticks(1).expect("initial quiescence");
    let baseline = frames();
    for iteration in 0..64 {
        cycle(iteration);
        let deadline = sys::ticks().checked_add(10).unwrap();
        while frames() != baseline {
            assert!(sys::ticks() < deadline, "child frames did not recover");
            sys::sleep_ticks(1).expect("wait for remote reclamation");
        }
    }
    assert_eq!(frames(), baseline);
    recordln!("[init-probe] lifecycle PASS cycles=64 frames={}", baseline);
    loop {
        sys::wait_events(sys::Events::CHILD, 0).expect("finished probe stays live");
    }
}

pub fn exhaustion() {
    cycle(0);
    sys::sleep_ticks(1).expect("warm-up quiescence");
    let baseline = frames();
    let mut children = [None; 127];
    let mut count = 0;
    while count < children.len() {
        match sys::spawn_with_env("/BIN/FLICKER.SVC", &[], &[]) {
            Ok(pid) => {
                children[count] = Some(ProcessKey { pid, generation: 1 });
                count += 1;
            }
            Err(()) => break,
        }
    }
    assert!(
        count >= 2 && count < children.len(),
        "must exercise allocation/quota failure"
    );
    let occupied = frames();
    assert!(sys::spawn_with_env("/BIN/FLICKER.SVC", &[], &[]).is_err());
    assert_eq!(
        frames(),
        occupied,
        "failed unpublished process must roll back frames"
    );
    for child in children[..count].iter().flatten() {
        supervision::terminate(*child).expect("terminate exhausted child");
    }
    for _ in 0..count {
        match completion() {
            ChildEvent::Completed {
                process,
                cause: Cause::Killed,
                status: 9,
            } => {
                let slot = children
                    .iter_mut()
                    .find(|slot| **slot == Some(process))
                    .expect("unique owned result");
                *slot = None;
            }
            event => panic!("unexpected {:?}", event),
        }
    }
    let deadline = sys::ticks().checked_add(10).unwrap();
    while frames() != baseline {
        assert!(
            sys::ticks() < deadline,
            "exhaustion cleanup did not recover"
        );
        sys::sleep_ticks(1).expect("remote quiescence");
    }
    assert_eq!(supervision::next_child(), Ok(None));
    recordln!(
        "[init-probe] exhaustion PASS children={} frames={}",
        count,
        baseline
    );
    loop {
        sys::wait_events(sys::Events::CHILD, 0).expect("finished probe stays live");
    }
}
