////////////////////////
//   Color Selection  //
////////////////////////

/// Define a color for the text displayed on the screen.
#[expect(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Color {
    Black = 0,
    Blue = 1,
    Green = 2,
    Cyan = 3,
    Red = 4,
    Magenta = 5,
    Brown = 6,
    LightGray = 7,
    DarkGray = 8,
    LightBlue = 9,
    LightGreen = 10,
    LightCyan = 11,
    LightRed = 12,
    Pink = 13,
    Yellow = 14,
    White = 15,
}

/// Repesente a `ColorCode` that is a combination of a foreground and a background color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct ColorCode(u8);

impl ColorCode {
    /// Create a new `ColorCode` with a foreground and a background color.
    pub const fn new(foreground: Color, background: Color) -> Self {
        Self(((background as u8) << 4) | (foreground as u8))
    }
}
