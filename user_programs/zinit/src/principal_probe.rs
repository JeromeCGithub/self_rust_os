//! Test-only native principal data, ordinary spawn paths and complete usercopy.
use userlib::{
    principal::{self, abi, Capabilities as Caps},
    recordln, supervision, sys,
};
fn raw(pointer: u64, size: u64, reserved: u64) -> u64 {
    // SAFETY: Only scalars cross the ABI. Valid pointers refer to the complete
    // live test buffers; invalid pointers must be rejected without any access.
    unsafe { sys::syscall(abi::SYS_PRINCIPAL, pointer, size, reserved) }
}
pub fn child(descend: bool) {
    let principal = principal::current().expect("child kernel principal");
    assert_eq!(
        (
            principal.user,
            principal.session,
            principal.scope,
            principal.flags
        ),
        (0, 0, 1, 0)
    );
    assert_eq!(principal.capabilities, Caps::ALL.ordinary_child());
    let queried = userlib::thread::Builder::new()
        .spawn(principal::current)
        .unwrap()
        .join()
        .unwrap()
        .unwrap();
    assert_eq!(queried, principal, "threads share process identity");
    if descend {
        let pid = sys::spawn_with_args("/BIN/ZINIT.SVC", &["--principal-grandchild"]).unwrap();
        assert_eq!(sys::waitpid(pid), Ok(0));
    }
}
pub fn run(pid: u32) -> ! {
    if pid != 1 {
        child(true);
        sys::exit(0);
    }
    let principal = principal::current().expect("PID 1 kernel principal must exist");
    assert_eq!(
        (
            principal.user,
            principal.session,
            principal.scope,
            principal.flags
        ),
        (0, 0, 1, abi::INIT)
    );
    assert_eq!(principal.capabilities, Caps::ALL);
    userlib::env::set_var("USER", "999").unwrap();
    userlib::env::set_var("SESSION", "999").unwrap();
    assert_eq!(
        principal::current(),
        Ok(principal),
        "environment is not identity"
    );
    static READONLY: [u8; 40] = [0xa5; 40];
    for (pointer, size, reserved) in [
        (0, 40, 0),
        (READONLY.as_ptr() as u64, 40, 0),
        (0x60000000, 40, 0),
        (0x3fffffffc, 40, 0),
        (u64::MAX - 10, 40, 0),
        (0xffff800000000000, 40, 0),
        (0, 0, 0),
        (0, 39, 0),
        (0, 41, 0),
        (0, 40, 1),
    ] {
        assert_eq!(raw(pointer, size, reserved), sys::SYSCALL_ERROR);
    }
    let mut storage = [0xa5; 8192];
    let base = storage.as_mut_ptr() as usize;
    let address = ((base + 4095) & !4095) + 4090;
    let offset = address - base;
    assert!(offset > 0 && offset + 40 < storage.len());
    assert_eq!(raw(address as u64, 40, 0), 0);
    assert_eq!(
        abi::Principal::decode(&storage[offset..offset + 40]),
        Ok(principal)
    );
    assert_eq!(storage[offset - 1], 0xa5);
    assert_eq!(storage[offset + 40], 0xa5);
    // Overwrite fake caller-supplied metadata; the operation has no input principal.
    storage[offset..offset + 40].fill(0xff);
    assert_eq!(raw(address as u64, 40, 0), 0);
    assert_eq!(
        abi::Principal::decode(&storage[offset..offset + 40]),
        Ok(principal)
    );
    let pid = sys::spawn("/BIN/ZINIT.SVC").unwrap();
    assert_eq!(sys::waitpid(pid), Ok(0));
    let pid = sys::spawn_with_args("/BIN/ZINIT.SVC", &["--principal-child"]).unwrap();
    assert_eq!(sys::waitpid(pid), Ok(0));
    let pid = sys::spawn_with_env(
        "/BIN/ZINIT.SVC",
        &["--principal-child"],
        &["USER=999", "SESSION=999"],
    )
    .unwrap();
    assert_eq!(sys::waitpid(pid), Ok(0));
    let pid = userlib::ipc::spawn("/BIN/ZINIT.SVC", &["--principal-child"], &[], &mut []).unwrap();
    assert_eq!(sys::waitpid(pid), Ok(0));
    let mut file = userlib::fs::File::open("/BIN/ZINIT.SVC", userlib::fs::O_RDONLY).unwrap();
    let length = file.seek(0, userlib::fs::SEEK_END).unwrap() as usize;
    assert!(length < 1024 * 1024);
    file.seek(0, userlib::fs::SEEK_SET).unwrap();
    let mut image = alloc::vec![0;length];
    let mut read = 0;
    while read < length {
        let end = (read + 64 * 1024).min(length);
        let n = file.read(&mut image[read..end]).unwrap();
        assert!(n > 0);
        read += n;
    }
    let pid = userlib::ipc::spawn_image(
        "/BIN/ZINIT.SVC",
        &image,
        &["--principal-child"],
        &[],
        &mut [],
    )
    .unwrap();
    assert_eq!(sys::waitpid(pid), Ok(0));
    assert!(supervision::next_child().unwrap().is_none());
    recordln!("[init-probe] principal PASS spawn_paths=5 threads=true canonical=true");
    loop {
        sys::wait_events(sys::Events::CHILD, 0).unwrap();
    }
}
