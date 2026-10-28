//! Desktop compositor. Only PID 0 owns the hardware framebuffer; applications
//! draw into private surfaces which are composed in window stacking order.
#![no_std]
#![no_main]
use font8x8::{UnicodeFonts, BASIC_FONTS};
use userlib::{println, sys};
const W: usize = 640;
const H: usize = 480;
const MAX: usize = 6;
static mut BACK: [u32; W * H] = [0; W * H];
static mut SURFACE: [u32; W * H] = [0; W * H];
static mut WALL: [u8; 57615] = [0; 57615];
#[derive(Clone, Copy)]
struct App {
    name: [u8; 12],
    path: [u8; 32],
}
const EMPTY_APP: App = App {
    name: [0; 12],
    path: [0; 32],
};
#[derive(Clone, Copy)]
struct Window {
    pid: u32,
    app: App,
    x: i32,
    y: i32,
    exited: bool,
}
fn string(bytes: &[u8]) -> &str {
    core::str::from_utf8(&bytes[..bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len())])
        .unwrap_or("")
}
fn pixel(x: i32, y: i32, c: u32) {
    if x >= 0 && y >= 0 && x < W as i32 && y < H as i32 {
        unsafe {
            BACK[y as usize * W + x as usize] = c;
        }
    }
}
fn rect(x: i32, y: i32, w: i32, h: i32, c: u32) {
    for yy in y.max(0)..(y + h).min(H as i32) {
        for xx in x.max(0)..(x + w).min(W as i32) {
            pixel(xx, yy, c);
        }
    }
}
fn text(x: i32, y: i32, s: &str, c: u32) {
    for (i, ch) in s.chars().enumerate() {
        if let Some(g) = BASIC_FONTS.get(ch) {
            for (yy, b) in g.iter().enumerate() {
                for xx in 0..8 {
                    if b & (1 << xx) != 0 {
                        pixel(x + i as i32 * 8 + xx, y + yy as i32, c);
                    }
                }
            }
        }
    }
}
fn hit(mx: i32, my: i32, x: i32, y: i32, w: i32, h: i32) -> bool {
    mx >= x && mx < x + w && my >= y && my < y + h
}
fn discover(apps: &mut [App; MAX]) -> usize {
    let mut entries = [0u8; 12 * 32];
    let mut count = 0;
    if let Some(n) = sys::ls("/BIN", &mut entries, 32) {
        for e in entries.chunks_exact(12).take(n.min(32)) {
            if &e[8..11] != b"BIN" || &e[..8] == b"WM      " || count == MAX {
                continue;
            }
            let len = e[..8].iter().position(|b| *b == b' ').unwrap_or(8);
            let mut app = EMPTY_APP;
            app.name[..len].copy_from_slice(&e[..len]);
            if &e[..len] == b"PORTAL" {
                app.name = [0; 12];
                app.name[..8].copy_from_slice(b"Terminal");
            }
            app.path[..5].copy_from_slice(b"/BIN/");
            app.path[5..5 + len].copy_from_slice(&e[..len]);
            app.path[5 + len..9 + len].copy_from_slice(b".BIN");
            apps[count] = app;
            count += 1;
        }
    }
    count
}
fn desktop(apps: &[App], wallpaper: bool) {
    for y in 0..H {
        for x in 0..W {
            let c = if wallpaper {
                let i = 15 + ((y * 120 / H) * 160 + x * 160 / W) * 3;
                unsafe {
                    ((WALL[i] as u32) << 16) | ((WALL[i + 1] as u32) << 8) | WALL[i + 2] as u32
                }
            } else {
                0x16364e
            };
            pixel(x as i32, y as i32, c);
        }
    }
    rect(0, 0, 640, 27, 0x111d30);
    text(15, 10, "RustOS", 0xffffff);
    text(375, 10, "DESKTOP / APPLICATIONS", 0xa3bfd6);
    for (i, app) in apps.iter().enumerate() {
        let x = 18 + i as i32 * 100;
        rect(x + 12, 48, 44, 38, 0x0a1628);
        rect(x + 12, 48, 44, 3, 0x72dac6);
        let symbol = match string(&app.name) {
            "Terminal" => ">_",
            "FORGE" => "sh",
            "HELLO" => "hi",
            "SNAKE" => "S",
            "SPIN" => "@",
            _ => "[]",
        };
        text(x + 22, 62, symbol, 0x91e4d2);
        text(x, 98, string(&app.name), 0xffffff);
    }
    rect(0, 448, 640, 32, 0x101c2e);
    text(14, 461, "R  /  Workspace 1", 0xc8e7f0);
    text(340, 461, "Click an app to open a window", 0x9bb2c8);
}
fn window(win: &mut Window, active: bool) {
    let result = unsafe { sys::snapshot(win.pid, &mut *core::ptr::addr_of_mut!(SURFACE)) };
    win.exited = result == Ok(true);
    rect(win.x + 5, win.y + 5, 492, 326, 0x081321);
    rect(
        win.x - 1,
        win.y - 1,
        490,
        322,
        if active { 0x86cfc8 } else { 0x45576e },
    );
    rect(
        win.x,
        win.y,
        488,
        28,
        if active { 0x233b55 } else { 0x1c2d42 },
    );
    text(win.x + 12, win.y + 10, string(&win.app.name), 0xf4f8ff);
    if win.exited {
        text(win.x + 200, win.y + 10, "Exited", 0x99b4c9);
    }
    text(win.x + 465, win.y + 10, "x", 0xffa9a9);
    let scale = string(&win.app.name) == "SNAKE";
    for y in 0..288 {
        for x in 0..488 {
            let sx = if scale { x * W / 488 } else { x };
            let sy = if scale { y * H / 288 } else { y };
            let c = if result.is_err() {
                0x101827
            } else {
                unsafe { SURFACE[sy * W + sx] }
            };
            pixel(win.x + x as i32, win.y + 28 + y as i32, c);
        }
    }
}
fn focus(pid: u32) {
    let _ = sys::focus(pid);
}
#[no_mangle]
pub extern "C" fn user_main() {
    sys::graphics_mode();
    let info = match sys::fb_info() {
        Ok(i) if i.width == W && i.height == H && i.bpp == 32 => i,
        _ => {
            println!("[desktop] requires 640x480x32");
            sys::exit(1)
        }
    };
    let wallpaper = unsafe {
        sys::read_file("/ETC/WALL.PPM", &mut *core::ptr::addr_of_mut!(WALL)) == Some(57615)
            && &WALL[..15] == b"P6\n160 120\n255\n"
    };
    println!("[desktop] wallpaper loaded: {}", wallpaper);
    let mut apps = [EMPTY_APP; MAX];
    let count = discover(&mut apps);
    println!("[desktop] ready: {} applications", count);
    let mut windows: [Option<Window>; MAX] = [None; MAX];
    let mut used = 0;
    let mut previous = 0;
    let mut drag: Option<(i32, i32)> = None;
    loop {
        let (mx, my, buttons) = sys::mouse();
        let down = buttons & 1 != 0;
        let click = down && previous & 1 == 0;
        if click {
            let window_hit = (0..used).rev().find(|&i| {
                let w = windows[i].unwrap();
                hit(mx, my, w.x, w.y, 488, 316)
            });
            if window_hit.is_none() && my >= 40 && my < 120 {
                let index = ((mx - 18).max(0) / 100) as usize;
                if index < count && used < MAX {
                    let app = apps[index];
                    match sys::spawn(string(&app.path)) {
                        Ok(pid) => {
                            windows[used] = Some(Window {
                                pid,
                                app,
                                x: 80 + used as i32 * 12,
                                y: 126 + (used as i32 * 2).min(6),
                                exited: false,
                            });
                            used += 1;
                            focus(pid);
                            println!("[desktop] launched {} pid={}", string(&app.name), pid);
                        }
                        Err(()) => println!("[desktop] launch failed: {}", string(&app.path)),
                    }
                }
            } else {
                if let Some(i) = window_hit {
                    let w = windows[i].unwrap();
                    for j in i..used - 1 {
                        windows[j] = windows[j + 1];
                    }
                    windows[used - 1] = Some(w);
                    focus(w.pid);
                    if hit(mx, my, w.x + 455, w.y, 33, 28) {
                        let _ = sys::kill(w.pid);
                        let _ = sys::waitpid(w.pid);
                        used -= 1;
                        windows[used] = None;
                        focus(if used > 0 {
                            windows[used - 1].unwrap().pid
                        } else {
                            0
                        });
                    } else if hit(mx, my, w.x, w.y, 455, 28) {
                        drag = Some((mx - w.x, my - w.y));
                    }
                } else {
                    focus(0);
                }
            }
        }
        if !down {
            drag = None;
        }
        if let Some((dx, dy)) = drag {
            if used > 0 {
                let w = windows[used - 1].as_mut().unwrap();
                w.x = (mx - dx).clamp(0, 152);
                w.y = (my - dy).clamp(28, 132);
            }
        }
        desktop(&apps[..count], wallpaper);
        for i in 0..used {
            window(windows[i].as_mut().unwrap(), i == used - 1);
        }
        for y in 0..14 {
            for x in 0..=y / 2 {
                pixel(
                    mx + x,
                    my + y,
                    if x == 0 || x == y / 2 {
                        0x081321
                    } else {
                        0xffffff
                    },
                );
            }
        }
        unsafe {
            let ptr = core::ptr::addr_of!(BACK) as *const u32;
            for y in 0..H {
                core::ptr::copy_nonoverlapping(
                    ptr.add(y * W),
                    (0x900000 + y * info.pitch) as *mut u32,
                    W,
                );
            }
        }
        previous = buttons;
        let tick = sys::ticks();
        while sys::ticks() == tick {
            core::hint::spin_loop();
        }
    }
}
