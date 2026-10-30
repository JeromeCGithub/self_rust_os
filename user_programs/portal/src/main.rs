// Portal: graphical terminal window for `self_rust_os`, built on `userlib`.
//
// Renders a shell into a private surface; the compositor owns window
// decorations, placement, focus, and presentation.


use userlib::{
    println, sys,
    ui::{self, Color, Event, Material, Rect, Window, WindowOptions},
};

const WIN_W: i32 = 488;
const WIN_H: i32 = 288;
const FONT_H: i32 = 12;
const GREEN: Color = Color::rgb(0xbed5e8);
const WHITE: Color = Color::rgb(0xffffff);

/// Simple line buffer for the shell.
struct Shell {
    lines: [[u8; 64]; 20], // 20 lines, 64 chars each
    line_count: usize,
    input: [u8; 64],
    input_len: usize,
}

impl Shell {
    fn new() -> Self {
        Self {
            lines: [[0; 64]; 20],
            line_count: 0,
            input: [0; 64],
            input_len: 0,
        }
    }

    fn push_line(&mut self, s: &[u8]) {
        println!(
            "[portal] output: {}",
            core::str::from_utf8(s).unwrap_or("?")
        );
        if self.line_count >= 20 {
            for i in 1..20 {
                self.lines[i - 1] = self.lines[i];
            }
            self.line_count = 19;
        }
        let len = s.len().min(64);
        self.lines[self.line_count][..len].copy_from_slice(&s[..len]);
        for i in len..64 {
            self.lines[self.line_count][i] = 0;
        }
        self.line_count += 1;
    }

    fn handle_command(&mut self) {
        let mut cmd_buf = [0u8; 64];
        let cmd_len = self.input_len.min(64);
        cmd_buf[..cmd_len].copy_from_slice(&self.input[..cmd_len]);
        let cmd = &cmd_buf[..cmd_len];
        println!(
            "[portal] command: {}",
            core::str::from_utf8(cmd).unwrap_or("?")
        );

        let mut echo = [0u8; 64];
        echo[0] = b'>';
        echo[1] = b' ';
        let len = cmd_len.min(62);
        echo[2..2 + len].copy_from_slice(&cmd[..len]);
        self.push_line(&echo[..2 + len]);

        if cmd == b"help" {
            self.push_line(b"help echo clear pwd ls cat write mkdir time exit");
        } else if cmd == b"pwd" {
            self.push_line(b"/");
        } else if cmd == b"exit" {
            sys::exit(0);
        } else if cmd == b"time" {
            let mut buf = [0u8; 64];
            use core::fmt::Write;
            let mut out = Line {
                buf: &mut buf,
                len: 0,
            };
            let _ = write!(out, "{} ticks since boot", sys::ticks());
            let n = out.len;
            self.push_line(&buf[..n]);
        } else if cmd == b"ls" || cmd.starts_with(b"ls ") {
            let path = core::str::from_utf8(cmd.get(3..).unwrap_or(b"/")).unwrap_or("/");
            let mut entries = [0u8; 384];
            if let Some(n) = sys::ls(path, &mut entries, 32) {
                for e in entries.chunks_exact(12).take(n.min(32)) {
                    let mut name = [0u8; 16];
                    let mut len = 0;
                    for &b in &e[..8] {
                        if b != b' ' {
                            name[len] = b;
                            len += 1;
                        }
                    }
                    if e[8] != b' ' {
                        name[len] = b'.';
                        len += 1;
                        for &b in &e[8..11] {
                            if b != b' ' {
                                name[len] = b;
                                len += 1;
                            }
                        }
                    }
                    if e[11] & 0x10 != 0 {
                        name[len] = b'/';
                        len += 1;
                    }
                    self.push_line(&name[..len]);
                }
            } else {
                self.push_line(b"ls: directory not found");
            }
        } else if cmd.starts_with(b"cat ") {
            let mut buf = [0u8; 1024];
            let path = core::str::from_utf8(&cmd[4..]).unwrap_or("");
            if let Some(n) = sys::read_file(path, &mut buf) {
                for line in buf[..n].split(|b| *b == b'\n') {
                    self.push_line(line);
                }
            } else {
                self.push_line(b"cat: file not found");
            }
        } else if cmd.starts_with(b"mkdir ") {
            let path = core::str::from_utf8(&cmd[6..]).unwrap_or("");
            self.push_line(if sys::mkdir(path).is_ok() {
                b"directory created"
            } else {
                b"mkdir: failed"
            });
        } else if cmd.starts_with(b"write ") {
            if let Some(end) = cmd[6..].iter().position(|b| *b == b' ') {
                let path = core::str::from_utf8(&cmd[6..6 + end]).unwrap_or("");
                self.push_line(if sys::write_file(path, &cmd[7 + end..]).is_some() {
                    b"file written"
                } else {
                    b"write: failed"
                });
            } else {
                self.push_line(b"usage: write /FILE.TXT text");
            }
        } else if cmd == b"clear" {
            self.line_count = 0;
        } else if cmd_len > 5 && &cmd[..5] == b"echo " {
            self.push_line(&cmd[5..]);
        } else if cmd.is_empty() {
        } else {
            self.push_line(b"unknown command");
        }

        self.input_len = 0;
    }
}

