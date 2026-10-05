//! VGA mode 13h (320x200, 256 colors) framebuffer driver.
//!
//! When the `vga_320x200` bootloader feature is enabled, the bootloader
//! switches the VGA card to mode 13h via BIOS and maps the 64 KiB framebuffer
//! at physical address `0xA0000`. Combined with `map_physical_memory`, the
//! framebuffer is then accessible at `physical_memory_offset + 0xA0000`.
//!
//! This module provides drawing primitives ([`FrameBuffer`]) plus a global
//! framebuffer initialized by [`init`]. All drawing is bounds-checked and
//! clipped, so out-of-range coordinates can never corrupt memory.
//!
//! Text is rendered with an 8x8 bitmap font ([`font8x8`]).

use conquer_once::spin::OnceCell;
use font8x8::{UnicodeFonts, BASIC_FONTS};
use x86_64::instructions::port::{PortReadOnly, PortWriteOnly};
use x86_64::VirtAddr;

/// Width of the mode 13h framebuffer in pixels.
pub const WIDTH: usize = 320;
/// Height of the mode 13h framebuffer in pixels.
pub const HEIGHT: usize = 200;
/// Physical address of the VGA framebuffer in mode 13h.
pub const FRAMEBUFFER_PHYS_ADDR: u64 = 0xA0000;
/// Bytes per pixel (8-bit paletted).
pub const BYTES_PER_PIXEL: usize = 1;

/// Width of a text character cell in pixels.
pub const CHAR_WIDTH: usize = 8;
/// Height of a text character cell in pixels.
pub const CHAR_HEIGHT: usize = 8;
/// Text columns on screen.
#[allow(clippy::integer_division, reason = "320 / 8 is exact")]
pub const COLS: usize = WIDTH / CHAR_WIDTH;
/// Text rows on screen.
#[allow(clippy::integer_division, reason = "200 / 8 is exact")]
pub const ROWS: usize = HEIGHT / CHAR_HEIGHT;

// Palette indices (programmed by `set_palette` at boot).
/// Black.
pub const COLOR_BLACK: u8 = 0;
/// Blue.
pub const COLOR_BLUE: u8 = 1;
/// Green.
pub const COLOR_GREEN: u8 = 2;
/// Cyan.
pub const COLOR_CYAN: u8 = 3;
/// Red.
pub const COLOR_RED: u8 = 4;
/// Magenta.
pub const COLOR_MAGENTA: u8 = 5;
/// Brown.
pub const COLOR_BROWN: u8 = 6;
/// Light gray.
pub const COLOR_LIGHT_GRAY: u8 = 7;
/// Dark gray.
pub const COLOR_DARK_GRAY: u8 = 8;
/// Bright blue.
pub const COLOR_BRIGHT_BLUE: u8 = 9;
/// Bright green.
pub const COLOR_BRIGHT_GREEN: u8 = 10;
/// Bright cyan.
pub const COLOR_BRIGHT_CYAN: u8 = 11;
/// Bright red.
pub const COLOR_BRIGHT_RED: u8 = 12;
/// Bright magenta.
pub const COLOR_BRIGHT_MAGENTA: u8 = 13;
/// Yellow.
pub const COLOR_YELLOW: u8 = 14;
/// White.
pub const COLOR_WHITE: u8 = 15;

/// The 16 standard VGA colors as 6-bit RGB triples for the DAC.
const PALETTE: [(u8, u8, u8); 16] = [
    (0x00, 0x00, 0x00), // black
    (0x00, 0x00, 0x2A), // blue
    (0x00, 0x2A, 0x00), // green
    (0x00, 0x2A, 0x2A), // cyan
    (0x2A, 0x00, 0x00), // red
    (0x2A, 0x00, 0x2A), // magenta
    (0x2A, 0x15, 0x00), // brown
    (0x2A, 0x2A, 0x2A), // light gray
    (0x15, 0x15, 0x15), // dark gray
    (0x15, 0x15, 0x3F), // bright blue
    (0x15, 0x3F, 0x15), // bright green
    (0x15, 0x3F, 0x3F), // bright cyan
    (0x3F, 0x15, 0x15), // bright red
    (0x3F, 0x15, 0x3F), // bright magenta
    (0x3F, 0x3F, 0x15), // yellow
    (0x3F, 0x3F, 0x3F), // white
];

/// A mode 13h framebuffer.
///
/// The pixel at `(x, y)` lives at `ptr + y * width + x`. All methods clip to
/// the framebuffer bounds.
#[derive(Debug)]
pub struct FrameBuffer {
    ptr: *mut u8,
    width: usize,
    height: usize,
}

