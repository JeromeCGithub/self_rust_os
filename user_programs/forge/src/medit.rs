// Terminal interaction for the bounded modal editor. Runs within Forge so it
// uses the same connected standard I/O in Portal and in standalone Forge.
use super::editor::{Action, Editor, CAPACITY};
use userlib::{
    fs::{File, O_CREAT, O_RDWR},
    println, sys,
};

fn render(editor: &Editor, path: &str) {
    // Portal's text protocol intentionally has no ANSI escape parser. Redraw a
    // bounded page using scrolling and show the cursor with a vertical bar.
    sys::console_write(b"\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n");
    println!("medit: {}", path);
    let line_start = editor.data[..editor.cursor]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |n| n + 1);
    let mut start = line_start;
    for _ in 0..3 {
        if start == 0 {
            break;
        }
        start = editor.data[..start - 1]
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(0, |n| n + 1);
    }
    for _ in 0..8 {
        let end = editor.data[start..editor.len]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(editor.len, |n| start + n);
        let active = editor.cursor >= start && editor.cursor <= end;
        let left = if active {
            (editor.cursor - start).saturating_sub(45)
        } else {
            0
        };
        let mut row = [b' '; 54];
        row[0] = if active { b'>' } else { b' ' };
        let mut n = 1;
        let visible_end = end.min(start + left + 49);
        for index in start + left..=visible_end {
            if active && index == editor.cursor {
                row[n] = b'|';
                n += 1;
            }
            if index < visible_end {
                row[n] = if editor.data[index] == b'\t' {
                    b' '
                } else {
                    editor.data[index]
                };
                n += 1;
            }
        }
        row[n] = b'\n';
        sys::console_write(&row[..n + 1]);
        if end == editor.len {
            break;
        }
        start = end + 1;
    }
    if let Some(n) = editor.command_len {
        sys::console_write(b":");
        sys::console_write(&editor.command[..n]);
        println!();
    } else {
        println!(
            "-- {} -- {} bytes{}",
            if editor.insert { "INSERT" } else { "NORMAL" },
            editor.len,
            if editor.dirty { " [+]" } else { "" }
        );
    }
    println!("i/a insert  Esc normal  hjkl move  x delete");
    println!(":w save  :q quit  :wq save+quit  :q! discard");
    if !editor.message.is_empty() {
        println!("{}", editor.message);
    }
}

pub fn run(path: &str) {
    if path.is_empty() {
        println!("usage: medit <file>");
        return;
    }
    let mut editor = Editor::new();
    // Opening a missing file creates an empty file; I/O errors and directories
    // must not be mistaken for an editable empty document.
    let Ok(mut file) = File::open(path, O_RDWR | O_CREAT) else {
        println!("medit: open failed");
        return;
    };
    loop {
        match file.read(&mut editor.data[editor.len..]) {
            Ok(0) => break,
            Ok(n) => editor.len += n,
            Err(()) => {
                println!("medit: read failed");
                return;
            }
        }
        if editor.len == CAPACITY {
            let mut extra = [0];
            match file.read(&mut extra) {
                Ok(0) => break,
                _ => {
                    println!("medit: file exceeds 16 KiB");
                    return;
                }
            }
        }
    }
    drop(file);
    if editor.data[..editor.len]
        .iter()
        .any(|&b| b != b'\n' && b != b'\t' && !(32..=126).contains(&b))
    {
        println!("medit: only ASCII text with LF newlines is supported");
        return;
    }
    println!("medit: {}", path);
    render(&editor, path);
    let mut input = [0];
    loop {
        if sys::read(sys::STDIN_FD, &mut input) != 1 {
            continue;
        }
        let action = editor.key(input[0]);
        match action {
            Action::Quit => break,
            Action::Save | Action::SaveQuit => {
                // Whole-file replacement commits the FAT entry after staging
                // data, unlike O_TRUNC followed by a possibly failing write.
                if sys::write_file(path, &editor.data[..editor.len]) == Some(editor.len) {
                    editor.dirty = false;
                    println!("medit: saved {} bytes", editor.len);
                    if action == Action::SaveQuit {
                        break;
                    }
                    editor.message = "saved";
                } else {
                    editor.message = "save failed; buffer retained";
                }
            }
            Action::None => (),
        }
        render(&editor, path);
    }
    println!("medit: closed");
}
