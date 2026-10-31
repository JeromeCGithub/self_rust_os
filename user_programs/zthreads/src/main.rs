use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use rustos_thread_abi as abi;
use userlib::{
    println,
    sync::{Arc, Condvar, Mutex, Once},
    sys, thread,
};

fn raw(number: u64, a: u64, b: u64, c: u64) -> u64 {
    // SAFETY: Regression probes deliberately pass invalid scalar/pointer
    // arguments; the kernel must reject them without dereferencing bad memory.
    unsafe { sys::syscall(number, a, b, c) }
}
fn until(mut predicate: impl FnMut() -> bool) {
    let deadline = sys::ticks() + 300;
    while !predicate() {
        assert!(sys::ticks() < deadline, "bounded thread progress");
        thread::yield_now();
    }
}
fn fault_probe(mode: u8) {
    if mode == 4 {
        // SAFETY: Deliberately privileged instruction in an isolated Ring 3
        // child. Its GP fault must terminate only this process with status 13.
        unsafe {
            core::arch::asm!("cli", options(nostack));
        }
        panic!("Ring 3 unexpectedly disabled interrupts");
    }
    if mode == 3 {
        let started = Arc::new(AtomicU32::new(0));
        let signal = Arc::clone(&started);
        let _worker = thread::Builder::new()
            .on_cpu(0)
            .spawn(move || {
                let data = [0x5au8; 8192];
                signal.store(1, Ordering::Release);
                loop {
                    assert_eq!(sys::write_file("/TMP/IO.DAT", &data), Some(data.len()));
                }
            })
            .unwrap();
        until(|| started.load(Ordering::Acquire) == 1);
        // Register-only owned standard I/O stays usable while the sibling does
        // filesystem work; the parent can kill us without acquiring the FS lock.
        assert_eq!(sys::console_write(b"R"), 1);
        loop {
            thread::sleep_ticks(1).unwrap();
        }
    }
    if mode == 1 {
        let value: u64;
        // SAFETY: This isolated regression child deliberately reads its unmapped
        // stack guard. The kernel must terminate this PID, preserving its parent.
        unsafe {
            core::arch::asm!("mov {value},qword ptr [{address}]",value=out(reg)value,address=in(reg)abi::STACK_ARENA,options(nostack,readonly));
        }
        panic!("guard unexpectedly accessible: {}", value);
    }
    let finish = Arc::new(AtomicU32::new(0));
    let target = Arc::new(AtomicU64::new(0));
    let other = Arc::clone(&finish);
    let location = Arc::clone(&target);
    let owner = thread::Builder::new()
        .on_cpu(0)
        .spawn(move || {
            let local = 31u64;
            location.store(
                (core::ptr::addr_of!(local) as u64) & !4095,
                Ordering::Release,
            );
            while other.load(Ordering::Acquire) == 0 {
                thread::yield_now();
            }
            core::hint::black_box(local)
        })
        .unwrap();
    until(|| target.load(Ordering::Acquire) != 0);
    let ready = Arc::new(AtomicU32::new(0));
    let go = Arc::new(AtomicU32::new(0));
    let started = Arc::clone(&ready);
    let released = Arc::clone(&go);
    let reader=thread::Builder::new().on_cpu(if thread::available_parallelism()>1 {1}else{0}).spawn(move || {
        let address=target.load(Ordering::Acquire);
        let mut value:u64;
        // SAFETY: Test-only byte snapshot of a mapped sibling stack page. Inline
        // assembly creates no Rust reference to concurrently accessed user RAM.
        unsafe {core::arch::asm!("mov {value},qword ptr [{address}]",value=out(reg)value,address=in(reg)address,options(nostack,readonly));}
        core::hint::black_box(value);
        started.store(1,Ordering::Release);
        while released.load(Ordering::Acquire)==0 {core::hint::spin_loop();}
        // SAFETY: Deliberately fault after remote unmapping. A stale TLB would
        // incorrectly allow this instruction to complete in this isolated child.
        unsafe {core::arch::asm!("mov {value},qword ptr [{address}]",value=out(reg)value,address=in(reg)address,options(nostack,readonly));}
        panic!("freed stack still accessible: {}",value);
    }).unwrap();
    until(|| ready.load(Ordering::Acquire) == 1);
    finish.store(1, Ordering::Release);
    assert_eq!(owner.join().unwrap(), 31);
    until(|| thread::stack_pages() == 64);
    go.store(1, Ordering::Release);
    reader.join().unwrap();
    panic!("faulting child returned");
}
fn main() {
    let mut mode = [0u8; 1];
    if sys::read_file("/TMP/THREAD.CFG", &mut mode) == Some(1) && mode[0] != 0 {
        fault_probe(mode[0]);
        return;
    }
    let cpus = thread::available_parallelism();
    assert!((1..=32).contains(&cpus));
    assert_eq!(
        raw(abi::SYS_MESSAGE_WAIT, 1, 0, 0),
        0,
        "empty mailbox wait must time out"
    );
    assert_eq!(
        raw(abi::SYS_MESSAGE_WAIT, u64::MAX, 0, 0),
        sys::SYSCALL_ERROR
    );
    assert_eq!(raw(abi::SYS_MESSAGE_WAIT, 1, 1, 0), sys::SYSCALL_ERROR);
    assert_eq!(
        raw(abi::SYS_THREAD_YIELD, 0, 0, 0),
        0,
        "yield must normalize the saved return register"
    );
    let mut workers = [const { None }; 32];
    let ready = Arc::new(AtomicU32::new(0));
    let go = Arc::new(AtomicU32::new(0));
    let active = Arc::new(AtomicU32::new(0));
    let all = u32::MAX >> (32 - cpus);
    for cpu in 0..cpus {
        let ready = Arc::clone(&ready);
        let go = Arc::clone(&go);
        let active = Arc::clone(&active);
        workers[cpu] = Some(
            thread::Builder::new()
                .on_cpu(cpu)
                .spawn(move || {
                    assert_eq!(thread::current_cpu(), cpu, "worker affinity");
                    let local = cpu as u64;
                    let address = core::ptr::addr_of!(local) as usize;
                    ready.fetch_or(1u32 << cpu, Ordering::Release);
                    let deadline = sys::ticks() + 300;
                    while go.load(Ordering::Acquire) == 0 {
                        core::hint::spin_loop();
                        assert!(sys::ticks() < deadline, "start barrier");
                    }
                    active.fetch_or(1u32 << cpu, Ordering::AcqRel);
                    // All participants must occupy distinct reported CPUs before any
                    // finishes. There is no voluntary yield in this rendezvous.
                    while active.load(Ordering::Acquire) != all {
                        core::hint::spin_loop();
                        assert!(sys::ticks() < deadline, "parallel CPU rendezvous");
                    }
                    let mut sum = 0u64;
                    for i in 0..20_000u64 {
                        sum = sum.wrapping_add(core::hint::black_box(i ^ local));
                    }
                    assert_eq!(thread::current_cpu(), cpu);
                    (cpu, address, sum)
                })
                .expect("pinned native worker"),
        );
    }
    until(|| ready.load(Ordering::Acquire) == all);
    go.store(1, Ordering::Release);
    let mut addresses = [0usize; 32];
    for (cpu, handle) in workers.iter_mut().take(cpus).enumerate() {
        let (actual, address, sum) = handle
            .take()
            .expect("worker handle")
            .join()
            .expect("worker join");
        assert_eq!(actual, cpu);
        assert!(address >= abi::STACK_ARENA as usize && address < abi::USER_END as usize);
        assert!(
            !addresses[..cpu].contains(&address),
            "private worker stacks"
        );
        addresses[cpu] = address;
        assert_eq!(sum, (0..20_000u64).map(|i| i ^ cpu as u64).sum::<u64>());
    }
    println!(
        "[threads] parallel CPUs={} mask={:#x}",
        cpus,
        active.load(Ordering::Acquire)
    );

    // Contention and Once publication through actual parked Ring 3 threads.
    let counter = Arc::new(Mutex::new(0u64));
    let once = Arc::new(Once::new());
    let initialized = Arc::new(AtomicU32::new(0));
    let mut handles = [const { None }; 8];
    for slot in &mut handles {
        let counter = Arc::clone(&counter);
        let once = Arc::clone(&once);
        let initialized = Arc::clone(&initialized);
        *slot = Some(
            thread::spawn(move || {
                once.call_once(|| {
                    initialized.fetch_add(1, Ordering::Relaxed);
                });
                for _ in 0..500 {
                    *counter.lock() += 1;
                }
            })
            .expect("counter worker"),
        );
    }
    for slot in &mut handles {
        slot.take().unwrap().join().unwrap();
    }
    assert_eq!(*counter.lock(), 4000);
    assert_eq!(initialized.load(Ordering::Acquire), 1);
    let condition = Arc::new((Mutex::new(false), Condvar::new()));
    let other = Arc::clone(&condition);
    let waiter = thread::spawn(move || {
        let mut guard = other.0.lock();
        while !*guard {
            guard = other.1.wait(guard);
        }
        73
    })
    .unwrap();
    {
        let mut guard = condition.0.lock();
        *guard = true;
        condition.1.notify_one();
    }
    assert_eq!(waiter.join().unwrap(), 73);

    let word = AtomicU32::new(1);
    assert_eq!(
        thread::wait(&word, 0, None).unwrap(),
        thread::Wait::Mismatch
    );
    assert_eq!(
        thread::wait(&word, 1, Some(1)).unwrap(),
        thread::Wait::TimedOut
    );
    for _ in 0..16 {
        let signal = Arc::new(AtomicU32::new(0));
        let other = Arc::clone(&signal);
        let worker = thread::spawn(move || {
            while other.load(Ordering::Acquire) == 0 {
                thread::wait(&other, 0, None).unwrap();
            }
        })
        .unwrap();
        signal.store(1, Ordering::Release);
        thread::wake(&signal, 1).unwrap();
        worker.join().unwrap();
    }
    println!("[threads] synchronization PASS");

    // Invalid ABI arguments, user guard mappings, stale/self join and rollback.
    assert_eq!(
        raw(abi::SYS_THREAD_JOIN, thread::current(), 0, 0),
        sys::SYSCALL_ERROR
    );
    for ptr in [0, 0x3000000, abi::STACK_ARENA, u64::MAX - 8] {
        assert_eq!(raw(abi::SYS_THREAD_CREATE, ptr, 40, 0), sys::SYSCALL_ERROR);
    }
    for (version, flags, entry, size, affinity) in [
        (0, 0, 0x400000, 16384, 0),
        (1, 1, 0x400000, 16384, 0),
        (1, 0, 0x3000000, 16384, 0),
        (1, 0, 0x400000, u64::MAX, 0),
        (1, 0, 0x400000, 16384, 1u64 << 32),
    ] {
        let request = abi::Create {
            version,
            flags,
            entry,
            argument: 0,
            stack_size: size,
            affinity,
        };
        assert_eq!(
            raw(
                abi::SYS_THREAD_CREATE,
                core::ptr::addr_of!(request) as u64,
                40,
                0
            ),
            sys::SYSCALL_ERROR
        );
    }
    assert_eq!(raw(abi::SYS_THREAD_WAIT, 1, 0, 0), sys::SYSCALL_ERROR);
    assert_eq!(raw(1, abi::STACK_ARENA, 4, 0), sys::SYSCALL_ERROR);
    let worker = thread::spawn(|| 99).unwrap();
    let stale = worker.id();
    assert_eq!(worker.join().unwrap(), 99);
    assert_eq!(raw(abi::SYS_THREAD_JOIN, stale, 0, 0), sys::SYSCALL_ERROR);
    until(|| thread::stack_pages() == 0);
    let baseline = thread::allocated_frames();
    for _ in 0..64 {
        let worker = thread::spawn(|| 7).unwrap();
        assert_eq!(worker.join().unwrap(), 7);
        until(|| thread::stack_pages() == 0);
        assert_eq!(
            thread::allocated_frames(),
            baseline,
            "create/join frame reclamation"
        );
    }
    let done = Arc::new(AtomicU32::new(0));
    for _ in 0..16 {
        let done = Arc::clone(&done);
        drop(
            thread::spawn(move || {
                done.fetch_add(1, Ordering::Release);
            })
            .unwrap(),
        );
    }
    until(|| done.load(Ordering::Acquire) == 16 && thread::stack_pages() == 0);
    assert_eq!(
        thread::allocated_frames(),
        baseline,
        "detached frame reclamation"
    );
    for _ in 0..3 {
        let pid = sys::spawn("/BIN/SPIN.BIN").expect("independent Ring 3 process");
        thread::yield_now();
        sys::kill(pid).unwrap();
        assert_eq!(sys::waitpid(pid).unwrap(), 9);
        until(|| thread::allocated_frames() == baseline);
    }
    // Freed worker addresses, including cached remote translations, stay absent.
    assert_eq!(raw(1, addresses[0] as u64, 1, 0), sys::SYSCALL_ERROR);
    println!("[threads] lifecycle and invalid arguments PASS");
    for (mode, code) in [(1u8, 14), (2, 14), (4, 13)] {
        assert_eq!(sys::write_file("/TMP/THREAD.CFG", &[mode]), Some(1));
        let child = sys::spawn("/BIN/ZTHREADS.BIN").expect("isolated fault child");
        assert_eq!(
            sys::waitpid(child).unwrap(),
            code,
            "user fault must stop only its process"
        );
        until(|| thread::allocated_frames() == baseline);
    }
    println!("[threads] guard and remote unmap isolation PASS");
    assert_eq!(sys::write_file("/TMP/EMPTY.TXT", &[]), Some(0));
    assert_eq!(sys::write_file("/TMP/THREAD.CFG", &[3]), Some(1));
    let mut child = userlib::process::Child::spawn("/BIN/ZTHREADS.BIN").unwrap();
    until(|| matches!(child.read(),Ok(Some((bytes,count))) if count>0 && bytes[0]==b'R'));
    sys::kill(child.pid()).unwrap();
    assert_eq!(sys::waitpid(child.pid()).unwrap(), 9);
    drop(child);
    until(|| thread::allocated_frames() == baseline);
    assert_eq!(sys::write_file("/TMP/THREAD.CFG", &[0]), Some(1));
    println!("[threads] in-flight service exit PASS");
    let mut stress = [0u8; 1];
    if sys::read_file("/TMP/SMPTEST.CFG", &mut stress) == Some(1) {
        let mut children = [0u32; 128];
        let mut count = 0;
        for slot in &mut children {
            let before = thread::allocated_frames();
            match sys::spawn("/BIN/SPIN.BIN") {
                Ok(pid) => {
                    *slot = pid;
                    count += 1;
                }
                Err(()) => {
                    assert_eq!(
                        thread::allocated_frames(),
                        before,
                        "failed mapping rollback"
                    );
                    break;
                }
            }
        }
        assert!(
            count > 0 && count < 100,
            "physical exhaustion precedes process quota"
        );
        for &pid in &children[..count] {
            sys::kill(pid).unwrap();
            assert_eq!(sys::waitpid(pid).unwrap(), 9);
        }
        until(|| thread::allocated_frames() == baseline);
        println!(
            "[threads] allocation failure rollback PASS processes={}",
            count
        );
    }
    println!("THREAD_TEST_PASS");
}