/// Entry point.
fn main() {
    println!("[portal] Portal starting...");
    let mut window = Window::create(
        WindowOptions::new("Terminal")
            .content_size(WIN_W as usize, WIN_H as usize)
            .background(Material::Glass),
    )
    .expect("create terminal window");

    let mut shell = Shell::new();
    shell.push_line(b"Portal / RustOS shell");
    shell.push_line(b"Type 'help' for commands");

    let mut char_buf = [0u8; 4];

    let mut dirty = true;
    let mut previous_blink = false;
    loop {
        for _ in 0..16 {
            if window.poll_event().expect("terminal events") == Some(Event::CloseRequested) {
                sys::exit(0);
            }
        }
        let blink = (sys::ticks() / 9) % 2 == 0;
        dirty |= blink != previous_blink;
        // Bound each input batch so an active typist cannot starve rendering.
        for _ in 0..64 {
            let n = sys::read_nb(sys::STDIN_FD, &mut char_buf);
            if n == 0 {
                break;
            }
            dirty = true;
            if n > 0 {
                let c = char_buf[0];
                if c == b'\n' || c == b'\r' {
                    shell.handle_command();
                } else if c == 8 || c == 127 {
                    if shell.input_len > 0 {
                        shell.input_len -= 1;
                    }
                } else if shell.input_len < 64 && c >= 32 && c < 127 {
                    shell.input[shell.input_len] = c;
                    shell.input_len += 1;
                }
            }
        }
        if dirty {
            window
                .draw(|canvas| {
                    // Transparent content lets the WM material show through. Text
                    // stays opaque and sharp, independent of the backdrop blur.
                    for (i, line) in shell.lines[..shell.line_count].iter().enumerate() {
                        let y = 12 + i as i32 * FONT_H;
                        if y + FONT_H > WIN_H - 4 {
                            break;
                        }
                        let len = line.iter().position(|&b| b == 0).unwrap_or(64);
                        let line = core::str::from_utf8(&line[..len]).unwrap_or("");
                        ui::text(canvas, 12, y, line, GREEN, 1);
                    }
                    let input_y = 12 + shell.line_count as i32 * FONT_H;
                    if input_y + FONT_H <= WIN_H - 4 {
                        ui::text(canvas, 12, input_y, "> ", GREEN, 1);
                        let input =
                            core::str::from_utf8(&shell.input[..shell.input_len]).unwrap_or("");
                        let end = ui::text(canvas, 28, input_y, input, WHITE, 1);
                        if blink {
                            canvas.fill_rect(Rect::new(end, input_y, 8, FONT_H as u32), WHITE);
                        }
                    }
                })
                .expect("terminal frame");
            dirty = false;
        }
        previous_blink = blink;
        sys::sleep_ticks(1).expect("terminal timed wait");
    }
}

struct Line<'a> {
    buf: &'a mut [u8],
    len: usize,
}
impl core::fmt::Write for Line<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let n = s.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}
