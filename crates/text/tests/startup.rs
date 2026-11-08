#![cfg(feature = "truetype")]
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
struct Count;
static BYTES: AtomicUsize = AtomicUsize::new(0);
// SAFETY: This host-only observer forwards every valid GlobalAlloc request and
// matching pointer/layout unchanged to System. It retains no pointer/reference;
// relaxed accounting has no synchronization role. The sole test reads its delta.
unsafe impl GlobalAlloc for Count {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        // SAFETY: Forward the caller's GlobalAlloc layout unchanged to System.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: This pointer and matching layout were allocated by System via
        // this forwarding allocator; no allocation ownership is retained here.
        unsafe { System.dealloc(pointer, layout) }
    }
}
#[global_allocator]
static ALLOC: Count = Count;
#[test]
fn constructing_embedded_faces_does_not_compile_or_allocate_every_glyph() {
    let before = BYTES.load(Ordering::Relaxed);
    let fonts = rustos_text::FontSet::load(
        include_bytes!("../../../third_party/fonts/DejaVuSans.ttf"),
        include_bytes!("../../../third_party/fonts/DejaVuSans-Bold.ttf"),
        include_bytes!("../../../third_party/fonts/DejaVuSerif.ttf"),
        include_bytes!("../../../third_party/fonts/DejaVuSansMono.ttf"),
    )
    .unwrap()
    .with_bold_faces(
        include_bytes!("../../../third_party/fonts/DejaVuSerif-Bold.ttf"),
        include_bytes!("../../../third_party/fonts/DejaVuSansMono-Bold.ttf"),
    )
    .unwrap();
    let allocated = BYTES.load(Ordering::Relaxed) - before;
    assert!(
        allocated < 128 * 1024,
        "font initialization allocated {allocated} bytes before a glyph was requested"
    );
    use rustos_text::Metrics;
    assert!(fonts.advance('Ω', rustos_text::Style::default()) > 0);
}
