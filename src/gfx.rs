//! Dynamic linear framebuffer driver (32bpp XRGB8888).
//!
//! The framebuffer geometry (address, width, height, pitch, bpp) is provided
//! by the bootloader (Multiboot2 framebuffer tag for GRUB, or VGA constants
//! for the legacy path). All drawing is bounds-checked and clipped.
//!
//! Text is rendered with an 8x8 bitmap font ([`font8x8`]), scaled 2x for
//! readability at high resolutions.

use conquer_once::spin::OnceCell;
use font8x8::{UnicodeFonts, BASIC_FONTS};
use x86_64::VirtAddr;

/// Framebuffer information from the bootloader.
#[derive(Debug, Clone, Copy)]
pub struct FbInfo {
    /// Physical address of the linear framebuffer.
    pub addr: u64,
    /// Width in pixels.
    pub width: usize,
    /// Height in pixels.
    pub height: usize,
    /// Bytes per scanline (may exceed width * 4).
    pub pitch: usize,
    /// Bits per pixel (must be 32).
    pub bpp: u8,
}

/// Width of a text character cell in pixels (2x scaled 8x8).
pub const CHAR_WIDTH: usize = 16;
/// Height of a text character cell in pixels (2x scaled 8x8).
pub const CHAR_HEIGHT: usize = 16;

// 32-bit XRGB colors (0x00RRGGBB).
pub const COLOR_BLACK: u32 = 0x000000;
pub const COLOR_BLUE: u32 = 0x0000AA;
pub const COLOR_GREEN: u32 = 0x00AA00;
pub const COLOR_CYAN: u32 = 0x00AAAA;
pub const COLOR_RED: u32 = 0xAA0000;
pub const COLOR_MAGENTA: u32 = 0xAA00AA;
pub const COLOR_BROWN: u32 = 0xAA5500;
pub const COLOR_LIGHT_GRAY: u32 = 0xAAAAAA;
pub const COLOR_DARK_GRAY: u32 = 0x555555;
pub const COLOR_BRIGHT_BLUE: u32 = 0x5555FF;
pub const COLOR_BRIGHT_GREEN: u32 = 0x55FF55;
pub const COLOR_BRIGHT_CYAN: u32 = 0x55FFFF;
pub const COLOR_BRIGHT_RED: u32 = 0xFF5555;
pub const COLOR_BRIGHT_MAGENTA: u32 = 0xFF55FF;
pub const COLOR_YELLOW: u32 = 0xFFFF55;
pub const COLOR_WHITE: u32 = 0xFFFFFF;

/// A 32bpp linear framebuffer with pitch.
pub struct FrameBuffer {
    ptr: *mut u8,
    width: usize,
    height: usize,
    pitch: usize,
}

// SAFETY: The framebuffer is a singleton accessed via a Mutex.
// The raw pointer is only dereferenced within the Mutex guard.
unsafe impl Send for FrameBuffer {}
unsafe impl Sync for FrameBuffer {}

impl FrameBuffer {
    /// Creates a framebuffer from the given info and virtual base address.
    ///
    /// # Safety
    /// `virt_addr` must point to the mapped framebuffer memory.
    pub unsafe fn new(info: &FbInfo, virt_addr: VirtAddr) -> Self {
        Self {
            ptr: virt_addr.as_mut_ptr(),
            width: info.width,
            height: info.height,
            pitch: info.pitch,
        }
    }

    /// Sets a pixel to a 32-bit XRGB color. Out-of-bounds is clipped.
    pub fn set_pixel(&mut self, x: usize, y: usize, color: u32) {
        if x >= self.width || y >= self.height {
            return;
        }
        // SAFETY: Bounds checked above. The framebuffer is mapped.
        unsafe {
            let offset = y * self.pitch + x * 4;
            let pixel_ptr = self.ptr.add(offset) as *mut u32;
            pixel_ptr.write_volatile(color);
        }
    }

