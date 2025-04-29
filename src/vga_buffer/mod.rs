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

#[test_case]
fn test_when_printing_a_line_should_appear_in_vga_buffer() {
    let line = "Some line that fits on a single line";

    println!("{}", line);

    for (i, c) in line.chars().enumerate() {
        let screen_char =
            writer::WRITER.lock().buffer.chars[constants::BUFFER_HEIGHT - 2][i].read();
        assert_eq!(
            char::from(screen_char.ascii_character),
            c,
            "Check if the character appear on vga buffer."
        );
    }
}
