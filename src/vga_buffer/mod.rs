//! VGA buffer management module.
//! It provides a safe interface to write to the VGA text buffer.

#![allow(unused_imports, reason = "Imports are used in test cases.")]
use core::fmt::Write;
use writer::WRITER;
use x86_64::instructions::interrupts;

mod buffer;
mod colors;
mod constants;
#[macro_use]
pub mod macros;
mod writer;

/// Test that printing many lines will not panic.
#[test_case]
fn test_when_printing_many_lines_should_not_panic() {
    for _ in 0..200 {
        println!("test many print output.");
    }
}

/// Test that a line a line will be printed on the screen.
/// # Panics
/// Fail if the line is not printed on the screen.
#[test_case]
fn test_when_printing_a_line_should_appear_in_vga_buffer() {
    let line = "Some line that fits on a single line";

    interrupts::without_interrupts(|| {
        let mut writer = WRITER.lock();

        #[expect(
            clippy::uninlined_format_args,
            reason = "This is a test, we want to use format_args!"
        )]
        writeln!(writer, "\n{}", line).expect("Failed to write line to vga buffer.");
        for (i, c) in line.chars().enumerate() {
            let screen_char = writer.buffer.chars[constants::BUFFER_HEIGHT - 2][i].read();
            assert_eq!(
                char::from(screen_char.ascii_character),
                c,
                "Character mismatch at position {i}.",
            );
        }
    });
}
