// Focused real Ring 3 argument ABI and runtime regression probe.
use userlib::{println, sys};
const PATH: &str = "/BIN/ZFILEIO.BIN";
const SIZE: usize = 2048;
fn raw(path: u64, request: u64, flags: u64) -> u64 {
    // SAFETY: Valid requests use complete borrowed fixed-size records. Other
    // cases deliberately supply invalid scalar pointers to test validation;
    // the probe itself never dereferences those malformed addresses.
    unsafe { sys::syscall(29, path, request, flags) }
}
fn ready() {
    let start = sys::ticks();
    while sys::read_file("/TMP/ARGV.GO", &mut [0]) != Some(1) {
        assert!(sys::ticks() - start < 100);
        sys::sleep_ticks(1).unwrap();
    }
}
pub fn run(count: usize, argv: &[&str]) {
    assert_eq!(count, argv.len());
    assert_eq!(userlib::argc(), count);
    assert_eq!(userlib::args().len(), count);
    for (expected, actual) in argv.iter().zip(userlib::args()) {
        assert_eq!(*expected, actual);
    }
    // SAFETY: Runtime publishes the kernel-created argc+1 pointer table in this
    // process's mapped initial stack; all strings are immutable and bounded.
    unsafe {
        let pointers = userlib::argv();
        assert!(pointers.add(count).read().is_null());
        for (index, string) in argv.iter().enumerate() {
            let pointer = pointers.add(index).read();
            assert_eq!(
                core::slice::from_raw_parts(pointer, string.len()),
                string.as_bytes()
            );
            assert_eq!(pointer.add(string.len()).read(), 0);
        }
    }
    if argv.first() == Some(&"argv-child") {
        assert_eq!(argv, &["argv-child", "alpha", "", "two words"]);
        sys::exit(37);
    }
    if argv.get(1) == Some(&"--child") {
        assert_eq!(
            argv,
            &[PATH, "--child", "typed", "", "quoted argument", "café"]
        );
        let worker = userlib::thread::Builder::new()
            .spawn(|| {
                assert_eq!(userlib::argc(), 6);
                assert_eq!(userlib::args::arg(5), Some("café"));
            })
            .unwrap();
        worker.join().unwrap();
        sys::exit(38);
    }
    if argv.get(1) == Some(&"--connected") {
        assert_eq!(argv, &[PATH, "--connected", "two words", ""]);
        println!("connected argument received");
        sys::exit(42);
    }
    if argv.get(1) == Some(&"--max") {
        assert_eq!(count, 4096);
        assert!(argv[2..].iter().all(|value| value.is_empty()));
        sys::exit(39);
    }
    if count == 1 {
        if argv[0].len() == 2031 {
            ready();
            assert!(
                argv[0].bytes().all(|b| b == b'a'),
                "parent mutation must not reach child"
            );
            sys::exit(17);
        }
        assert_eq!(argv[0], PATH);
        sys::exit(19);
    }
    assert_eq!(count, 0, "direct initial boot has empty argv");
    let mut path = [0; 256];
    path[..PATH.len()].copy_from_slice(PATH.as_bytes());
    let mut request = [0; SIZE];
    request[..8].copy_from_slice(&1u64.to_le_bytes());
    request[8..16].copy_from_slice(&4u64.to_le_bytes());
    let data = b"argv-child\0alpha\0\0two words\0";
    request[16..16 + data.len()].copy_from_slice(data);
    let pid = raw(path.as_ptr() as u64, request.as_ptr() as u64, 0);
    assert_ne!(
        pid,
        sys::SYSCALL_ERROR,
        "argument-bearing spawn must be supported"
    );
    assert_eq!(sys::waitpid(pid as u32), Ok(37));
    let pid =
        sys::spawn_with_args(PATH, &["--child", "typed", "", "quoted argument", "café"]).unwrap();
    assert_eq!(sys::waitpid(pid), Ok(38));
    let pid = sys::spawn(PATH).unwrap();
    assert_eq!(sys::waitpid(pid), Ok(19));
    let mut maximum = [""; 4095];
    maximum[0] = "--max";
    let pid = sys::spawn_with_args(PATH, &maximum).unwrap();
    assert_eq!(sys::waitpid(pid), Ok(39));
    assert_eq!(sys::spawn_with_args(PATH, &[""; 4096]), Err(()));
    assert_eq!(sys::spawn_with_args(PATH, &["bad\0argument"]), Err(()));
    let oversized = [b'x'; 128 * 1024];
    assert_eq!(
        sys::spawn_with_args(PATH, &[core::str::from_utf8(&oversized).unwrap()]),
        Err(())
    );
    for pointer in [0, 0x2bfffff, 0x3000000, u64::MAX - 1] {
        assert_eq!(raw(path.as_ptr() as u64, pointer, 0), sys::SYSCALL_ERROR);
        assert_eq!(raw(pointer, request.as_ptr() as u64, 0), sys::SYSCALL_ERROR);
    }
    assert_eq!(
        raw(path.as_ptr() as u64, request.as_ptr() as u64, 2),
        sys::SYSCALL_ERROR
    );
    for (version, argc) in [(0u64, 1), (2, 1), (1, 33), (1, u64::MAX)] {
        request[..8].copy_from_slice(&version.to_le_bytes());
        request[8..16].copy_from_slice(&argc.to_le_bytes());
        assert_eq!(
            raw(path.as_ptr() as u64, request.as_ptr() as u64, 0),
            sys::SYSCALL_ERROR
        );
    }
    request[..8].copy_from_slice(&1u64.to_le_bytes());
    request[8..16].copy_from_slice(&1u64.to_le_bytes());
    request[16..].fill(b'a');
    assert_eq!(
        raw(path.as_ptr() as u64, request.as_ptr() as u64, 0),
        sys::SYSCALL_ERROR
    );
    request[16] = 255;
    request[17] = 0;
    assert_eq!(
        raw(path.as_ptr() as u64, request.as_ptr() as u64, 0),
        sys::SYSCALL_ERROR
    );
    // Exact combined-byte boundary, then modify and release the source record.
    request[16..].fill(b'a');
    request[SIZE - 1] = 0;
    let pid = raw(path.as_ptr() as u64, request.as_ptr() as u64, 0);
    assert_ne!(pid, sys::SYSCALL_ERROR);
    request.fill(0);
    sys::write_file("/TMP/ARGV.GO", b"1").unwrap();
    assert_eq!(sys::waitpid(pid as u32), Ok(17));
    let mut child =
        userlib::process::Child::spawn_with_args(PATH, &["--connected", "two words", ""]).unwrap();
    let mut output = [0; 64];
    let mut length = 0;
    let start = sys::ticks();
    loop {
        match child.read() {
            Ok(Some((bytes, count))) => {
                assert!(length + count <= output.len());
                output[length..length + count].copy_from_slice(&bytes[..count]);
                length += count;
            }
            Ok(None) => {
                assert!(sys::ticks() - start < 100);
                sys::sleep_ticks(1).unwrap();
            }
            Err(_) => break,
        }
    }
    assert_eq!(&output[..length], b"connected argument received\n");
    assert_eq!(sys::waitpid(child.pid()), Ok(42));
    drop(child);
    // Repeat creation/reaping after all failures to expose stale construction state.
    for _ in 0..4 {
        let pid = sys::spawn_with_args(PATH, &["--child", "typed", "", "quoted argument", "café"])
            .unwrap();
        assert_eq!(sys::waitpid(pid), Ok(38));
    }
    println!("[argtest] PASS");
}