// SAFETY: The framebuffer is a single global MMIO region; access is
// serialized through the global lock.
unsafe impl Send for FrameBuffer {}

impl FrameBuffer {
    /// Returns the width in pixels.
    #[must_use]
    pub const fn width(&self) -> usize {
        self.width
    }

    /// Returns the height in pixels.
    #[must_use]
    pub const fn height(&self) -> usize {
        self.height
    }

    /// Creates a framebuffer view over a raw pixel buffer.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that `ptr` points to at least
    /// `width * height` writable bytes for the lifetime of the framebuffer.
    #[must_use]
    pub const unsafe fn new(ptr: *mut u8, width: usize, height: usize) -> Self {
        Self { ptr, width, height }
    }

    /// Returns the raw pixel buffer pointer.
    ///
    /// # Safety
    ///
    /// The caller must ensure the pointer is used within the `width * height`
    /// byte buffer and that no aliasing `&mut` references exist.
    #[must_use]
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.ptr
    }

    /// Returns the color index of the pixel at `(x, y)`, or `None` if out of bounds.
    #[must_use]
    pub fn get_pixel(&self, x: usize, y: usize) -> Option<u8> {
        if x < self.width && y < self.height {
            // SAFETY: Bounds-checked above; the buffer is valid.
            Some(unsafe { self.ptr.add(y * self.width + x).read() })
        } else {
            None
        }
    }

    /// Sets the pixel at `(x, y)`; silently ignores out-of-bounds coordinates.
    pub fn set_pixel(&mut self, x: usize, y: usize, color: u8) {
        if x < self.width && y < self.height {
            // SAFETY: Bounds-checked above; the buffer is valid and we hold
            // the global lock (or own the test buffer).
            unsafe {
                self.ptr.add(y * self.width + x).write(color);
            }
        }
    }

    /// Fills the entire framebuffer with `color`.
    pub fn clear(&mut self, color: u8) {
        // SAFETY: The buffer holds exactly width*height bytes.
        unsafe {
            core::ptr::write_bytes(self.ptr, color, self.width * self.height);
        }
    }

    /// Fills a rectangle, clipped to the framebuffer bounds.
    pub fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: u8) {
        let x_end = core::cmp::min(x.saturating_add(w), self.width);
        let y_end = core::cmp::min(y.saturating_add(h), self.height);
        for row in y..y_end {
            for col in x..x_end {
                // SAFETY: Clipped to bounds above.
                unsafe {
                    self.ptr.add(row * self.width + col).write(color);
                }
            }
        }
    }

    /// Draws an 8x8 character cell at pixel `(x, y)`.
    ///
    /// `bg` of `None` leaves the background transparent. Unknown characters
    /// render as a blank cell.
    pub fn draw_char(&mut self, x: usize, y: usize, c: u8, fg: u8, bg: Option<u8>) {
        let glyph = BASIC_FONTS
            .get(char::from(c))
            .unwrap_or([0_u8; 8]);
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..8 {
                let pixel_on = (bits >> col) & 1 == 1;
                if pixel_on {
                    self.set_pixel(x + col, y + row, fg);
                } else if let Some(bg_color) = bg {
                    self.set_pixel(x + col, y + row, bg_color);
                }
            }
        }
    }

    /// Draws a byte string left-to-right starting at pixel `(x, y)`.
    pub fn draw_text(&mut self, x: usize, y: usize, text: &[u8], fg: u8, bg: Option<u8>) {
        for (i, &c) in text.iter().enumerate() {
            self.draw_char(x + i * CHAR_WIDTH, y, c, fg, bg);
        }
    }
}

/// The global hardware framebuffer, initialized by [`init`].
static FRAMEBUFFER: OnceCell<spin::Mutex<FrameBuffer>> = OnceCell::uninit();

/// Physical address of the framebuffer in use, set by [`init`].
static FB_PHYS_ADDR: OnceCell<u64> = OnceCell::uninit();

/// Initializes the graphics subsystem: maps the hardware framebuffer,
/// programs the palette, and clears the screen.
///
/// Must be called once before any drawing or console output. `phys_mem_offset`
/// must map all physical memory; `fb_phys_addr` is the physical address of
/// the linear framebuffer configured by the bootloader, with the given
/// `width`/`height` in pixels (8 bits per pixel).
///
/// # Panics
///
/// Panics if called more than once.
pub fn init(phys_mem_offset: VirtAddr, fb_phys_addr: u64, width: usize, height: usize) {
    FB_PHYS_ADDR
        .try_init_once(|| fb_phys_addr)
        .expect("gfx::init must be called exactly once");
    let ptr = (phys_mem_offset.as_u64() + fb_phys_addr) as *mut u8;
    // SAFETY: The caller guarantees that all physical memory is mapped at
    // `phys_mem_offset`; the framebuffer is a linear 8-bpp MMIO region.
    let fb = unsafe { FrameBuffer::new(ptr, width, height) };
    FRAMEBUFFER
        .try_init_once(|| spin::Mutex::new(fb))
        .expect("gfx::init must be called exactly once");
    set_palette();
    with_framebuffer(|fb| fb.clear(COLOR_BLACK));
}

