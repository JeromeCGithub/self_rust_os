// Focused Ring 3 capability regressions, one subsystem per disposable VM.
extern crate alloc;
use userlib::{fs::*, println, sys};
pub fn run(case: &[u8]) {
    match case {
        b"fds" => descriptors(),
        b"files" => files(),
        b"heap" => heap(),
        b"env" => environment(),
        b"fpu" => crate::fpu::run(),
        b"names" => names(),
        _ => panic!("unknown capability case"),
    }
    println!("[captest] PASS");
}
fn descriptors() {
    sys::write_file("/TMP/FDS.TXT", b"descriptor").unwrap();
    let mut fds = [0; 1024];
    for (index, fd) in fds.iter_mut().enumerate() {
        *fd = sys::open("/TMP/FDS.TXT", O_RDONLY).expect("1024 file descriptors must be available");
        assert_eq!(*fd, index as u64 + 3);
    }
    assert_eq!(sys::open("/TMP/FDS.TXT", O_RDONLY), Err(()));
    for fd in fds {
        sys::close(fd).unwrap();
    }
    assert_eq!(sys::open("/TMP/FDS.TXT", O_RDONLY), Ok(3));
}
fn files() {
    let mut file = File::open("/TMP/BIG.DAT", O_RDWR | O_CREAT | O_TRUNC).unwrap();
    let data = [0x5a; 32768];
    for _ in 0..128 {
        file.write_all(&data).expect("file must grow past 1 MiB");
    }
    assert_eq!(file.seek(0, SEEK_END), Ok(4 * 1024 * 1024));
    file.seek(2 * 1024 * 1024 - 3, SEEK_SET).unwrap();
    file.write_all(b"patch across sector").unwrap();
    file.seek(2 * 1024 * 1024 - 4, SEEK_SET).unwrap();
    let mut check = [0; 21];
    assert_eq!(file.read(&mut check), Ok(21));
    assert_eq!(&check, b"\x5apatch across sector\x5a");
    assert_eq!(check[20], 0x5a);
    file.seek(4 * 1024 * 1024 + 700, SEEK_SET).unwrap();
    file.write_all(b"end").unwrap();
    file.seek(4 * 1024 * 1024, SEEK_SET).unwrap();
    let mut gap = [1; 703];
    assert_eq!(file.read(&mut gap), Ok(703));
    assert!(gap[..700].iter().all(|&b| b == 0));
    assert_eq!(&gap[700..], b"end");
}

fn heap() {
    for _ in 0..3 {
        let mut bytes = alloc::vec::Vec::new();
        bytes
            .try_reserve_exact(32 * 1024 * 1024)
            .expect("SDK heap must grow beyond fixed 1/16 MiB storage");
        bytes.resize(32 * 1024 * 1024, 0x5a);
        assert_eq!(bytes[0], 0x5a);
        assert_eq!(bytes[bytes.len() - 1], 0x5a);
    }
    // Failed physical allocation must roll back every partial mapping/frame.
    // SAFETY: Scalar diagnostic/query syscalls retain no user references.
    let before = unsafe { sys::syscall(107, 0, 0, 0) };
    let old = unsafe { sys::brk(0) };
    let mut excessive = alloc::vec::Vec::<u8>::new();
    assert!(excessive.try_reserve_exact(2 * 1024 * 1024 * 1024).is_err());
    assert_eq!(unsafe { sys::brk(0) }, old);
    // SAFETY: Deliberately unmapped output is used only to test kernel checks.
    assert_eq!(
        unsafe { sys::syscall(28, (old + 4096) as u64, 16, 0) },
        sys::SYSCALL_ERROR
    );
    assert_eq!(
        unsafe { sys::syscall(107, 0, 0, 0) },
        before,
        "failed heap growth leaked frames"
    );
}

