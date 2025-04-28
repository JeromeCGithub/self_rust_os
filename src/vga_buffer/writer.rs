use crate::vga_buffer::{
    buffer::Buffer,
    colors::{Color, ColorCode},
};
use core::fmt;
use lazy_static::lazy_static;
use spin::Mutex;

use super::{
    buffer::ScreenChar,
    constants::{BUFFER_HEIGHT, BUFFER_WIDTH},
};

lazy_static! {
    /// Global instance of the VGA buffer writer.
    /// We use lazy_static to be able to dinamically initialize the color.
    pub static ref WRITER: Mutex<Writer> = Mutex::new(Writer {
        column_position: 0,
        color_code: ColorCode::new(Color::Yellow, Color::Black),

        // SAFETY:
        // vga buffer adress should always exist.
        buffer: unsafe { &mut *(0xb8000 as *mut Buffer) },
    });
}

////////////////////////
//   Screen Writer    //
////////////////////////

/// Represent the writer that will write to the VGA buffer.
pub struct Writer {
    pub column_position: usize,
    pub color_code: ColorCode,
    pub buffer: &'static mut Buffer,
}

impl Writer {
    /// Write a byte to the VGA buffer.
    pub fn write_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => self.new_line(),
            character => {
                if self.column_position >= BUFFER_WIDTH {
                    self.new_line();
                }

                let row = BUFFER_HEIGHT - 1;
                let col = self.column_position;

                let color_code = self.color_code;
                self.buffer.chars[row][col].write(ScreenChar {
                    ascii_character: character,
                    color_code,
                });
                self.column_position += 1;
            }
        }
    }

    /// Write a string to the VGA buffer.
    pub fn write_string(&mut self, s: &str) {
        for byte in s.bytes() {
            match byte {
                // printable ASCII byte or newline
                0x20..=0x7e | b'\n' => self.write_byte(byte),
                // not part of printable ASCII range
                _ => self.write_byte(0xfe),
            }
        }
    }

    /// Move all characters one row up and clear the last row.
    fn new_line(&mut self) {
        for row in 1..BUFFER_HEIGHT {
            for col in 0..BUFFER_WIDTH {
                let character = self.buffer.chars[row][col].read();
                self.buffer.chars[row - 1][col].write(character);
            }
        }
        self.clear_row(BUFFER_HEIGHT - 1);
        self.column_position = 0;
    }

    /// Clear a row by filling it with blank characters.
    fn clear_row(&mut self, row: usize) {
        let blank = ScreenChar {
            ascii_character: b' ',
            color_code: self.color_code,
        };
        for col in 0..BUFFER_WIDTH {
            self.buffer.chars[row][col].write(blank);
        }
    }
}

impl fmt::Write for Writer {
    /// Implement the `write_str` function from the `fmt::Write` trait to be able to
    /// support the write! and writeln! macros.
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_string(s);
        Ok(())
    }
}
