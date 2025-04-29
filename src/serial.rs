use lazy_static::lazy_static;
use spin::Mutex;
use uart_16550::SerialPort;

lazy_static! {
    pub static ref SERIAL1: Mutex<SerialPort> = {
        // SAFETY:
        // Need to map a special defined serial port
        let mut serial_port = unsafe { SerialPort::new(0x3F8) };
        serial_port.init();
        Mutex::new(serial_port)
    };
}

#[doc(hidden)]
pub fn _print(args: ::core::fmt::Arguments) {
    use core::fmt::Write;

    #[expect(
        clippy::expect_used,
        reason = "Serial port should be correctly initialized."
    )]
    SERIAL1
        .lock()
        .write_fmt(args)
        .expect("Failed to print on serial port.");
}

/// Print to host machine through serial port interface.
#[macro_export]
macro_rules! serial_print {
    ($($args:tt)*) => {
        $crate::serial::_print(format_args!($($args)*));
    };
}

/// Print to host machine through serial port interface a line.
#[macro_export]
macro_rules! serial_println {
    () => ($crate::serial_print!("\n"));
    ($fmt:expr) => ($crate::serial_print!(concat!($fmt, "\n")));
    ($fmt:expr, $($arg:tt)*) => ($crate::serial_print!(
        concat!($fmt, "\n"), $($arg)*));
}
