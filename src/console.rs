//! Framebuffer text console.
//!
//! Replaces the VGA text-mode buffer: in graphics mode (mode 13h) there is no
//! text buffer, so characters are rendered as 8x8 glyphs into the framebuffer
//! by [`gfx`](crate::gfx). The console is 40x25 characters and scrolls.
//!
//! The [`print!`] and [`println!`] macros write here; [`serial_println!`]
//! still goes to the serial port.

use core::fmt::{self, Write};

use conquer_once::spin::OnceCell;
use spin::Mutex;
use x86_64::instructions::interrupts;

use crate::gfx::{
    self, CHAR_HEIGHT, CHAR_WIDTH, COLOR_BLACK, COLOR_LIGHT_GRAY, COLS, ROWS,
};

/// A text console rendered into the graphics framebuffer.
#[derive(Debug)]
pub struct Console {
    col: usize,
    row: usize,
    fg: u8,
    bg: u8,
}

impl Console {
    /// Creates a console with the default colors.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            col: 0,
            row: 0,
            fg: COLOR_LIGHT_GRAY,
            bg: COLOR_BLACK,
        }
    }

    /// Writes a single byte, handling `\n` and scrolling.
    fn write_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => self.new_line(),
            byte => {
                if self.col >= COLS {
                    self.new_line();
                }
                let x = self.col * CHAR_WIDTH;
                let y = self.row * CHAR_HEIGHT;
                gfx::with_framebuffer(|fb| fb.draw_char(x, y, byte, self.fg, Some(self.bg)));
                self.col += 1;
            }
        }
    }

    fn new_line(&mut self) {
        self.col = 0;
        if self.row + 1 >= ROWS {
            self.scroll();
        } else {
            self.row += 1;
        }
    }

    /// Scrolls the console up by one text row.
    fn scroll(&mut self) {
        gfx::with_framebuffer(|fb| {
            let row_bytes = COLS * CHAR_WIDTH;
            let ptr = fb.as_mut_ptr();
            // Move rows 1..ROWS up by one text row. The regions overlap,
            // so `copy` (not `copy_nonoverlapping`) is required.
            //
            // SAFETY: Both regions are inside the `WIDTH * HEIGHT` byte
            // framebuffer; `ptr` is the buffer start.
            unsafe {
                let src = ptr.add(CHAR_HEIGHT * row_bytes);
                let bytes = (ROWS - 1) * CHAR_HEIGHT * row_bytes;
                core::ptr::copy(src, ptr, bytes);
            }
            // Clear the last row.
            fb.fill_rect(0, (ROWS - 1) * CHAR_HEIGHT, row_bytes, CHAR_HEIGHT, self.bg);
        });
    }

    /// Clears the screen and homes the cursor.
    pub fn clear(&mut self) {
        gfx::with_framebuffer(|fb| fb.clear(self.bg));
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
        .try_init_once(|| Mutex::new(Console::new()))
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
    use super::{Console, COLS, ROWS};
    use core::fmt::Write;

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
        for _ in 0..COLS {
            write!(console, "x").unwrap();
        }
        assert_eq!((console.col, console.row), (COLS, 0));
        // The next character wraps to the following row.
        write!(console, "y").unwrap();
        assert_eq!((console.col, console.row), (1, 1));
    }

    #[test_case]
    fn test_console_scroll_keeps_cursor_on_last_row() {
        let mut console = Console::new();
        for _ in 0..(ROWS + 5) {
            write!(console, "\n").unwrap();
        }
        assert_eq!((console.col, console.row), (0, ROWS - 1));
    }

    #[test_case]
    fn test_console_non_ascii_becomes_question_mark() {
        let mut console = Console::new();
        write!(console, "héllo✓").unwrap();
        // Each non-ASCII byte becomes '?': 'h'(1) + 'é'(2) + 'l','l','o'(3) + '✓'(3) = 9.
        assert_eq!(console.col, 9);
    }
}
