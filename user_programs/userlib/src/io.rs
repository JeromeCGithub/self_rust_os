//! Console I/O: `print!` and `println!` macros.
//!
//! These format through [`core::fmt`] and emit the result with the
//! `SYS_WRITE` syscall, so `println!("ticks: {}", sys::ticks())` just works.

use core::fmt::{self, Write};

/// Console writer used by the `print!`/`println!` macros.
pub struct Console;

impl Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        crate::sys::console_write(s.as_bytes());
        Ok(())
    }
}

/// Writes formatted output to the console.
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let _ = write!($crate::io::Console, $($arg)*);
    }};
}

/// Writes formatted output to the console, followed by a newline.
#[macro_export]
macro_rules! println {
    () => {
        $crate::print!("\n")
    };
    ($($arg:tt)*) => {{
        $crate::print!($($arg)*);
        $crate::print!("\n");
    }};
}
