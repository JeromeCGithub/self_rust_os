//! Window manager for `self_rust_os`, built on `userlib`.
//!
//! A user-space compositor that draws the desktop (wallpaper, taskbar with
//! terminal icon), tracks the mouse cursor, and manages portal windows
//! (spawn on icon click, close via X button, drag via title bar).
//!
//! Uses 32bpp XRGB framebuffer with dynamic geometry from `SYS_FB_INFO`.

#![no_std]
#![no_main]

use userlib::{println, sys};

/// Framebuffer base address (mapped by kernel).
const FB_ADDR: usize = 0x90_0000;

// 32-bit XRGB colors (0x00RRGGBB).
const BLACK: u32 = 0x000000;
const BLUE: u32 = 0x0000AA;
const GREEN: u32 = 0x00AA00;
const RED: u32 = 0xAA0000;
const BROWN: u32 = 0xAA5500;
const LIGHT_GRAY: u32 = 0xAAAAAA;
const DARK_GRAY: u32 = 0x555555;
const LIGHT_BLUE: u32 = 0x5555FF;
const LIGHT_GREEN: u32 = 0x55FF55;
const YELLOW: u32 = 0xFFFF55;
const WHITE: u32 = 0xFFFFFF;

/// Global framebuffer geometry (set at startup from SYS_FB_INFO).
static mut FB_WIDTH: i32 = 1280;
static mut FB_HEIGHT: i32 = 720;
static mut FB_PITCH: i32 = 1280 * 4;

/// Taskbar height (scaled for 720p).
const TASKBAR_H: i32 = 40;
/// Terminal icon rect (in taskbar).
const ICON_X: i32 = 16;
const ICON_W: i32 = 64;
const ICON_H: i32 = 32;

/// Portal window defaults (scaled for 720p).
const WIN_W: i32 = 640;
const WIN_H: i32 = 400;
const WIN_X0: i32 = 100;
const WIN_Y0: i32 = 80;
const TITLE_H: i32 = 32;

/// Position file for portal window (written by WM, read by portal).
const POS_FILE: &str = "portal.pos";

/// Returns the icon Y position (depends on FB_HEIGHT).
fn icon_y() -> i32 {
    unsafe { FB_HEIGHT - TASKBAR_H + 4 }
}

/// Sets a pixel (with bounds check). 32bpp XRGB.
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

/// Draws a simple wallpaper: gradient sky, sun, hills (scaled for 720p).
fn draw_wallpaper() {
    unsafe {
        let horizon = FB_HEIGHT - TASKBAR_H - 100;
        // Sky gradient.
        for y in 0..horizon {
            let color = if y < horizon / 2 { LIGHT_BLUE } else { BLUE };
            for x in 0..FB_WIDTH {
                set_pixel(x, y, color);
            }
        }
        // Sun (yellow circle, bigger for 720p).
        let (scx, scy, r) = (FB_WIDTH - 200, 80, 40);
        for dy in -r..=r {
            for dx in -r..=r {
                if dx * dx + dy * dy <= r * r {
                    set_pixel(scx + dx, scy + dy, YELLOW);
                }
            }
        }
        // Hills (green).
        for x in 0..FB_WIDTH {
            let t1 = (x % 400) as i32;
            let h1 = horizon - 60 + if t1 < 200 { t1 / 4 } else { (400 - t1) / 4 };
            let t2 = ((x + 100) % 500) as i32;
            let h2 = horizon - 40 + if t2 < 250 { t2 / 4 } else { (500 - t2) / 4 };
            let hill_top = h1.min(h2);
            for y in hill_top..(FB_HEIGHT - TASKBAR_H) {
                let color = if y < hill_top + 20 { LIGHT_GREEN } else { GREEN };
                set_pixel(x, y, color);
            }
        }
        // Ground strip.
        for y in (FB_HEIGHT - TASKBAR_H - 30)..(FB_HEIGHT - TASKBAR_H) {
            for x in 0..FB_WIDTH {
                set_pixel(x, y, BROWN);
            }
        }
    }
}

