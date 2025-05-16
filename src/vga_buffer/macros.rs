//! Macros for printing to the screen.
//! Provides a safe interface to write on the VGA text buffer with println macro.

use core::fmt;

use crate::vga_buffer::writer::WRITER;

////////////////////////
//    Print macros    //
////////////////////////

/// Copy of the std print! macro that redirect our internal _print function.
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::vga_buffer::macros::_print(format_args!($($arg)*)));
}

/// Copy of the std println! macro that redirect our internal _print function.
#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}

/// Macro that allow to print to the screen with the WRITER static instance.
#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;
    #[expect(
        clippy::unwrap_used,
        reason = "We should be able to write inside the vga frame buffer"
    )]
    WRITER.lock().write_fmt(args).unwrap();
}
