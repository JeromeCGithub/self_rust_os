// Real Ring 3 descriptor, usercopy, cwd, error and process-lifecycle checks.
mod arguments;
mod capabilities;
mod fpu;
use core::sync::atomic::{AtomicU64, Ordering};
use userlib::{fs::*, println, sys};
fn raw(number: u64, a: u64, b: u64, c: u64) -> u64 {
    // SAFETY: Deliberately malformed scalar/user-pointer arguments exercise
    // kernel validation; the probe never dereferences these invalid pointers.
    unsafe { sys::syscall(number, a, b, c) }
}
fn child(mode: u8) {
    let mut cwd = [0; 256];
    assert_eq!(sys::getcwd(&mut cwd), Ok("/HOME"));
    assert_eq!(
        sys::close(3),
        Err(()),
        "parent descriptors must not leak into child"
    );
    assert_eq!(sys::chdir("/"), Ok(()));
    for _ in 0..16 {
        sys::open("/HOME/IO.TXT", O_RDONLY).unwrap();
    }
    assert_eq!(sys::write_file("/TMP/FILE.RDY", b"1"), Some(1));
    if mode == b'k' {
        loop {
            sys::sleep_ticks(1).unwrap();
        }
    }
    // Leave every descriptor open: process exit/kill must reclaim them.
}
fn main(argc: usize, argv: &[&str]) {
    let mut capability = [0; 16];
    if let Some(n) = sys::read_file("/TMP/CAPS.RUN", &mut capability) {
        capabilities::run(&capability[..n]);
        return;
    }
    if sys::read_file("/TMP/ARGV.RUN", &mut [0]) == Some(1) {
        arguments::run(argc, argv);
        return;
    }
    let mut mode = [0];
    if sys::read_file("/TMP/FILE.MOD", &mut mode) == Some(1) && mode[0] != b'p' {
        child(mode[0]);
        return;
    }
    assert_eq!(sys::chdir("/HOME"), Ok(()));
    assert_eq!(sys::write_file("IO.TXT", b"abcdef"), Some(6));
    let fd = sys::open("./IO.TXT", O_RDWR).unwrap();
    assert_eq!(fd, 3);
    let other = sys::open("IO.TXT", O_RDONLY).unwrap();
    assert_eq!(other, 4);
    let mut buf = [0; 8];
    assert_eq!(sys::read(fd, &mut buf[..2]), 2);
    assert_eq!(&buf[..2], b"ab");
    assert_eq!(sys::read(other, &mut buf[..2]), 2);
    assert_eq!(&buf[..2], b"ab");
    assert_eq!(sys::write(fd, b"XY"), Ok(2));
    assert_eq!(sys::lseek(fd, -2, SEEK_END), Ok(4));
    assert_eq!(sys::read(fd, &mut buf), 2);
    assert_eq!(&buf[..2], b"ef");
    assert_eq!(sys::read(fd, &mut buf), 0);
    assert_eq!(sys::lseek(fd, 8, SEEK_SET), Ok(8));
    assert_eq!(sys::write(fd, b"z"), Ok(1));
    let mut all = [0; 9];
    assert_eq!(sys::read_file("IO.TXT", &mut all), Some(9));
    assert_eq!(&all, b"abXYef\0\0z");
    assert_eq!(sys::lseek(fd, -1, SEEK_SET), Err(()));
    assert_eq!(sys::lseek(fd, i64::MAX, SEEK_CUR), Err(()));
    assert_eq!(sys::lseek(fd, 0, 99), Err(()));
    assert_eq!(sys::lseek(fd, 0, SEEK_CUR), Ok(9));
    assert_eq!(sys::write(other, b"bad"), Err(()));
    let writer = sys::open("IO.TXT", O_WRONLY).unwrap();
    assert_eq!(sys::read(writer, &mut buf) as u64, sys::SYSCALL_ERROR);
    sys::close(writer).unwrap();
    assert_eq!(sys::open("IO.TXT", O_RDONLY | O_TRUNC), Err(()));
    assert_eq!(sys::open("IO.TXT", 3), Err(()));
    assert_eq!(sys::open("IO.TXT", 1 << 63), Err(()));
    assert_eq!(sys::open("bad\0.txt", O_CREAT), Err(()));
    assert_eq!(sys::open("/HOME", O_RDONLY), Err(()));
    assert_eq!(sys::open("/TMP/ABSENT/NEW.TXT", O_CREAT | O_RDWR), Err(()));
    assert_eq!(sys::chdir(""), Err(()));
    assert_eq!(sys::mkdir(""), Err(()));
    assert_eq!(sys::chdir("IO.TXT"), Err(()));
    assert_eq!(sys::chdir("/MISSING/.."), Err(()));
    assert_eq!(sys::chdir("IO.TXT/.."), Err(()));
    assert_eq!(sys::getcwd(&mut buf), Ok("/HOME"));
    assert_eq!(sys::getcwd(&mut buf[..5]), Err(()));
    for pointer in [0, 0x2bfffff, 0x3000000, u64::MAX - 1] {
        assert_eq!(raw(sys::SYS_READ, fd, pointer, 8), sys::SYSCALL_ERROR);
        assert_eq!(raw(sys::SYS_WRITE_FD, fd, pointer, 8), sys::SYSCALL_ERROR);
        assert_eq!(raw(sys::SYS_GETCWD, pointer, 256, 0), sys::SYSCALL_ERROR);
        assert_eq!(raw(sys::SYS_CHDIR, pointer, 0, 0), sys::SYSCALL_ERROR);
        assert_eq!(raw(sys::SYS_OPEN, pointer, 0, 0), sys::SYSCALL_ERROR);
    }
    assert_eq!(
        raw(sys::SYS_READ, fd, b"readonly".as_ptr() as u64, 8),
        sys::SYSCALL_ERROR
    );
    assert_eq!(
        raw(sys::SYS_GETCWD, buf.as_mut_ptr() as u64, u64::MAX, 0),
        sys::SYSCALL_ERROR
    );
    assert_eq!(
        raw(sys::SYS_READ, fd, buf.as_mut_ptr() as u64, u64::MAX),
        sys::SYSCALL_ERROR
    );
    assert_eq!(
        sys::lseek(fd, 0, SEEK_CUR),
        Ok(9),
        "invalid I/O cannot advance offsets"
    );
    assert_eq!(
        sys::ls("/", &mut buf, 1),
        None,
        "SDK prevents undersized listing buffers"
    );
    // Fill table: failed O_TRUNC/O_CREAT must have no disk side effects.
    let mut descriptors = [0; 1022];
    for entry in &mut descriptors {
        *entry = sys::open("IO.TXT", O_RDONLY).unwrap();
    }
    assert_eq!(sys::open("IO.TXT", O_WRONLY | O_TRUNC), Err(()));
    assert_eq!(sys::open("NOFILE", O_RDWR | O_CREAT), Err(()));
    assert_eq!(sys::read_file("IO.TXT", &mut all), Some(9));
    for entry in descriptors {
        sys::close(entry).unwrap();
    }
    assert_eq!(sys::open("NOFILE", O_RDONLY), Err(()));
    sys::close(other).unwrap();
    assert_eq!(sys::close(other), Err(()));
    for _ in 0..40 {
        let temp = sys::open("IO.TXT", O_RDONLY).unwrap();
        assert_eq!(temp, 4);
        sys::close(temp).unwrap();
    }
    // Child cwd/descriptor isolation and leak cleanup on exit and kill.
    for phase in [b'e', b'k', b'e'] {
        sys::write_file("/TMP/FILE.RDY", b"0").unwrap();
        sys::write_file("/TMP/FILE.MOD", &[phase]).unwrap();
        let pid = sys::spawn("/BIN/ZFILEIO.BIN").unwrap();
        let start = sys::ticks();
        loop {
            if sys::read_file("/TMP/FILE.RDY", &mut mode) == Some(1) && mode[0] == b'1' {
                break;
            }
            assert!(sys::ticks() - start < 100, "child readiness deadline");
            sys::sleep_ticks(1).unwrap();
        }
        if phase == b'k' {
            sys::kill(pid).unwrap();
        }
        assert_eq!(sys::waitpid(pid), Ok(if phase == b'k' { 9 } else { 0 }));
        assert_eq!(sys::getcwd(&mut cwd_buffer()), Ok("/HOME"));
        assert_eq!(sys::lseek(fd, 0, SEEK_CUR), Ok(9));
    }
    sys::write_file("/TMP/FILE.MOD", b"p").unwrap();
    sys::close(fd).unwrap();
    // Stream more than the old cat's 1 KiB limit and verify short reads + EOF.
    let contents = [b'L'; 4097];
    assert_eq!(sys::write_file("LONG.TXT", &contents), Some(contents.len()));
    let mut file = File::open("LONG.TXT", O_RDONLY).unwrap();
    let mut chunk = [0; 1024];
    let mut total = 0;
    loop {
        let n = file.read(&mut chunk).unwrap();
        if n == 0 {
            break;
        }
        assert!(chunk[..n].iter().all(|&c| c == b'L'));
        total += n;
    }
    assert_eq!(total, contents.len());
    drop(file);
    let mut file = File::open("EMPTY.TXT", O_RDWR | O_CREAT | O_TRUNC).unwrap();
    assert_eq!(file.read(&mut chunk), Ok(0));
    file.write_all(b"sdk").unwrap();
    file.seek(0, SEEK_SET).unwrap();
    assert_eq!(file.read(&mut chunk), Ok(3));
    assert_eq!(&chunk[..3], b"sdk");
    drop(file);
    concurrent_offsets();
    println!("[filetest] PASS: descriptors, offsets, usercopy, cwd, quotas, exit/kill cleanup, streaming");
    sys::exit(0);
}
fn cwd_buffer() -> [u8; 256] {
    [0; 256]
}

