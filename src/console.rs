//! Framebuffer text console.
//!
//! Replaces the VGA text-mode buffer: in graphics mode there is no text
//! buffer, so characters are rendered as 8x8 glyphs into the framebuffer by
//! [`gfx`](crate::gfx). The console scrolls; its size in characters is
//! derived from the framebuffer dimensions at init time.
//!
//! The [`print!`] and [`println!`] macros write here; [`serial_println!`]
//! still goes to the serial port.

use core::fmt::{self, Write};

use conquer_once::spin::OnceCell;
use spin::Mutex;
use x86_64::instructions::interrupts;

use crate::gfx::{self, CHAR_HEIGHT, CHAR_WIDTH, COLOR_BLACK, COLOR_LIGHT_GRAY};

/// A text console rendered into the graphics framebuffer.
#[derive(Debug)]
pub struct Console {
    col: usize,
    row: usize,
    fg: u32,
    bg: u32,
    cols: usize,
    rows: usize,
}

impl Console {
    /// Creates a console with the default colors.
    /// Dimensions are set by [`for_framebuffer`]; this is for tests only.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            col: 0,
            row: 0,
            fg: COLOR_LIGHT_GRAY,
            bg: COLOR_BLACK,
            // Default 80x25 for tests; real size comes from for_framebuffer().
            cols: 80,
            rows: 25,
        }
    }

    /// Creates a console sized for the currently initialized framebuffer.
    #[must_use]
    pub fn for_framebuffer() -> Self {
        let info = gfx::fb_info();
        Self {
            cols: info.width / CHAR_WIDTH,
            rows: info.height / CHAR_HEIGHT,
            ..Self::new()
        }
    }

    /// Writes a single byte, handling `\n` and scrolling.
    fn write_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => self.new_line(),
            byte => {
                if self.col >= self.cols {
                    self.new_line();
                }
                let c = byte as char;
                gfx::with_framebuffer(|fb| {
                    fb.draw_char(c, self.col, self.row, self.fg, self.bg)
                });
                self.col += 1;
            }
        }
    }

    fn new_line(&mut self) {
        self.col = 0;
        if self.row + 1 >= self.rows {
            self.scroll();
        } else {
            self.row += 1;
        }
    }

    /// Scrolls the console up by one text row.
    fn scroll(&mut self) {
        // For 32bpp with pitch, we need to copy line by line.
        // Simpler: clear and redraw is expensive. For now, do a naive
        // pixel-by-pixel scroll using the framebuffer API.
        //
        // Actually, the simplest correct approach: use with_framebuffer
        // to get direct access. But FrameBuffer doesn't expose raw ptr.
        // Let's add a scroll method to FrameBuffer instead.
        gfx::with_framebuffer(|fb| {
            fb.scroll_up(CHAR_HEIGHT, self.bg);
        });
    }

    /// Clears the screen and homes the cursor.
    pub fn clear(&mut self) {
        gfx::with_framebuffer(|fb| {
            fb.fill_rect(0, 0, fb.width(), fb.height(), self.bg);
        });
        self.col = 0;
        self.row = 0;
    }
}

impl Default for Console {
    fn default() -> Self {
        Self::new()
    }
}

impl Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            match byte {
                // Printable ASCII or newline.
                0x20..=0x7E | b'\n' => self.write_byte(byte),
                // Replace other bytes (e.g. UTF-8 multibyte) with '?'.
                _ => self.write_byte(b'?'),
            }
        }
        Ok(())
    }
}

/// The global console, initialized by [`init`].
static WRITER: OnceCell<Mutex<Console>> = OnceCell::uninit();

/// Initializes the console. Must be called after [`gfx::init`].
///
/// # Panics
///
/// Panics if called more than once.
pub fn init() {
    WRITER
        .try_init_once(|| Mutex::new(Console::for_framebuffer()))
        .expect("console::init must be called exactly once");
    WRITER.try_get().expect("console just initialized").lock().clear();
}

/// Copy of the std `print!` macro that redirects to our internal `_print`.
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::console::_print(format_args!($($arg)*)));
}

/// Copy of the std `println!` macro that redirects to our internal `_print`.
#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}

/// Prints formatted arguments to the framebuffer console.
///
/// Does nothing until [`init`] has been called (early boot code must init
/// the console before printing).
#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    if DESKTOP_ACTIVE.load(core::sync::atomic::Ordering::Relaxed) { return; }
    use core::fmt::Write;
    if let Ok(writer) = WRITER.try_get() {
        #[expect(
            clippy::unwrap_used,
            reason = "We should be able to write to the framebuffer console"
        )]
        interrupts::without_interrupts(|| {
            writer.lock().write_fmt(args).unwrap();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::Console;
    use core::fmt::Write;

    // Test dimensions (matches Console::new defaults).
    const TEST_COLS: usize = 80;
    const TEST_ROWS: usize = 25;

    #[test_case]
    fn test_console_advances_cursor() {
        let mut console = Console::new();
        write!(console, "hi").unwrap();
        assert_eq!((console.col, console.row), (2, 0));
        write!(console, "\n").unwrap();
        assert_eq!((console.col, console.row), (0, 1));
    }

    #[test_case]
    fn test_console_wraps_at_end_of_row() {
        let mut console = Console::new();
        for _ in 0..TEST_COLS {
            write!(console, "x").unwrap();
        }
        assert_eq!((console.col, console.row), (TEST_COLS, 0));
        // The next character wraps to the following row.
        write!(console, "y").unwrap();
        assert_eq!((console.col, console.row), (1, 1));
    }

    #[test_case]
    fn test_console_scroll_keeps_cursor_on_last_row() {
        let mut console = Console::new();
        for _ in 0..(TEST_ROWS + 5) {
            write!(console, "\n").unwrap();
        }
        assert_eq!((console.col, console.row), (0, TEST_ROWS - 1));
    }

    #[test_case]
    fn test_console_non_ascii_becomes_question_mark() {
        let mut console = Console::new();
        write!(console, "héllo✓").unwrap();
        // Each non-ASCII byte becomes '?': 'h'(1) + 'é'(2) + 'l','l','o'(3) + '✓'(3) = 9.
        assert_eq!(console.col, 9);
    }
}

/// Stops boot console writes once the compositor owns the display.
pub static DESKTOP_ACTIVE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
