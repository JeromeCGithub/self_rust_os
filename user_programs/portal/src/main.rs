//! Portal: graphical terminal window for `self_rust_os`, built on `userlib`.
//!
//! Draws a window (position from `portal.pos` file, written by the WM),
//! and runs a mini-shell inside. Uses 32bpp framebuffer.

#![no_std]
#![no_main]

use userlib::{println, sys};
use font8x8::{UnicodeFonts, BASIC_FONTS};

/// Framebuffer base address (mapped by kernel).
const FB_ADDR: usize = 0x90_0000;

/// Global framebuffer geometry (set at startup).
static mut FB_WIDTH: i32 = 1280;
static mut FB_HEIGHT: i32 = 720;
static mut FB_PITCH: i32 = 1280 * 4;

/// Window dimensions (must match WM).
const WIN_W: i32 = 640;
const WIN_H: i32 = 400;
const TITLE_H: i32 = 32;

/// Content area (inside window frame).
const CONTENT_X_OFF: i32 = 4;
const CONTENT_Y_OFF: i32 = TITLE_H + 4;

/// Font dimensions (2x scaled 8x8 for readability at 720p).
const FONT_W: i32 = 16;
const FONT_H: i32 = 16;

/// Colors (32-bit XRGB).
const BLACK: u32 = 0x000000;
const GREEN: u32 = 0x00FF00;
const WHITE: u32 = 0xFFFFFF;

/// Position file (written by WM).
const POS_FILE: &str = "portal.pos";

/// Sets a pixel (with bounds check). 32bpp.
fn set_pixel(x: i32, y: i32, color: u32) {
    unsafe {
        if x >= 0 && x < FB_WIDTH && y >= 0 && y < FB_HEIGHT {
            let offset = (y * FB_PITCH + x * 4) as usize;
            core::ptr::write((FB_ADDR as *mut u8).add(offset) as *mut u32, color);
        }
    }
}

/// Fills a rectangle.
fn fill_rect(x: i32, y: i32, w: i32, h: i32, color: u32) {
    for dy in 0..h {
        for dx in 0..w {
            set_pixel(x + dx, y + dy, color);
        }
    }
}

/// Draws a character at (x, y) using the 8x8 font, scaled 2x.
fn draw_char(x: i32, y: i32, c: char, color: u32) {
    if let Some(glyph) = BASIC_FONTS.get(c) {
        for (row, byte) in glyph.iter().enumerate() {
            for col in 0..8 {
                if (byte >> col) & 1 != 0 {
                    // 2x scale: each pixel becomes 2x2.
                    let px = x + (col as i32) * 2;
                    let py = y + (row as i32) * 2;
                    fill_rect(px, py, 2, 2, color);
                }
            }
        }
    }
}

/// Draws a string at (x, y). Returns the x position after the last char.
fn draw_str(x: i32, y: i32, s: &str, color: u32) -> i32 {
    let mut cx = x;
    for c in s.chars() {
        draw_char(cx, y, c, color);
        cx += FONT_W;
    }
    cx
}

/// Reads the window position from the pos file. Returns (x, y).
fn read_pos() -> (i32, i32) {
    let mut buf = [0u8; 16];
    match sys::read_file(POS_FILE, &mut buf) {
        Some(n) => {
            let s = core::str::from_utf8(&buf[..n]).unwrap_or("100 80");
            let mut parts = s.split_whitespace();
            let x = parts.next().and_then(|p| p.parse().ok()).unwrap_or(100);
            let y = parts.next().and_then(|p| p.parse().ok()).unwrap_or(80);
            (x, y)
        }
        None => (100, 80),
    }
}

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

        let mut echo = [0u8; 64];
        echo[0] = b'>';
        echo[1] = b' ';
        let len = cmd_len.min(62);
        echo[2..2 + len].copy_from_slice(&cmd[..len]);
        self.push_line(&echo[..2 + len]);

        if cmd == b"help" {
            self.push_line(b"Commands: help, echo, clear");
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
#[no_mangle]
pub extern "C" fn user_main() {
    println!("[portal] Portal starting...");

    // Get framebuffer geometry.
    if let Ok(info) = sys::fb_info() {
        unsafe {
            FB_WIDTH = info.width as i32;
            FB_HEIGHT = info.height as i32;
            FB_PITCH = info.pitch as i32;
        }
        println!(
            "[portal] Framebuffer: {}x{}",
            info.width, info.height
        );
    }

    let mut shell = Shell::new();
    shell.push_line(b"portal v1.0 - mini shell (720p)");
    shell.push_line(b"Type 'help' for commands");

    let mut char_buf = [0u8; 4];

    loop {
        let (wx, wy) = read_pos();
        let cx = wx + CONTENT_X_OFF;
        let cy = wy + CONTENT_Y_OFF;
        let cw = WIN_W - 8;
        let ch = WIN_H - TITLE_H - 8;

        // Draw content background.
        fill_rect(cx, cy, cw, ch, BLACK);

        // Draw shell lines.
        for (i, line) in shell.lines[..shell.line_count].iter().enumerate() {
            let y = cy + 8 + (i as i32 * FONT_H);
            if y + FONT_H > cy + ch {
                break;
            }
            let len = line.iter().position(|&b| b == 0).unwrap_or(64);
            let s = core::str::from_utf8(&line[..len]).unwrap_or("");
            draw_str(cx + 8, y, s, GREEN);
        }

        // Draw input line with cursor.
        let input_y = cy + 8 + (shell.line_count as i32 * FONT_H);
        if input_y + FONT_H <= cy + ch {
            draw_str(cx + 8, input_y, "> ", GREEN);
            let input_str = core::str::from_utf8(&shell.input[..shell.input_len]).unwrap_or("");
            let end_x = draw_str(cx + 8 + 32, input_y, input_str, WHITE);
            if (sys::ticks() / 9) % 2 == 0 {
                fill_rect(end_x, input_y, 16, FONT_H, WHITE);
            }
        }

        // Handle keyboard input (non-blocking).
        let n = sys::read_nb(sys::STDIN_FD, &mut char_buf);
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

        let start = sys::ticks();
        while sys::ticks() - start < 1 {}
    }
}
