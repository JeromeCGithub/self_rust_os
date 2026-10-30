//! Per-client window state and bounded cache-slot allocation.
use userlib::ui::protocol::{self, Material};
pub(super) const MAX: usize = 6;
#[derive(Clone, Copy)]
pub(super) struct Window {
    pub(super) pid: u32,
    pub(super) large: bool,
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) exited: bool,
    pub(super) cache_slot: usize,
    pub(super) revision: Option<(u64, bool)>,
    pub(super) width: usize,
    pub(super) height: usize,
    pub(super) title: [u8; protocol::TITLE_BYTES],
    pub(super) native: bool,
    pub(super) material: Material,
    pub(super) commit: Option<u32>,
    pub(super) reply: Option<u32>,
    pub(super) focus_event: Option<bool>,
    pub(super) pointer: Option<(i32, i32, u8)>,
    pub(super) closing: Option<u64>,
}
pub(super) fn hit(mx: i32, my: i32, x: i32, y: i32, w: i32, h: i32) -> bool {
    mx >= x && mx < x + w && my >= y && my < y + h
}
pub(super) fn free_slot(windows: &[Option<Window>]) -> usize {
    (0..MAX)
        .find(|slot| !windows.iter().flatten().any(|w| w.cache_slot == *slot))
        .expect("free cache slot")
}
pub(super) fn new_window(pid: u32, cache_slot: usize, index: usize) -> Window {
    let mut title = [0; protocol::TITLE_BYTES];
    title[..11].copy_from_slice(b"Application");
    Window {
        pid,
        large: false,
        x: 80 + index as i32 * 12,
        y: 126 + (index as i32 * 2).min(6),
        exited: false,
        cache_slot,
        revision: None,
        width: 488,
        height: 288,
        title,
        native: false,
        material: Material::Glass,
        commit: None,
        reply: None,
        pointer: None,
        closing: None,
        focus_event: None,
    }
}
