// Large file plus large BSS: four private instances exercise storage and frames.
static PAYLOAD: [u8; 8 * 1024 * 1024] = [0xa5; 8 * 1024 * 1024];
static mut MEMORY: [u8; 128 * 1024 * 1024] = [0; 128 * 1024 * 1024];
fn main() {
    // SAFETY: Both static arrays live for the entire process. The offsets are
    // their in-bounds first/last bytes (alignment one); read-only PAYLOAD is
    // never written. This single-threaded fixture alone accesses MEMORY through
    // raw pointers, so the BSS writes do not alias references or race a thread.
    unsafe {
        let payload = core::ptr::addr_of!(PAYLOAD).cast::<u8>();
        assert_eq!(payload.read_volatile(), 0xa5);
        assert_eq!(payload.add(PAYLOAD.len() - 1).read_volatile(), 0xa5);
        let memory = core::ptr::addr_of_mut!(MEMORY).cast::<u8>();
        assert_eq!(memory.read_volatile(), 0);
        assert_eq!(memory.add(128 * 1024 * 1024 - 1).read_volatile(), 0);
        memory.write_volatile(17);
        memory.add(128 * 1024 * 1024 - 1).write_volatile(29);
    }
    userlib::recordln!("[load-probe] verified file=8388608 bss=134217728");
    loop {
        userlib::sys::sleep_ticks(100).expect("pressure hold");
    }
}
