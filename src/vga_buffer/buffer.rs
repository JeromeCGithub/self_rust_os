use volatile::Volatile;

use super::{colors::ColorCode, constants::{BUFFER_HEIGHT, BUFFER_WIDTH}};


/// Represent the VGA buffer.
pub struct Buffer {
    pub chars: [[Volatile<ScreenChar>; BUFFER_WIDTH]; BUFFER_HEIGHT],
}

/// Represent a character on the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct ScreenChar {
    pub ascii_character: u8,
    pub color_code: ColorCode,
}