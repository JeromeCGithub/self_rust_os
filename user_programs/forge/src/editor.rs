// Bounded ASCII modal text model; independent of terminal and filesystem I/O.
pub const CAPACITY: usize = 16 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Save,
    Quit,
    SaveQuit,
}
pub struct Editor {
    pub data: [u8; CAPACITY],
    pub len: usize,
    pub cursor: usize,
    pub dirty: bool,
    pub insert: bool,
    pub command: [u8; 16],
    pub command_len: Option<usize>,
    pub message: &'static str,
}
impl Editor {
    pub fn new() -> Self {
        Self {
            data: [0; CAPACITY],
            len: 0,
            cursor: 0,
            dirty: false,
            insert: false,
            command: [0; 16],
            command_len: None,
            message: "",
        }
    }
    fn start(&self) -> usize {
        self.data[..self.cursor]
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(0, |n| n + 1)
    }
    fn end(&self) -> usize {
        self.data[self.cursor..self.len]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(self.len, |n| self.cursor + n)
    }
    fn remove(&mut self) {
        if self.cursor < self.len {
            self.data
                .copy_within(self.cursor + 1..self.len, self.cursor);
            self.len -= 1;
            self.dirty = true;
        }
    }
    fn add(&mut self, byte: u8) {
        if self.len == CAPACITY {
            self.message = "file limit: 16 KiB";
            return;
        }
        self.data
            .copy_within(self.cursor..self.len, self.cursor + 1);
        self.data[self.cursor] = byte;
        self.len += 1;
        self.cursor += 1;
        self.dirty = true;
    }
    pub fn key(&mut self, byte: u8) -> Action {
        self.message = "";
        if let Some(length) = self.command_len {
            match byte {
                27 => self.command_len = None,
                8 | 127 => self.command_len = Some(length.saturating_sub(1)),
                b'\n' | b'\r' => {
                    self.command_len = None;
                    return match &self.command[..length] {
                        b"w" => Action::Save,
                        b"wq" => Action::SaveQuit,
                        b"q!" => Action::Quit,
                        b"q" if !self.dirty => Action::Quit,
                        b"q" => {
                            self.message = "unsaved changes; use :w or :q!";
                            Action::None
                        }
                        _ => {
                            self.message = "unknown command";
                            Action::None
                        }
                    };
                }
                32..=126 if length < self.command.len() => {
                    self.command[length] = byte;
                    self.command_len = Some(length + 1);
                }
                _ => (),
            }
            return Action::None;
        }
        if self.insert {
            match byte {
                27 => {
                    self.insert = false;
                    if self.cursor > self.start() {
                        self.cursor -= 1;
                    }
                }
                8 | 127 if self.cursor > 0 => {
                    self.cursor -= 1;
                    self.remove();
                }
                b'\r' | b'\n' => self.add(b'\n'),
                b'\t' => self.add(b'\t'),
                32..=126 => self.add(byte),
                _ => (),
            }
            return Action::None;
        }
        match byte {
            b'i' => self.insert = true,
            b'a' => {
                if self.cursor < self.end() {
                    self.cursor += 1;
                }
                self.insert = true;
            }
            b'h' => self.cursor = self.cursor.saturating_sub(1).max(self.start()),
            b'l' => self.cursor = (self.cursor + 1).min(self.end()),
            b'0' => self.cursor = self.start(),
            b'$' => self.cursor = self.end().saturating_sub(1).max(self.start()),
            b'k' => {
                let start = self.start();
                if start > 0 {
                    let previous = self.data[..start - 1]
                        .iter()
                        .rposition(|&b| b == b'\n')
                        .map_or(0, |n| n + 1);
                    self.cursor = (previous + self.cursor - start).min(start - 1);
                }
            }
            b'j' => {
                let end = self.end();
                if end < self.len {
                    let column = self.cursor - self.start();
                    self.cursor = end + 1;
                    self.cursor = (self.cursor + column).min(self.end());
                }
            }
            b'x' => self.remove(),
            b':' => self.command_len = Some(0),
            _ => (),
        }
        Action::None
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn keys(editor: &mut Editor, input: &[u8]) -> Action {
        let mut action = Action::None;
        for &b in input {
            action = editor.key(b);
        }
        action
    }
    #[test]
    fn insert_move_delete_and_save_quit() {
        let mut e = Editor::new();
        assert_eq!(keys(&mut e, b"iabc\nxyz\x1bhx"), Action::None);
        assert_eq!(&e.data[..e.len], b"abc\nxz");
        assert!(e.dirty);
        assert_eq!(keys(&mut e, b":wq\n"), Action::SaveQuit);
    }
    #[test]
    fn dirty_quit_is_refused_and_force_quit_preserves_bytes() {
        let mut e = Editor::new();
        keys(&mut e, b"iunsaved\x1b");
        assert_eq!(keys(&mut e, b":q\n"), Action::None);
        assert_eq!(e.message, "unsaved changes; use :w or :q!");
        assert_eq!(keys(&mut e, b":q!\n"), Action::Quit);
        assert_eq!(&e.data[..e.len], b"unsaved");
    }
    #[test]
    fn empty_boundaries_vertical_motion_and_backspace() {
        let mut e = Editor::new();
        keys(&mut e, b"hkjx0$");
        assert_eq!((e.len, e.cursor, e.dirty), (0, 0, false));
        keys(&mut e, b"ione\ntwo\x08!\x1bk0xj$a?\x08\x1b");
        assert_eq!(&e.data[..e.len], b"ne\ntw!");
        assert!(e.cursor <= e.len);
        assert_eq!(keys(&mut e, b":bad\n"), Action::None);
        assert_eq!(e.message, "unknown command");
    }
    #[test]
    fn capacity_and_command_limits_do_not_drop_existing_text() {
        let mut e = Editor::new();
        e.data.fill(b'a');
        e.len = CAPACITY;
        e.cursor = CAPACITY;
        keys(&mut e, b"ib");
        assert_eq!(e.len, CAPACITY);
        assert!(!e.dirty);
        keys(&mut e, b"\x08b\x1b:");
        for _ in 0..100 {
            e.key(b'z');
        }
        assert!(e.command_len.unwrap() <= e.command.len());
        assert_eq!(e.data[CAPACITY - 1], b'b');
        assert_eq!(keys(&mut e, b"\x1b:q!\n"), Action::Quit);
    }
}