fn environment() {
    if let Some(mode) = userlib::args::arg(1) {
        if mode == "--env-child" || mode == "--inherit-env" {
            let expected = if mode == "--env-child" {
                "hello world"
            } else {
                "parent value"
            };
            assert_eq!(userlib::env::var("NORMAL_ENV").as_deref(), Some(expected));
            userlib::env::set_var("NORMAL_ENV", "child only").unwrap();
            sys::exit(42);
        }
        if mode == "--large-env" {
            let value = userlib::env::var("LARGE").unwrap();
            assert_eq!(value.len(), 16000);
            assert!(value.bytes().all(|b| b == b'x'));
            sys::exit(44);
        }
    }
    let mut path = [0; 256];
    let executable = b"/BIN/ZFILEIO.BIN";
    path[..executable.len()].copy_from_slice(executable);
    let child = b"--env-child";
    let variable = b"NORMAL_ENV=hello world";
    let argv = [
        [executable.as_ptr() as u64, executable.len() as u64],
        [child.as_ptr() as u64, child.len() as u64],
    ];
    let env = [[variable.as_ptr() as u64, variable.len() as u64]];
    let request = [1u64, 0, argv.as_ptr() as u64, 2, env.as_ptr() as u64, 1];
    // SAFETY: Complete fixed records point at readable immutable string data;
    // every borrowed array remains live through this synchronous syscall.
    let pid = unsafe { sys::syscall(30, path.as_ptr() as u64, request.as_ptr() as u64, 48) };
    assert_ne!(
        pid,
        sys::SYSCALL_ERROR,
        "general argv/envp spawn must be available"
    );
    assert_eq!(sys::waitpid(pid as u32), Ok(42));
    assert_eq!(userlib::env::var("NORMAL_ENV"), None);
    userlib::env::set_var("NORMAL_ENV", "parent value").unwrap();
    let pid = sys::spawn_with_args("/BIN/ZFILEIO.BIN", &["--inherit-env"]).unwrap();
    assert_eq!(sys::waitpid(pid), Ok(42));
    assert_eq!(
        userlib::env::var("NORMAL_ENV").as_deref(),
        Some("parent value")
    );
    let pid = sys::spawn_with_env(
        "/BIN/ZFILEIO.BIN",
        &["--env-child"],
        &["NORMAL_ENV=hello world"],
    )
    .unwrap();
    assert_eq!(sys::waitpid(pid), Ok(42));
    let large = alloc::format!("LARGE={}", "x".repeat(16000));
    let pid = sys::spawn_with_env("/BIN/ZFILEIO.BIN", &["--large-env"], &[&large]).unwrap();
    assert_eq!(sys::waitpid(pid), Ok(44));
    userlib::env::remove_var("NORMAL_ENV");
    assert_eq!(userlib::env::var("NORMAL_ENV"), None);
    assert!(userlib::env::set_var("bad=key", "v").is_err());
    assert!(userlib::env::set_var("key", "bad\0value").is_err());
    let bad = [1u64, 0, 0, u64::MAX, 0, 0];
    // SAFETY: Fixed request is readable; excessive count must fail before copying any vector.
    assert_eq!(
        unsafe { sys::syscall(30, path.as_ptr() as u64, bad.as_ptr() as u64, 48) },
        sys::SYSCALL_ERROR
    );
    for pointer in [0, u64::MAX - 8, 0x8000000000] {
        let malformed = [1u64, 0, argv.as_ptr() as u64, 2, pointer, 1];
        // SAFETY: Malformed scalar pointers test rejection only. Valid request
        // and argv storage remain readable; no user reference escapes the call.
        unsafe {
            assert_eq!(
                sys::syscall(30, path.as_ptr() as u64, pointer, 48),
                sys::SYSCALL_ERROR
            );
            assert_eq!(
                sys::syscall(30, path.as_ptr() as u64, malformed.as_ptr() as u64, 48),
                sys::SYSCALL_ERROR
            );
        }
    }
    // SAFETY: A complete readable request with the wrong wire size is rejected.
    assert_eq!(
        unsafe { sys::syscall(30, path.as_ptr() as u64, request.as_ptr() as u64, 47) },
        sys::SYSCALL_ERROR
    );
}

fn names() {
    let mut host = File::open("/TMP/Host created Unicode café name.txt", O_RDONLY).unwrap();
    let mut host_data = [0; 16];
    assert_eq!(host.read(&mut host_data), Ok(16));
    assert_eq!(&host_data, b"host LFN fixture");
    drop(host);
    let directory = "/TMP/Directory with a long name";
    sys::mkdir(directory).expect("long directory names must be accepted");
    let path = "/TMP/Directory with a long name/café 文書 and notes.txt";
    let mut file =
        File::open(path, O_RDWR | O_CREAT | O_TRUNC).expect("long Unicode filename must open");
    file.write_all(b"long file data").unwrap();
    file.seek(0, SEEK_SET).unwrap();
    let mut bytes = [0; 14];
    assert_eq!(file.read(&mut bytes), Ok(14));
    assert_eq!(&bytes, b"long file data");
    drop(file);
    let entries = userlib::fs::read_dir(directory).unwrap();
    assert!(entries
        .iter()
        .any(|entry| entry.name == "café 文書 and notes.txt"));
    let maximum = "x".repeat(255);
    let path = alloc::format!("{}/{}", directory, maximum);
    let mut file = File::open(&path, O_RDWR | O_CREAT).unwrap();
    file.write_all(b"255 UTF-16 units").unwrap();
    drop(file);
    for index in 0..40 {
        let path = alloc::format!(
            "{}/a repeated long prefix filename {}.txt",
            directory,
            index
        );
        let mut file = File::open(&path, O_RDWR | O_CREAT).unwrap();
        file.write_all(&[index]).unwrap();
    }
    let entries = userlib::fs::read_dir(directory).unwrap();
    assert_eq!(entries.len(), 42);
    let path = alloc::format!("{}\0", directory);
    let mut sentinel = [0x5a; 32];
    // SAFETY: The live NUL path and complete writable sentinel are valid. Other
    // scalar ranges intentionally overflow or are unmapped. Every failure must
    // reject before any partial listing is copied into the destination.
    unsafe {
        let required = sys::syscall(33, path.as_ptr() as u64, 0, 0);
        assert!(required > sentinel.len() as u64 && required != sys::SYSCALL_ERROR);
        assert_eq!(
            sys::syscall(
                33,
                path.as_ptr() as u64,
                sentinel.as_mut_ptr() as u64,
                sentinel.len() as u64
            ),
            sys::SYSCALL_ERROR
        );
        assert_eq!(
            sys::syscall(33, path.as_ptr() as u64, 0, required),
            sys::SYSCALL_ERROR
        );
        assert_eq!(
            sys::syscall(
                33,
                path.as_ptr() as u64,
                sentinel.as_mut_ptr() as u64,
                u64::MAX
            ),
            sys::SYSCALL_ERROR
        );
    }
    assert_eq!(sentinel, [0x5a; 32]);
}
