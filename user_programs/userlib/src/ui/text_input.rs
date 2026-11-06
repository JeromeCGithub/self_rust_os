//! Bounded single-line UTF-8 editor, independent of window transport.
extern crate alloc;
use alloc::string::String;
#[derive(Default)]
pub struct TextInput {
    text: String,
    cursor: usize,
    selected: bool,
}
impl TextInput {
    pub fn new(text: &str) -> Self {
        let mut input = Self::default();
        input.set(text);
        input
    }
    pub fn as_str(&self) -> &str {
        &self.text
    }
    pub fn cursor(&self) -> usize {
        self.cursor
    }
    pub fn is_selected(&self) -> bool {
        self.selected
    }
    pub fn select_all(&mut self) {
        self.selected = true;
    }
    /// Replace only valid single-line text; reject oversized values atomically.
    pub fn set(&mut self, text: &str) {
        if text.len() <= 2048 && !text.chars().any(char::is_control) {
            self.text.clear();
            self.text.push_str(text);
            self.cursor = self.text.len();
            self.selected = false;
        }
    }
    /// Printable ASCII, Backspace/Delete, Ctrl-A (home), Ctrl-E (end).
    /// Transport-specific key decoding stays outside this reusable model.
    pub fn byte(&mut self, byte: u8) -> bool {
        if byte == 1 {
            self.cursor = 0;
            self.selected = false;
            return true;
        }
        if byte == 5 {
            self.cursor = self.text.len();
            self.selected = false;
            return true;
        }
        if byte != 8 && byte != 127 && !(32..127).contains(&byte) {
            return false;
        }
        if self.selected {
            self.text.clear();
            self.cursor = 0;
            self.selected = false;
        }
        if byte == 8 || byte == 127 {
            if self.cursor > 0 {
                let previous = self.text[..self.cursor]
                    .char_indices()
                    .next_back()
                    .map(|(at, _)| at)
                    .unwrap_or(0);
                self.text.drain(previous..self.cursor);
                self.cursor = previous;
            }
        } else if self.text.len() < 2048 {
            self.text.insert(self.cursor, byte as char);
            self.cursor += 1;
        } else {
            return false;
        }
        true
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn edits_selection_backspace_cursor_and_bounded_input() {
        let mut input = TextInput::new("https://host/");
        input.select_all();
        input.byte(b'a');
        input.byte(b'b');
        input.byte(8);
        assert_eq!(input.as_str(), "a");
        input.byte(b'c');
        input.byte(1);
        input.byte(b'b');
        assert_eq!(input.as_str(), "bac");
        input.byte(5);
        input.byte(8);
        assert_eq!(input.as_str(), "ba");
        input.set("é");
        input.byte(8);
        assert_eq!(input.as_str(), "");
        for _ in 0..3000 {
            input.byte(b'x');
        }
        assert_eq!(input.as_str().len(), 2048);
        assert!(!input.byte(0));
        assert!(!input.byte(b'\n'));
        input.select_all();
        input.byte(8);
        assert_eq!(input.as_str(), "");
    }
    #[test]
    fn invalid_and_oversized_set_preserves_the_previous_value() {
        let mut input = TextInput::new("old");
        input.set("bad\nurl");
        assert_eq!(input.as_str(), "old");
        input.set(&"x".repeat(2049));
        assert_eq!(input.as_str(), "old");
        input.set("new");
        assert_eq!(input.cursor(), 3);
    }
}
