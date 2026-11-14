//! Test-only assertions of the kernel's ambient mechanism boundary.
use userlib::{
    fs, ipc,
    principal::{self, Capabilities as Caps},
    recordln, sys,
};
fn raw(n: u64, a: u64, b: u64, c: u64) -> u64 {
    // SAFETY: Scalars or complete live buffers cross these synchronous calls.
    // Deliberate invalid scalars/pointers must be rejected before any access.
    unsafe { sys::syscall(n, a, b, c) }
}
fn no_files() {
    let mut bytes = [0; 16];
    assert!(sys::read_file("/CAP.TXT", &mut bytes).is_none());
    assert!(fs::File::open("/CAP.TXT", fs::O_RDONLY).is_err());
    assert!(fs::read_dir("/").is_err());
}
fn no_spawn(image: &[u8]) {
    assert!(sys::spawn("/BIN/ZINIT.SVC").is_err());
    assert!(sys::spawn_with_args("/BIN/ZINIT.SVC", &["--cap-unexpected"]).is_err());
    assert!(sys::spawn_with_env("/BIN/ZINIT.SVC", &["--cap-unexpected"], &[]).is_err());
    assert!(ipc::spawn("/BIN/ZINIT.SVC", &["--cap-unexpected"], &[], &mut []).is_err());
    assert!(ipc::spawn_image("zinit", image, &["--cap-unexpected"], &[], &mut []).is_err());
    // Exercise the old numeric path-spawn as well as the SDK's newer v1/v2/v3 paths.
    let path = b"/BIN/ZINIT.SVC\0";
    assert_eq!(raw(8, path.as_ptr() as u64, 0, 0), sys::SYSCALL_ERROR);
}
pub fn child(mode: &str) {
    if mode == "--cap-unexpected" {
        panic!("denied spawn published a process");
    }
    if mode == "--cap-worker" {
        userlib::process::restrict_worker(1024 * 1024).unwrap();
        assert_eq!(raw(130, 0, 0, 0), sys::SYSCALL_ERROR);
        assert_eq!(raw(129, 0, 0, 0), sys::SYSCALL_ERROR);
        return;
    }
    if mode == "--cap-inherited" {
        assert_eq!(principal::current().unwrap().capabilities, Caps::SPAWN);
        no_files();
        principal::attenuate(Caps::NONE).unwrap();
        assert!(sys::spawn("/BIN/ZINIT.SVC").is_err());
        return;
    }
    if mode == "--cap-inherit" {
        principal::attenuate(Caps::SPAWN).expect("drop before spawning a child");
        let pid = sys::spawn_with_args("/BIN/ZINIT.SVC", &["--cap-inherited"]).unwrap();
        assert_eq!(sys::waitpid(pid), Ok(0));
        return;
    }
    if mode == "--cap-read" {
        principal::attenuate(Caps::FILE_READ).unwrap();
        let mut file = fs::File::open("/CAP.TXT", fs::O_RDONLY).unwrap();
        let mut bytes = [0; 4];
        assert_eq!(file.read(&mut bytes), Ok(4));
        assert_eq!(&bytes, b"keep");
        // Read-only create must not gain a write side effect through access mode 0.
        assert!(fs::File::open("/HIDDEN.TXT", fs::O_RDONLY | fs::O_CREAT).is_err());
        assert!(sys::write_file("/CAP.TXT", b"bad!").is_none());
        assert!(sys::mkdir("/CAPDIR").is_err());
        assert!(sys::spawn("/BIN/ZINIT.SVC").is_err());
        return;
    }
    if mode == "--cap-write" {
        principal::attenuate(Caps::FILE_WRITE).unwrap();
        no_files();
        assert!(fs::File::open("/CAP.TXT", fs::O_RDWR).is_err());
        let mut file = fs::File::open("/CAP.TXT", fs::O_WRONLY).unwrap();
        assert_eq!(file.write(b"okay"), Ok(4));
        return;
    }
    assert_eq!(mode, "--cap-none");
    let initial = principal::current().unwrap();
    let fd = sys::open("/CAP.TXT", fs::O_RDWR).unwrap();
    let [mut a, b] = ipc::Endpoint::pair().unwrap();
    let mut file = fs::File::open("/BIN/ZINIT.SVC", fs::O_RDONLY).unwrap();
    let length = file.seek(0, fs::SEEK_END).unwrap() as usize;
    assert!(length < 1024 * 1024);
    file.seek(0, fs::SEEK_SET).unwrap();
    let mut image = alloc::vec![0;length];
    let mut offset = 0;
    while offset < length {
        let end = (offset + 64 * 1024).min(length);
        let count = file.read(&mut image[offset..end]).unwrap();
        assert!(count > 0);
        offset += count;
    }
    drop(file);
    for (bits, flags, reserved) in [(512, 0, 0), (u64::MAX, 0, 0), (0, 1, 0), (0, 0, 1)] {
        assert_eq!(raw(130, bits, flags, reserved), sys::SYSCALL_ERROR);
        assert_eq!(principal::current().unwrap(), initial);
    }
    use core::sync::atomic::{AtomicBool, Ordering};
    let done = userlib::sync::Arc::new(AtomicBool::new(false));
    let other = done.clone();
    let thread = userlib::thread::spawn(move || {
        while !other.load(Ordering::Acquire) {
            userlib::thread::yield_now();
        }
    })
    .unwrap();
    assert!(
        principal::attenuate(Caps::NONE).is_err(),
        "cannot race an in-flight sibling syscall"
    );
    assert_eq!(principal::current().unwrap(), initial);
    done.store(true, Ordering::Release);
    thread.join().unwrap();
    principal::attenuate(Caps::NONE).expect("single-threaded irreversible capability reduction");
    assert_eq!(principal::current().unwrap().capabilities, Caps::NONE);
    principal::attenuate(Caps::NONE).unwrap();
    assert!(principal::attenuate(initial.capabilities).is_err());
    no_files();
    no_spawn(&image);
    let mut bytes = [0; 4];
    assert_eq!(sys::read(fd, &mut bytes) as u64, sys::SYSCALL_ERROR);
    assert_eq!(sys::read_nb(fd, &mut bytes) as u64, sys::SYSCALL_ERROR);
    assert!(sys::write(fd, b"bad!").is_err());
    assert!(sys::write_file("/CAP.TXT", b"bad!").is_none());
    assert!(sys::write_file("/HIDDEN.TXT", b"bad!").is_none());
    assert!(fs::File::open("/HIDDEN.TXT", fs::O_CREAT).is_err());
    assert!(sys::mkdir("/CAPDIR").is_err());
    assert!(userlib::thread::spawn(|| 42).is_err());
    assert!(ipc::Endpoint::pair().is_err());
    for number in [64, 65, 66, 67, 69, 70, 82, 83] {
        assert_eq!(raw(number, 0, 0, 0), sys::SYSCALL_ERROR, "domain {number}");
    }
    assert_eq!(raw(84, 0, 2, 0), sys::SYSCALL_ERROR);
    assert!(matches!(
        userlib::system::system_action(userlib::system::SystemAction::Halt),
        Err(userlib::system::Error::Denied)
    ));
    // Explicit object ownership survives dropping factory authority.
    b.send(b"owned").unwrap();
    let event = ipc::next().unwrap().unwrap();
    assert_eq!(event.endpoint, a.raw().unwrap());
    assert_eq!(&event.payload[..event.length], b"owned");
    a.set_readable(false).unwrap();
    drop(b);
    drop(a);
    assert_eq!(sys::lseek(fd, 0, fs::SEEK_SET), Ok(0));
    sys::close(fd).unwrap();
    sys::sleep_ticks(1).unwrap();
    assert!(!sys::wait_events(sys::Events::CHILD, 1)
        .unwrap()
        .contains(sys::Events::CHILD));
}
pub fn run() -> ! {
    let init = principal::current().unwrap();
    assert!(
        principal::attenuate(Caps::NONE).is_err(),
        "native init must retain recovery authority"
    );
    assert_eq!(principal::current().unwrap(), init);
    assert_eq!(sys::write_file("/CAP.TXT", b"keep"), Some(4));
    for mode in ["--cap-none", "--cap-read", "--cap-inherit", "--cap-write"] {
        let pid = sys::spawn_with_args("/BIN/ZINIT.SVC", &[mode]).unwrap();
        assert_eq!(sys::waitpid(pid), Ok(0), "child mode {mode}");
    }
    let worker =
        userlib::process::Child::spawn_with_args("/BIN/ZINIT.SVC", &["--cap-worker"]).unwrap();
    assert_eq!(sys::waitpid(worker.pid()), Ok(0));
    drop(worker);
    let mut bytes = [0; 16];
    assert_eq!(sys::read_file("/CAP.TXT", &mut bytes), Some(4));
    assert_eq!(&bytes[..4], b"okay");
    assert!(fs::File::open("/HIDDEN.TXT", fs::O_RDONLY).is_err());
    assert!(fs::read_dir("/CAPDIR").is_err());
    assert!(userlib::supervision::next_child().unwrap().is_none());
    recordln!("[init-probe] capabilities PASS denied=true inherited=true cleanup=true");
    loop {
        sys::wait_events(sys::Events::CHILD, 0).unwrap();
    }
}