/// Returns the text console width in characters for the current framebuffer.
///
/// # Panics
///
/// Panics if [`init`] was not called first.
#[must_use]
pub fn cols() -> usize {
    with_framebuffer(|fb| fb.width() / CHAR_WIDTH)
}

/// Returns the text console height in characters for the current framebuffer.
///
/// # Panics
///
/// Panics if [`init`] was not called first.
#[must_use]
pub fn rows() -> usize {
    with_framebuffer(|fb| fb.height() / CHAR_HEIGHT)
}

/// Returns the physical address of the framebuffer passed to [`init`].
///
/// # Panics
///
/// Panics if [`init`] was not called first.
#[must_use]
pub fn framebuffer_phys_addr() -> u64 {
    *FB_PHYS_ADDR.try_get().expect("gfx::init was not called")
}

/// Runs `f` with the global framebuffer locked.
///
/// # Panics
///
/// Panics if [`init`] was not called first.
pub fn with_framebuffer<F, R>(f: F) -> R
where
    F: FnOnce(&mut FrameBuffer) -> R,
{
    let lock = FRAMEBUFFER.try_get().expect("gfx::init was not called");
    f(&mut lock.lock())
}

/// Programs the VGA palette via the DAC ports.
fn set_palette() {
    // SAFETY: Writing the VGA DAC ports is the defined way to set the palette.
    unsafe {
        let mut index = PortWriteOnly::new(0x3C8_u16);
        let mut data = PortWriteOnly::new(0x3C9_u16);
        index.write(0_u8);
        for (r, g, b) in PALETTE {
            data.write(r);
            data.write(g);
            data.write(b);
        }
    }
}

/// Programs VGA mode 13h (320x200, 256 colors) by writing the VGA registers
/// directly. Works in long mode without BIOS calls.
///
/// This is used by the GRUB boot path: GRUB 2.12 cannot reliably provide a
/// graphics framebuffer via Multiboot2 (its framebuffer request tag corrupts
/// the heap), so the kernel sets up its own video mode. The framebuffer is
/// then at physical address 0xA0000.
///
/// # Safety
///
/// Must be called in ring 0 with I/O port access. Resets the VGA hardware;
/// any previous video mode is lost.
pub unsafe fn set_vga_mode_13h() {
    // SAFETY: The caller guarantees ring-0 I/O access. These are the
    // standard VGA mode 13h register values.
    unsafe {
        // Miscellaneous Output Register: 25MHz clock, enable RAM, etc.
        PortWriteOnly::new(0x3C2_u16).write(0x63_u8);

        // Sequencer registers.
        let mut seq_index = PortWriteOnly::new(0x3C4_u16);
        let mut seq_data = PortWriteOnly::new(0x3C5_u16);
        for (i, val) in [0x03_u8, 0x01, 0x0F, 0x00, 0x0E].iter().enumerate() {
            seq_index.write(i as u8);
            seq_data.write(*val);
        }

        // CRTC registers. Unlock first (register 0x11, bit 7 = 0).
        let mut crtc_index = PortWriteOnly::new(0x3D4_u16);
        let mut crtc_data = PortWriteOnly::new(0x3D5_u16);
        crtc_index.write(0x11_u8);
        crtc_data.write(0x0E_u8);
        for (i, val) in [
            0x5F_u8, 0x4F, 0x50, 0x82, 0x54, 0x80, 0xBF, 0x1F, 0x00, 0x41, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x9C, 0x0E, 0x8F, 0x28, 0x40, 0x96, 0xB9, 0xA3, 0xFF,
        ]
        .iter()
        .enumerate()
        {
            crtc_index.write(i as u8);
            crtc_data.write(*val);
        }

        // Graphics Controller registers.
        let mut gc_index = PortWriteOnly::new(0x3CE_u16);
        let mut gc_data = PortWriteOnly::new(0x3CF_u16);
        for (i, val) in [0x00_u8, 0x00, 0x00, 0x00, 0x00, 0x40, 0x05, 0x0F, 0xFF]
            .iter()
            .enumerate()
        {
            gc_index.write(i as u8);
            gc_data.write(*val);
        }

        // Attribute Controller registers. Reset the flip-flop first by
        // reading the Input Status register.
        PortReadOnly::<u8>::new(0x3DA_u16).read();
        let mut ac = PortWriteOnly::new(0x3C0_u16);
        for (i, val) in [
            0x00_u8, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D,
            0x0E, 0x0F, 0x41, 0x00, 0x0F, 0x00, 0x00,
        ]
        .iter()
        .enumerate()
        {
            ac.write(i as u8);
            ac.write(*val);
        }
        // Reset flip-flop and enable display.
        PortReadOnly::<u8>::new(0x3DA_u16).read();
    }
}

