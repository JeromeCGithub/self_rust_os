//! Window manager for `self_rust_os`, built on `userlib`.
//!
//! A user-space compositor that draws the desktop (nature wallpaper,
//! taskbar with terminal icon), tracks the mouse cursor, and manages
//! portal windows (spawn on icon click, close via X button, drag via
//! title bar).

#![no_std]
#![no_main]

use userlib::{println, sys};

/// Framebuffer: 320x200, 8-bit color at 0x900000.
const FB_ADDR: usize = 0x90_0000;
const FB_WIDTH: i32 = 320;
const FB_HEIGHT: i32 = 200;

/// VGA palette indices.
const BLACK: u8 = 0;
const BLUE: u8 = 1;
const GREEN: u8 = 2;
const RED: u8 = 4;
const BROWN: u8 = 6;
const LIGHT_GRAY: u8 = 7;
const DARK_GRAY: u8 = 8;
const LIGHT_BLUE: u8 = 9;
const LIGHT_GREEN: u8 = 10;
const YELLOW: u8 = 14;
const WHITE: u8 = 15;

/// Taskbar height.
const TASKBAR_H: i32 = 20;
/// Terminal icon rect (in taskbar).
const ICON_X: i32 = 8;
const ICON_Y: i32 = FB_HEIGHT - TASKBAR_H + 2;
const ICON_W: i32 = 32;
const ICON_H: i32 = 16;

/// Portal window defaults.
const WIN_W: i32 = 220;
const WIN_H: i32 = 140;
const WIN_X0: i32 = 50;
const WIN_Y0: i32 = 40;
const TITLE_H: i32 = 16;

/// Position file for portal window (written by WM, read by portal).
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

/// Draws a simple nature wallpaper: sky gradient, sun, hills.
fn draw_wallpaper() {
    // Sky gradient (top to horizon).
    for y in 0..120 {
        let t = y as u8;
        // Interpolate between light blue (top) and lighter near horizon.
        let color = if t < 60 { LIGHT_BLUE } else { BLUE };
        for x in 0..FB_WIDTH {
            set_pixel(x, y, color);
        }
    }
    // Sun (yellow circle).
    let (scx, scy, r) = (260, 30, 14);
    for dy in -r..=r {
        for dx in -r..=r {
            if dx * dx + dy * dy <= r * r {
                set_pixel(scx + dx, scy + dy, YELLOW);
            }
        }
    }
    // Hills (green, two overlapping). Use simple triangle waves.
    for x in 0..FB_WIDTH {
        // Hill 1: triangle wave with period 160.
        let t1 = (x % 160) as i32;
        let h1 = 120 + if t1 < 80 { t1 / 4 } else { (160 - t1) / 4 };
        // Hill 2: triangle wave with period 200, offset.
        let t2 = ((x + 50) % 200) as i32;
        let h2 = 130 + if t2 < 100 { t2 / 4 } else { (200 - t2) / 4 };
        let hill_top = h1.min(h2);
        for y in hill_top..(FB_HEIGHT - TASKBAR_H) {
            let color = if y < hill_top + 10 { LIGHT_GREEN } else { GREEN };
            set_pixel(x, y, color);
        }
    }
    // Ground (brown strip at bottom above taskbar).
    for y in (FB_HEIGHT - TASKBAR_H - 10)..(FB_HEIGHT - TASKBAR_H) {
        for x in 0..FB_WIDTH {
            set_pixel(x, y, BROWN);
        }
    }
}

/// Draws the taskbar with terminal icon.
fn draw_taskbar() {
    // Taskbar background.
    fill_rect(0, FB_HEIGHT - TASKBAR_H, FB_WIDTH, TASKBAR_H, DARK_GRAY);
    // Top border.
    fill_rect(0, FB_HEIGHT - TASKBAR_H, FB_WIDTH, 1, LIGHT_GRAY);
    
    // Terminal icon: dark rect with green ">" symbol.
    fill_rect(ICON_X, ICON_Y, ICON_W, ICON_H, BLACK);
    fill_rect(ICON_X, ICON_Y, ICON_W, 1, LIGHT_GRAY); // top border
    fill_rect(ICON_X, ICON_Y + ICON_H - 1, ICON_W, 1, LIGHT_GRAY); // bottom
    fill_rect(ICON_X, ICON_Y, 1, ICON_H, LIGHT_GRAY); // left
    fill_rect(ICON_X + ICON_W - 1, ICON_Y, 1, ICON_H, LIGHT_GRAY); // right
    // Draw ">" using pixels (simple arrow).
    let cx = ICON_X + 6;
    let cy = ICON_Y + 8;
    for i in 0..5 {
        set_pixel(cx + i, cy - i, GREEN);
        set_pixel(cx + i, cy + i, GREEN);
    }
    // Underscore cursor.
    for i in 0..6 {
        set_pixel(cx + 8 + i, cy + 4, GREEN);
    }
}

/// Draws the mouse cursor (simple white arrow).
fn draw_cursor(x: i32, y: i32) {
    // Arrow shape: vertical line with diagonal.
    for i in 0..10 {
        set_pixel(x, y + i, WHITE);
    }
    for i in 1..6 {
        set_pixel(x + i, y + i, WHITE);
    }
    // Outline (black).
    set_pixel(x - 1, y, BLACK);
    set_pixel(x, y - 1, BLACK);
}

