//! Terminal text storage, independent of shell command syntax and window rendering.
pub const COLS: usize = 58;
pub const ROWS: usize = 22;
pub struct Screen {
    pub lines: [[u8; COLS]; ROWS],
    pub row: usize,
    pub column: usize,
}
impl Screen {
    pub fn new() -> Self {
        Self {
            lines: [[0; COLS]; ROWS],
            row: 0,
            column: 0,
        }
    }
    pub fn put(&mut self, byte: u8) {
        match byte {
            b'\n' => self.newline(),
            b'\r' => self.column = 0,
            8 | 127 => self.column = self.column.saturating_sub(1),
            32..=126 => {
                if self.column == COLS {
                    self.newline();
                }
                self.lines[self.row][self.column] = byte;
                self.column += 1;
            }
            _ => {}
        }
    }
    fn newline(&mut self) {
        self.column = 0;
        if self.row == ROWS - 1 {
            self.lines.copy_within(1..ROWS, 0);
            self.lines[ROWS - 1] = [0; COLS];
        } else {
            self.row += 1;
        }
    }
    pub fn current(&self) -> &[u8] {
        let line = &self.lines[self.row];
        &line[..line
            .iter()
            .rposition(|b| *b != 0 && *b != b' ')
            .map(|n| n + 1)
            .unwrap_or(0)]
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn output_wraps_scrolls_and_preserves_the_cursor() {
        let mut screen = Screen::new();
        for _ in 0..COLS + 1 {
            screen.put(b'x');
        }
        assert_eq!((screen.row, screen.column), (1, 1));
        for _ in 0..ROWS - 1 {
            screen.put(b'\n');
        }
        assert_eq!(screen.row, ROWS - 1);
        assert_eq!(screen.column, 0);
        assert_eq!(screen.lines[0][0], b'x');
    }
    #[test]
    fn backspace_echo_and_carriage_return_edit_text_without_commands() {
        let mut screen = Screen::new();
        for &b in b"> abc\x08 \x08d" {
            screen.put(b);
        }
        assert_eq!(screen.current(), b"> abd");
        screen.put(b'\r');
        screen.put(b'X');
        assert_eq!(screen.current(), b"X abd");
        screen.put(0xff);
        assert_eq!(screen.current(), b"X abd");
    }
}
