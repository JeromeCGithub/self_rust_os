//! Portal: graphical terminal window for `self_rust_os`, built on `userlib`.
//!
//! Draws a window (position from `/portal.pos` file, written by the WM),
//! and runs a mini-shell inside. For Phase 1, the shell is built-in.

#![no_std]
#![no_main]

use userlib::{println, sys};
use font8x8::{UnicodeFonts, BASIC_FONTS};

/// Framebuffer: 320x200, 8-bit color at 0x900000.
const FB_ADDR: usize = 0x90_0000;
const FB_WIDTH: i32 = 320;
const FB_HEIGHT: i32 = 200;

/// Window dimensions (must match WM).
const WIN_W: i32 = 220;
const WIN_H: i32 = 140;
const TITLE_H: i32 = 16;

/// Content area (inside window frame).
const CONTENT_X_OFF: i32 = 2;
const CONTENT_Y_OFF: i32 = TITLE_H + 2;

/// Font dimensions.
const FONT_W: i32 = 8;
const FONT_H: i32 = 8;

/// Colors.
const BLACK: u8 = 0;
const GREEN: u8 = 2;
const WHITE: u8 = 15;

/// Position file (written by WM).
const POS_FILE: &str = "portal.pos";

/// Sets a pixel (with bounds check).
fn set_pixel(x: i32, y: i32, color: u8) {
    if x >= 0 && x < FB_WIDTH && y >= 0 && y < FB_HEIGHT {
        unsafe {
            core::ptr::write((FB_ADDR as *mut u8).add((y * FB_WIDTH + x) as usize), color);
        }
    }
}

/// Fills a rectangle.
fn fill_rect(x: i32, y: i32, w: i32, h: i32, color: u8) {
    for dy in 0..h {
        for dx in 0..w {
            set_pixel(x + dx, y + dy, color);
        }
    }
}

/// Draws a character at (x, y) using the 8x8 font.
fn draw_char(x: i32, y: i32, c: char, color: u8) {
    if let Some(glyph) = BASIC_FONTS.get(c) {
        for (row, byte) in glyph.iter().enumerate() {
            for col in 0..8 {
                if (byte >> col) & 1 != 0 {
                    set_pixel(x + col, y + row as i32, color);
                }
            }
        }
    }
}

/// Draws a string at (x, y). Returns the x position after the last char.
fn draw_str(x: i32, y: i32, s: &str, color: u8) -> i32 {
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
            let s = core::str::from_utf8(&buf[..n]).unwrap_or("50 40");
            let mut parts = s.split_whitespace();
            let x = parts.next().and_then(|p| p.parse().ok()).unwrap_or(50);
            let y = parts.next().and_then(|p| p.parse().ok()).unwrap_or(40);
            (x, y)
        }
        None => (50, 40),
    }
}

/// Simple line buffer for the shell.
struct Shell {
    lines: [[u8; 32]; 12], // 12 lines, 32 chars each (fits in 220px / 8 = 27 chars)
    line_count: usize,
    input: [u8; 32],
    input_len: usize,
}

impl Shell {
    fn new() -> Self {
        Self {
            lines: [[0; 32]; 12],
            line_count: 0,
            input: [0; 32],
            input_len: 0,
        }
    }
    
    fn push_line(&mut self, s: &[u8]) {
        if self.line_count >= 12 {
            // Scroll: shift lines up.
            for i in 1..12 {
                self.lines[i - 1] = self.lines[i];
            }
            self.line_count = 11;
        }
        let len = s.len().min(32);
        self.lines[self.line_count][..len].copy_from_slice(&s[..len]);
        // Clear rest.
        for i in len..32 {
            self.lines[self.line_count][i] = 0;
        }
        self.line_count += 1;
    }
    
    fn handle_command(&mut self) {
        // Copy command to local buffer to avoid borrow issues.
        let mut cmd_buf = [0u8; 32];
        let cmd_len = self.input_len.min(32);
        cmd_buf[..cmd_len].copy_from_slice(&self.input[..cmd_len]);
        let cmd = &cmd_buf[..cmd_len];
        
        // Echo the command.
        let mut echo = [0u8; 32];
        echo[0] = b'>';
        echo[1] = b' ';
        let len = cmd_len.min(30);
        echo[2..2 + len].copy_from_slice(&cmd[..len]);
        self.push_line(&echo[..2 + len]);
        
        // Parse.
        if cmd == b"help" {
            self.push_line(b"Commands: help, echo, clear");
        } else if cmd == b"clear" {
            self.line_count = 0;
        } else if cmd_len > 5 && &cmd[..5] == b"echo " {
            self.push_line(&cmd[5..]);
        } else if cmd.is_empty() {
            // Do nothing.
        } else {
            self.push_line(b"unknown command");
        }
        
        // Clear input.
        self.input_len = 0;
    }
}

/// Entry point.
#[no_mangle]
pub extern "C" fn user_main() {
    println!("[portal] Portal starting...");
    
    let mut shell = Shell::new();
    shell.push_line(b"portal v1.0 - mini shell");
    shell.push_line(b"Type 'help' for commands");
    
    let mut char_buf = [0u8; 4];
    
    loop {
        // Read window position from WM.
        let (wx, wy) = read_pos();
        let cx = wx + CONTENT_X_OFF;
        let cy = wy + CONTENT_Y_OFF;
        let cw = WIN_W - 4;
        let ch = WIN_H - TITLE_H - 4;
        
        // Draw content background.
        fill_rect(cx, cy, cw, ch, BLACK);
        
        // Draw shell lines.
        for (i, line) in shell.lines[..shell.line_count].iter().enumerate() {
            let y = cy + 4 + (i as i32 * FONT_H);
            if y + FONT_H > cy + ch {
                break;
            }
            // Convert line to str (stop at first 0).
            let len = line.iter().position(|&b| b == 0).unwrap_or(32);
            let s = core::str::from_utf8(&line[..len]).unwrap_or("");
            draw_str(cx + 4, y, s, GREEN);
        }
        
        // Draw input line with cursor.
        let input_y = cy + 4 + (shell.line_count as i32 * FONT_H);
        if input_y + FONT_H <= cy + ch {
            draw_str(cx + 4, input_y, "> ", GREEN);
            let input_str = core::str::from_utf8(&shell.input[..shell.input_len]).unwrap_or("");
            let end_x = draw_str(cx + 4 + 16, input_y, input_str, WHITE);
            // Blinking cursor (based on ticks).
            if (sys::ticks() / 9) % 2 == 0 {
                fill_rect(end_x, input_y, 8, FONT_H, WHITE);
            }
        }
        
        // Handle keyboard input (non-blocking).
        let n = sys::read_nb(sys::STDIN_FD, &mut char_buf);
        if n > 0 {
            let c = char_buf[0];
            if c == b'\n' || c == b'\r' {
                shell.handle_command();
            } else if c == 8 || c == 127 {
                // Backspace.
                if shell.input_len > 0 {
                    shell.input_len -= 1;
                }
            } else if shell.input_len < 32 && c >= 32 && c < 127 {
                shell.input[shell.input_len] = c;
                shell.input_len += 1;
            }
        }
        
        // Small delay.
        let start = sys::ticks();
        while sys::ticks() - start < 1 {}
    }
}