// One descriptor is intentionally shared among eight Ring 3 workers. Each
// four-byte record must be delivered exactly once, across CPU counts 1/8/32.
fn concurrent_offsets() {
    static SEEN: [AtomicU64; 16] = [const { AtomicU64::new(0) }; 16];
    let mut records = [0; 4096];
    for (index, record) in records.chunks_mut(4).enumerate() {
        record.copy_from_slice(&(index as u32).to_le_bytes());
    }
    sys::write_file("RECORDS.BIN", &records).unwrap();
    let descriptor = sys::open("RECORDS.BIN", O_RDONLY).unwrap();
    let workers: [_; 8] = core::array::from_fn(|_| {
        userlib::thread::Builder::new()
            .spawn(move || {
                let mut record = [0; 4];
                loop {
                    match sys::read(descriptor, &mut record) {
                        0 => break,
                        4 => {
                            let index = u32::from_le_bytes(record) as usize;
                            assert!(index < 1024);
                            let bit = 1 << (index % 64);
                            // Each atomic bit independently records delivery. Joining
                            // workers precedes inspection; no multi-field publication.
                            assert_eq!(SEEN[index / 64].fetch_or(bit, Ordering::Relaxed) & bit, 0);
                        }
                        _ => panic!("record read failed"),
                    }
                }
            })
            .unwrap()
    });
    for worker in workers {
        worker.join().unwrap();
    }
    for word in &SEEN {
        assert_eq!(word.load(Ordering::Relaxed), u64::MAX);
    }
    assert_eq!(sys::lseek(descriptor, 0, SEEK_CUR), Ok(4096));
    sys::close(descriptor).unwrap();
}