/// Draws the taskbar with terminal icon.
fn draw_taskbar() {
    unsafe {
        let taskbar_y = FB_HEIGHT - TASKBAR_H;
        // Taskbar background.
        fill_rect(0, taskbar_y, FB_WIDTH, TASKBAR_H, DARK_GRAY);
        // Top border.
        fill_rect(0, taskbar_y, FB_WIDTH, 2, LIGHT_GRAY);

        let iy = icon_y();
        // Terminal icon: dark rect with green ">" symbol.
        fill_rect(ICON_X, iy, ICON_W, ICON_H, BLACK);
        fill_rect(ICON_X, iy, ICON_W, 2, LIGHT_GRAY);
        fill_rect(ICON_X, iy + ICON_H - 2, ICON_W, 2, LIGHT_GRAY);
        fill_rect(ICON_X, iy, 2, ICON_H, LIGHT_GRAY);
        fill_rect(ICON_X + ICON_W - 2, iy, 2, ICON_H, LIGHT_GRAY);
        // Draw ">" (bigger for 720p).
        let cx = ICON_X + 12;
        let cy = iy + 16;
        for i in 0..10 {
            set_pixel(cx + i, cy - i, GREEN);
            set_pixel(cx + i, cy + i, GREEN);
        }
        // Underscore cursor.
        for i in 0..12 {
            set_pixel(cx + 16 + i, cy + 8, GREEN);
        }
    }
}

/// Draws the mouse cursor (bigger white arrow for 720p).
fn draw_cursor(x: i32, y: i32) {
    // Arrow: vertical line (20px) with diagonal.
    for i in 0..20 {
        set_pixel(x, y + i, WHITE);
        set_pixel(x + 1, y + i, WHITE);
    }
    for i in 2..12 {
        set_pixel(x + i, y + i, WHITE);
        set_pixel(x + i, y + i + 1, WHITE);
    }
    // Black outline.
    for i in 0..20 {
        set_pixel(x - 1, y + i, BLACK);
        set_pixel(x + 2, y + i, BLACK);
    }
}

/// Draws window decorations (title bar with close button).
/// Returns the close button rect for hit-testing.
fn draw_window_frame(x: i32, y: i32) -> (i32, i32, i32, i32) {
    // Window border.
    fill_rect(x - 4, y - 4, WIN_W + 8, WIN_H + 8, LIGHT_GRAY);
    // Title bar.
    fill_rect(x, y, WIN_W, TITLE_H, BLUE);
    // Close button (red X, bigger).
    let bx = x + WIN_W - 28;
    let by = y + 4;
    fill_rect(bx, by, 24, 24, RED);
    for i in 0..16 {
        set_pixel(bx + 4 + i, by + 4 + i, WHITE);
        set_pixel(bx + 4 + i, by + 5 + i, WHITE);
        set_pixel(bx + 19 - i, by + 4 + i, WHITE);
        set_pixel(bx + 19 - i, by + 5 + i, WHITE);
    }
    (bx, by, 24, 24)
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

    // Get framebuffer geometry from the kernel.
    match sys::fb_info() {
        Ok(info) => {
            unsafe {
                FB_WIDTH = info.width as i32;
                FB_HEIGHT = info.height as i32;
                FB_PITCH = info.pitch as i32;
            }
            println!(
                "[wm] Framebuffer: {}x{} pitch={} bpp={}",
                info.width, info.height, info.pitch, info.bpp
            );
        }
        Err(()) => {
            println!("[wm] Failed to get framebuffer info, using defaults");
        }
    }

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
            let iy = icon_y();
            // Click on terminal icon?
            if in_rect(mx, my, ICON_X, iy, ICON_W, ICON_H) {
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
                    unsafe {
                        let nx = (mx - ox).clamp(0, FB_WIDTH - WIN_W);
                        let ny = (my - oy).clamp(0, FB_HEIGHT - TASKBAR_H - WIN_H);
                        portal = Some((pid, nx, ny));
                        write_pos(nx, ny);
                    }
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
