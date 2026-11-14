//! Native system-file mutation denial, against an explicitly disposable image.
use userlib::{
    fs,
    principal::{self, Capabilities as Caps},
    recordln, sys,
};
fn assert_denied(path: &str) {
    assert!(
        sys::write_file(path, b"bad!").is_none(),
        "old path write: {path}"
    );
    for flags in [
        fs::O_WRONLY,
        fs::O_RDWR,
        fs::O_WRONLY | fs::O_TRUNC,
        fs::O_RDONLY | fs::O_CREAT,
        fs::O_WRONLY | fs::O_CREAT,
    ] {
        assert!(
            fs::File::open(path, flags).is_err(),
            "protected open {path} flags={flags:o}"
        );
    }
}
pub fn child(alias: &str) {
    let principal = principal::current().unwrap();
    assert!(!principal.capabilities.contains(Caps::SYSTEM_WRITE));
    assert!(principal.capabilities.contains(Caps::FILE_WRITE));
    assert!(
        principal::attenuate(Caps::ALL).is_err(),
        "cannot mint system-write rights"
    );
    assert_eq!(principal::current().unwrap(), principal);
    for path in [
        "/ETC/SYSLOCK.TXT",
        "/etc/syslock.txt",
        "/ETC/../ETC/SYSLOCK.TXT",
        "/BIN/ZINIT.SVC",
        "/kernel.elf",
        alias,
    ] {
        assert_denied(path);
    }
    sys::chdir("/ETC").unwrap();
    assert_denied("SYSLOCK.TXT");
    assert_denied("../BIN/ZINIT.SVC");
    assert!(sys::mkdir("NEWDIR/deeper").is_err());
    assert!(sys::write_file("MISSING.TXT", b"bad!").is_none());
    assert!(fs::File::open("MISSING.TXT", fs::O_WRONLY | fs::O_CREAT).is_err());
    sys::chdir("/").unwrap();
    let mut buffer = [0; 4];
    assert_eq!(sys::read_file("/ETC/SYSLOCK.TXT", &mut buffer), Some(4));
    assert_eq!(&buffer, b"keep");
    // A denied writable open must release its reserved descriptor, repeatedly.
    for _ in 0..32 {
        assert!(sys::open("/ETC/SYSLOCK.TXT", fs::O_WRONLY).is_err());
    }
    let fd = sys::open("/PUBLIC.TXT", fs::O_WRONLY | fs::O_CREAT | fs::O_TRUNC).unwrap();
    assert_eq!(
        fd, 3,
        "failed protected opens leaked descriptor reservations"
    );
    assert_eq!(sys::write(fd, b"okay"), Ok(4));
    sys::close(fd).unwrap();
    // Component boundaries matter: an ordinary sibling prefix stays writable.
    sys::mkdir("/ETCETERA").unwrap();
    assert_eq!(sys::write_file("/ETCETERA/OK.TXT", b"okay"), Some(4));
}
pub fn run() -> ! {
    assert!(principal::current()
        .unwrap()
        .capabilities
        .contains(Caps::SYSTEM_WRITE));
    sys::mkdir("/ETC/Protected Long Directory").unwrap();
    assert_eq!(sys::write_file("/ETC/SYSLOCK.TXT", b"keep"), Some(4));
    assert_eq!(
        sys::write_file("/ETC/Protected Long Directory/SYSLOCK.TXT", b"keep"),
        Some(4)
    );
    let mut records = [0; 12 * 64];
    let count = sys::ls("/ETC", &mut records, 64).unwrap();
    let short = records[..count * 12]
        .chunks_exact(12)
        .find(|entry| &entry[..6] == b"PROTEC" && entry[11] & 0x10 != 0)
        .unwrap();
    let short = core::str::from_utf8(&short[..8]).unwrap().trim_end();
    let alias = alloc::format!("/ETC/{short}/SYSLOCK.TXT");
    assert_ne!(short, "Protected Long Directory");
    let pid = sys::spawn_with_args("/BIN/ZINIT.SVC", &["--system-files-child", &alias]).unwrap();
    assert_eq!(sys::waitpid(pid), Ok(0));
    for path in ["/ETC/SYSLOCK.TXT", &alias] {
        let mut buffer = [0; 16];
        assert_eq!(sys::read_file(path, &mut buffer), Some(4));
        assert_eq!(&buffer[..4], b"keep");
    }
    assert!(fs::File::open("/ETC/MISSING.TXT", fs::O_RDONLY).is_err());
    assert!(fs::read_dir("/ETC/NEWDIR").is_err());
    assert_eq!(
        fs::File::open("/PUBLIC.TXT", fs::O_RDONLY)
            .unwrap()
            .seek(0, fs::SEEK_END),
        Ok(4)
    );
    // PID 1 retains deliberate write authority for recovery/configuration.
    let mut file = fs::File::open("/ETC/SYSLOCK.TXT", fs::O_WRONLY | fs::O_TRUNC).unwrap();
    assert_eq!(file.write(b"root"), Ok(4));
    drop(file);
    recordln!("[init-probe] system files PASS paths=true aliases=true rollback=true");
    loop {
        sys::wait_events(sys::Events::CHILD, 0).unwrap();
    }
}