/// Draws window decorations (title bar with close button).
/// Returns the close button rect for hit-testing.
fn draw_window_frame(x: i32, y: i32) -> (i32, i32, i32, i32) {
    // Window border.
    fill_rect(x - 2, y - 2, WIN_W + 4, WIN_H + 4, LIGHT_GRAY);
    // Title bar.
    fill_rect(x, y, WIN_W, TITLE_H, BLUE);
    // Title text "portal" (simple pixels - just draw a line for now).
    // Close button (red X).
    let bx = x + WIN_W - 14;
    let by = y + 2;
    fill_rect(bx, by, 12, 12, RED);
    // White X.
    for i in 0..8 {
        set_pixel(bx + 2 + i, by + 2 + i, WHITE);
        set_pixel(bx + 9 - i, by + 2 + i, WHITE);
    }
    (bx, by, 12, 12)
}

/// Writes the portal window position to the pos file.
fn write_pos(x: i32, y: i32) {
    let mut buf = [0u8; 16];
    let s = format_pos(x, y, &mut buf);
    let _ = sys::write_file(POS_FILE, s.as_bytes());
}

/// Formats "x y\n" into the buffer. Returns the string.
fn format_pos<'a>(x: i32, y: i32, buf: &'a mut [u8; 16]) -> &'a str {
    let mut i = 0;
    // Write x.
    let mut n = x;
    if n == 0 {
        buf[i] = b'0';
        i += 1;
    } else {
        let mut digits = [0u8; 8];
        let mut d = 0;
        while n > 0 {
            digits[d] = b'0' + (n % 10) as u8;
            n /= 10;
            d += 1;
        }
        for j in (0..d).rev() {
            buf[i] = digits[j];
            i += 1;
        }
    }
    buf[i] = b' ';
    i += 1;
    // Write y.
    let mut n = y;
    if n == 0 {
        buf[i] = b'0';
        i += 1;
    } else {
        let mut digits = [0u8; 8];
        let mut d = 0;
        while n > 0 {
            digits[d] = b'0' + (n % 10) as u8;
            n /= 10;
            d += 1;
        }
        for j in (0..d).rev() {
            buf[i] = digits[j];
            i += 1;
        }
    }
    buf[i] = b'\n';
    i += 1;
    core::str::from_utf8(&buf[..i]).unwrap_or("0 0\n")
}

/// Checks if point (px, py) is in rect (x, y, w, h).
fn in_rect(px: i32, py: i32, x: i32, y: i32, w: i32, h: i32) -> bool {
    px >= x && px < x + w && py >= y && py < y + h
}

/// Entry point.
#[no_mangle]
pub extern "C" fn user_main() {
    println!("[wm] Window manager starting...");
    
    // Window state: None = no portal, Some((pid, x, y)) = portal open.
    let mut portal: Option<(u32, i32, i32)> = None;
    
    // Drag state: None = not dragging, Some((offset_x, offset_y)) = dragging.
    let mut drag: Option<(i32, i32)> = None;
    
    // Previous mouse buttons for edge detection.
    let mut prev_buttons: u8 = 0;
    
    // Initial draw.
    draw_wallpaper();
    draw_taskbar();
    
    loop {
        let (mx, my, buttons) = sys::mouse();
        let left_pressed = (buttons & 0x01) != 0;
        let left_clicked = left_pressed && (prev_buttons & 0x01) == 0;
        let left_released = !left_pressed && (prev_buttons & 0x01) != 0;
        
        // Redraw desktop (simple: full redraw each frame).
        draw_wallpaper();
        draw_taskbar();
        
        // Draw portal window frame if open.
        let mut close_rect = (0, 0, 0, 0);
        if let Some((_, wx, wy)) = portal {
            close_rect = draw_window_frame(wx, wy);
        }
        
        // Handle input.
        if left_clicked {
            // Click on terminal icon?
            if in_rect(mx, my, ICON_X, ICON_Y, ICON_W, ICON_H) {
                if portal.is_none() {
                    println!("[wm] Spawning portal...");
                    match sys::spawn("/bin/PORTAL.BIN") {
                        Ok(pid) => {
                            println!("[wm] Portal spawned, pid {}", pid);
                            portal = Some((pid, WIN_X0, WIN_Y0));
                            write_pos(WIN_X0, WIN_Y0);
                        }
                        Err(()) => {
                            println!("[wm] Failed to spawn portal");
                        }
                    }
                }
            }
            // Click on close button?
            else if let Some((pid, wx, wy)) = portal {
                let (bx, by, bw, bh) = close_rect;
                if in_rect(mx, my, bx, by, bw, bh) {
                    println!("[wm] Closing portal pid {}", pid);
                    let _ = sys::kill(pid);
                    portal = None;
                    // Remove pos file.
                    let _ = sys::write_file(POS_FILE, b"");
                }
                // Click on title bar? Start drag.
                else if in_rect(mx, my, wx, wy, WIN_W, TITLE_H) {
                    drag = Some((mx - wx, my - wy));
                }
            }
        }
        
        // Handle drag.
        if let Some((ox, oy)) = drag {
            if left_pressed {
                if let Some((pid, _, _)) = portal {
                    let nx = (mx - ox).clamp(0, FB_WIDTH - WIN_W);
                    let ny = (my - oy).clamp(0, FB_HEIGHT - TASKBAR_H - WIN_H);
                    portal = Some((pid, nx, ny));
                    write_pos(nx, ny);
                }
            } else {
                drag = None;
            }
        }
        
        if left_released {
            drag = None;
        }
        
        // Draw cursor on top.
        draw_cursor(mx, my);
        
        prev_buttons = buttons;
        
        // Small delay to avoid busy loop (~30fps).
        let start = sys::ticks();
        while sys::ticks() - start < 2 {}
    }
}
