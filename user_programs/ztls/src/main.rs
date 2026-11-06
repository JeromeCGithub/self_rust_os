// Compiler local-exec TLS, with initialized and zero-filled per-thread objects.
#[thread_local]
static mut INITIAL: u64 = 0x123456789abcdef0;
#[thread_local]
static mut ZERO: [u8; 128] = [0; 128];
fn exercise(value: u64) {
    // SAFETY: Compiler TLS names the calling thread's uniquely owned instance.
    // No reference or pointer escapes to another thread; volatile accesses keep
    // every post-yield check observable and in bounds.
    unsafe {
        let initial = core::ptr::addr_of_mut!(INITIAL);
        let zero = core::ptr::addr_of_mut!(ZERO).cast::<u8>();
        assert_eq!(initial.read_volatile(), 0x123456789abcdef0);
        assert_eq!(zero.read_volatile(), 0);
        initial.write_volatile(value);
        zero.write_volatile(value as u8);
        for _ in 0..100 {
            userlib::thread::yield_now();
            assert_eq!(initial.read_volatile(), value);
            assert_eq!(zero.read_volatile(), value as u8);
        }
    }
}
fn main() {
    let mut pointer = 0u64;
    // SAFETY: ARCH_GET_FS writes one complete writable local u64; no references
    // are retained. This verifies the Linux-style thread-pointer operation.
    assert_eq!(
        unsafe { userlib::sys::syscall(32, 0x1003, core::ptr::addr_of_mut!(pointer) as u64, 0) },
        0
    );
    assert_ne!(pointer, 0);
    assert_eq!(userlib::sys::fs_base(), Ok(pointer));
    // SAFETY: Reinstall exactly the current process-owned compiler TLS block;
    // object locations/lifetimes stay identical. Invalid pointers must fail.
    assert_eq!(unsafe { userlib::sys::set_fs_base(pointer) }, Ok(()));
    assert!(unsafe { userlib::sys::set_fs_base(u64::MAX) }.is_err());
    assert_eq!(userlib::sys::fs_base(), Ok(pointer));
    exercise(0xaa);
    let mut baseline = None;
    for _ in 0..3 {
        let workers: [_; 8] = core::array::from_fn(|index| {
            userlib::thread::Builder::new()
                .spawn(move || exercise(index as u64 + 1))
                .unwrap()
        });
        for worker in workers {
            worker.join().unwrap();
        }
        // SAFETY: Only this initial thread can access its own TLS instances.
        unsafe {
            assert_eq!(core::ptr::addr_of!(INITIAL).read_volatile(), 0xaa);
        }
        // SAFETY: Diagnostic syscall has only zero scalar arguments. Entry
        // reaps off-CPU thread resources before reporting allocated frames.
        let frames = unsafe { userlib::sys::syscall(107, 0, 0, 0) };
        if let Some(previous) = baseline {
            assert_eq!(frames, previous, "TLS thread pages must be reclaimed");
        } else {
            baseline = Some(frames);
        }
    }
    let value = core::hint::black_box(1.25f64);
    assert_eq!(value * 2.0 + 0.5, 3.0);
    userlib::println!("[captest] PASS");
}
