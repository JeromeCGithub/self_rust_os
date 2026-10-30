//! Fixed-format wallpaper loading and immutable pixel ownership.
use userlib::sys;
const WALL_HEADER: &[u8] = b"P6\n1920 1080\n255\n";
const WALL_BYTES: usize = 17 + 1920 * 1080 * 3;
static mut WALL: [u8; WALL_BYTES] = [0; WALL_BYTES];

/// Load once before rendering. The returned payload stays immutable for this process.
pub(super) fn load() -> Option<&'static [u8]> {
    use core::sync::atomic::{AtomicBool, Ordering};
    static LOADED: AtomicBool = AtomicBool::new(false);
    // Only one caller may mutate WALL; subsequent calls never borrow it mutably.
    if LOADED.swap(true, Ordering::Relaxed) {
        return None;
    }
    // SAFETY: This process alone owns WALL, borrowed exclusively for the
    // synchronous file read. The borrow ends before immutable render access;
    // the loaded size/header are validated before reading any RGB payload.
    let loaded = unsafe {
        sys::read_file("/ETC/WALL.PPM", &mut *core::ptr::addr_of_mut!(WALL)) == Some(WALL_BYTES)
            && &WALL[..17] == WALL_HEADER
    };
    if loaded {
        // SAFETY: The event loop calls load once; WALL remains immutable afterwards.
        Some(unsafe { &(&*core::ptr::addr_of!(WALL))[17..] })
    } else {
        None
    }
}