#[cfg(test)]
mod tests {
    use super::{FrameBuffer, CHAR_HEIGHT, CHAR_WIDTH, COLOR_BLACK, COLOR_WHITE, HEIGHT, WIDTH};
    use alloc::vec::Vec;

    /// Creates a test framebuffer backed by a Vec.
    fn test_fb() -> (FrameBuffer, Vec<u8>) {
        let mut buf = alloc::vec![0_u8; WIDTH * HEIGHT];
        // SAFETY: The Vec outlives the framebuffer in these tests.
        let fb = unsafe { FrameBuffer::new(buf.as_mut_ptr(), WIDTH, HEIGHT) };
        (fb, buf)
    }

    #[test_case]
    fn test_set_and_get_pixel() {
        let (mut fb, _buf) = test_fb();
        fb.set_pixel(10, 20, 42);
        assert_eq!(fb.get_pixel(10, 20), Some(42));
        assert_eq!(fb.get_pixel(11, 20), Some(0));
    }

    #[test_case]
    fn test_pixel_out_of_bounds_is_ignored() {
        let (mut fb, buf) = test_fb();
        fb.set_pixel(WIDTH, HEIGHT, 99);
        fb.set_pixel(usize::MAX, usize::MAX, 99);
        assert!(buf.iter().all(|&b| b == 0));
        assert_eq!(fb.get_pixel(WIDTH, 0), None);
    }

    #[test_case]
    fn test_clear_fills_screen() {
        let (mut fb, buf) = test_fb();
        fb.clear(7);
        assert!(buf.iter().all(|&b| b == 7));
    }

    #[test_case]
    fn test_fill_rect_clips_to_bounds() {
        let (mut fb, _buf) = test_fb();
        // Rectangle extends past the bottom-right corner.
        fb.fill_rect(WIDTH - 2, HEIGHT - 2, 10, 10, 5);
        assert_eq!(fb.get_pixel(WIDTH - 1, HEIGHT - 1), Some(5));
        assert_eq!(fb.get_pixel(WIDTH - 3, HEIGHT - 3), Some(0));
        // Fully out-of-bounds rectangle draws nothing and does not panic.
        fb.fill_rect(WIDTH + 10, HEIGHT + 10, 5, 5, 5);
    }

    #[test_case]
    fn test_draw_char_renders_glyph_pixels() {
        let (mut fb, _buf) = test_fb();
        fb.draw_char(0, 0, b'A', COLOR_WHITE, Some(COLOR_BLACK));
        // The glyph for 'A' has pixels set in its 8x8 cell; the cell must not
        // be entirely background after drawing.
        let mut fg_count = 0;
        for y in 0..CHAR_HEIGHT {
            for x in 0..CHAR_WIDTH {
                if fb.get_pixel(x, y) == Some(COLOR_WHITE) {
                    fg_count += 1;
                }
            }
        }
        assert!(fg_count > 0, "glyph 'A' should have foreground pixels");
        assert!(fg_count < 64, "glyph 'A' should not fill the whole cell");
    }

    #[test_case]
    fn test_draw_text_advances_by_char_width() {
        let (mut fb, _buf) = test_fb();
        fb.clear(COLOR_BLACK);
        fb.draw_text(0, 0, b"AB", COLOR_WHITE, Some(COLOR_BLACK));
        // Second character starts at x = CHAR_WIDTH; its cell must differ
        // from background somewhere (glyphs 'A' and 'B' are non-blank).
        let cell_a_has_fg = (0..CHAR_WIDTH)
            .any(|x| fb.get_pixel(x, 0) == Some(COLOR_WHITE));
        let cell_b_has_fg = (0..CHAR_WIDTH)
            .any(|x| fb.get_pixel(CHAR_WIDTH + x, 0) == Some(COLOR_WHITE));
        assert!(cell_a_has_fg && cell_b_has_fg);
    }
}
