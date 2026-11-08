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

#[cfg(feature = "alloc")]
mod cancel;
#[cfg(feature = "alloc")]
pub use cancel::Cancellation;

mod record;
/// Format a bounded diagnostic before writing. Formatting failure emits nothing.
/// Unconnected console records use one syscall; connected stdio retains its own
/// packet/stream semantics and does not gain cross-process atomicity.
pub fn record(args: fmt::Arguments<'_>) -> fmt::Result {
    let mut bytes = [0; 256];
    let bytes = record::format(&mut bytes, args)?;
    if crate::sys::console_write(bytes) == bytes.len() { Ok(()) } else { Err(fmt::Error) }
}
/// Best-effort diagnostic line, bounded to 256 UTF-8 bytes including newline.
/// Oversized records are dropped whole; use io::record to handle errors directly.
#[macro_export]
macro_rules! recordln {
    ($($arg:tt)*) => {{
        let _ = $crate::io::record(format_args!("{}\n", format_args!($($arg)*)));
    }};
}
