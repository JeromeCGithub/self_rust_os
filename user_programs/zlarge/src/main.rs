// Compact ELF with 64 MiB BSS, beyond the historical 40 MiB image arena.
static mut LARGE: [u8; 64 * 1024 * 1024] = [0; 64 * 1024 * 1024];
fn main() {
    // SAFETY: This single-threaded probe alone owns its private BSS. Raw
    // pointers avoid references to mutable statics; both endpoints are in bounds.
    unsafe {
        let data = core::ptr::addr_of_mut!(LARGE).cast::<u8>();
        assert_eq!(data.read_volatile(), 0);
        assert_eq!(data.add(64 * 1024 * 1024 - 1).read_volatile(), 0);
        data.write_volatile(17);
        data.add(64 * 1024 * 1024 - 1).write_volatile(29);
        assert_eq!(data.read_volatile(), 17);
        assert_eq!(data.add(64 * 1024 * 1024 - 1).read_volatile(), 29);
    }
    userlib::println!("[captest] PASS");
}