    /// Fills a rectangle with a color. Clipped to framebuffer bounds.
    pub fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: u32) {
        let x_end = (x + w).min(self.width);
        let y_end = (y + h).min(self.height);
        for py in y..y_end {
            for px in x..x_end {
                self.set_pixel(px, py, color);
            }
        }
    }

    /// Draws a character at (col, row) in text coordinates.
    pub fn draw_char(&mut self, c: char, col: usize, row: usize, fg: u32, bg: u32) {
        let glyph = BASIC_FONTS.get(c).unwrap_or([0; 8]);
        let x0 = col * CHAR_WIDTH;
        let y0 = row * CHAR_HEIGHT;
        for (gy, row_bits) in glyph.iter().enumerate() {
            for gx in 0..8 {
                let color = if (row_bits >> gx) & 1 == 1 { fg } else { bg };
                // 2x scale: each source pixel becomes 2x2.
                self.fill_rect(x0 + gx * 2, y0 + gy * 2, 2, 2, color);
            }
        }
    }

    /// Returns the width in pixels.
    pub fn width(&self) -> usize {
        self.width
    }

    /// Returns the height in pixels.
    pub fn height(&self) -> usize {
        self.height
    }

    /// Returns text columns.
    pub fn cols(&self) -> usize {
        self.width / CHAR_WIDTH
    }

    /// Returns text rows.
    pub fn rows(&self) -> usize {
        self.height / CHAR_HEIGHT
    }

    /// Scrolls the framebuffer up by `pixels` rows, filling the bottom with `color`.
    pub fn scroll_up(&mut self, pixels: usize, color: u32) {
        if pixels >= self.height {
            self.fill_rect(0, 0, self.width, self.height, color);
            return;
        }
        // Copy rows [pixels..height) to [0..height-pixels).
        // Must handle pitch correctly and overlapping regions.
        for y in 0..(self.height - pixels) {
            let src_offset = (y + pixels) * self.pitch;
            let dst_offset = y * self.pitch;
            // SAFETY: Both regions are within the framebuffer.
            // Use copy (not copy_nonoverlapping) for overlapping regions.
            unsafe {
                let src = self.ptr.add(src_offset);
                let dst = self.ptr.add(dst_offset);
                core::ptr::copy(src, dst, self.width * 4);
            }
        }
        // Clear the bottom rows.
        self.fill_rect(0, self.height - pixels, self.width, pixels, color);
    }
}

// Global framebuffer, initialized by `init`.
static FRAMEBUFFER: OnceCell<spin::Mutex<FrameBuffer>> = OnceCell::uninit();
static FB_INFO: OnceCell<FbInfo> = OnceCell::uninit();

/// Initializes the global framebuffer.
///
/// `fb_virt_addr` is the virtual address where the framebuffer is mapped.
/// Must be called once before any drawing.
///
/// # Safety
/// `fb_virt_addr` must point to mapped framebuffer memory of sufficient size.
pub unsafe fn init(info: FbInfo, fb_virt_addr: VirtAddr) {
    assert_eq!(info.bpp, 32, "Only 32bpp framebuffers are supported");
    let fb = FrameBuffer::new(&info, fb_virt_addr);
    FB_INFO.init_once(|| info);
    FRAMEBUFFER.init_once(|| spin::Mutex::new(fb));
}

/// Returns the framebuffer info.
pub fn fb_info() -> FbInfo {
    *FB_INFO.get().expect("Framebuffer not initialized")
}

/// Returns the framebuffer info, or `None` if not initialized.
/// Useful for code that needs to work in unit tests.
pub fn try_fb_info() -> Option<FbInfo> {
    FB_INFO.get().copied()
}

/// Returns the physical address of the framebuffer.
/// Panics if the framebuffer is not initialized.
pub fn framebuffer_phys_addr() -> u64 {
    fb_info().addr
}

/// Runs a closure with the global framebuffer.
pub fn with_framebuffer<F, R>(f: F) -> R
where
    F: FnOnce(&mut FrameBuffer) -> R,
{
    let fb = FRAMEBUFFER.get().expect("Framebuffer not initialized");
    let mut guard = fb.lock();
    f(&mut *guard)
}
