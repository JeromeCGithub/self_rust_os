//! Desktop compositor. Only PID 0 owns the hardware framebuffer; applications
//! draw into private surfaces which are composed in window stacking order.
#![no_std]
#![no_main]
use font8x8::{UnicodeFonts, BASIC_FONTS};
use userlib::{println, sys};
const MAX_W: usize = 1920;
const MAX_H: usize = 1080;
const MAX_PITCH: usize = 8192;
struct Desktop {
    info: sys::FbInfo,
}
const MAX: usize = 6;
static mut BACK: [u32; MAX_W * MAX_H] = [0; MAX_W * MAX_H];
static mut SURFACE: [u32; MAX_PITCH / 4 * MAX_H] = [0; MAX_PITCH / 4 * MAX_H];
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
impl Desktop {
    fn pixel(&self, x: i32, y: i32, c: u32) {
        if x >= 0 && y >= 0 && x < self.info.width as i32 && y < self.info.height as i32 {
            // SAFETY: Validated dimensions and the coordinate bounds keep
            // this index inside BACK. Only this compositor writes its buffer.
            unsafe {
                BACK[y as usize * self.info.width + x as usize] = c;
            }
        }
    }
    fn rect(&self, x: i32, y: i32, w: i32, h: i32, c: u32) {
        for yy in y.max(0)..(y + h).min(self.info.height as i32) {
            for xx in x.max(0)..(x + w).min(self.info.width as i32) {
                self.pixel(xx, yy, c);
            }
        }
    }
    fn text(&self, x: i32, y: i32, s: &str, c: u32) {
        for (i, ch) in s.chars().enumerate() {
            if let Some(g) = BASIC_FONTS.get(ch) {
                for (yy, b) in g.iter().enumerate() {
                    for xx in 0..8 {
                        if b & (1 << xx) != 0 {
                            self.pixel(x + i as i32 * 8 + xx, y + yy as i32, c);
                        }
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
impl Desktop {
    fn desktop(&self, apps: &[App], wallpaper: bool) {
        for y in 0..self.info.height {
            for x in 0..self.info.width {
                let c = if wallpaper {
                    let i =
                        15 + ((y * 120 / self.info.height) * 160 + x * 160 / self.info.width) * 3;
                    // SAFETY: Scaling bounds this RGB triplet to WALL's
                    // 160x120 payload, loaded before the rendering loop.
                    unsafe {
                        ((WALL[i] as u32) << 16) | ((WALL[i + 1] as u32) << 8) | WALL[i + 2] as u32
                    }
                } else {
                    0x16364e
                };
                self.pixel(x as i32, y as i32, c);
            }
        }
        self.rect(0, 0, self.info.width as i32, 27, 0x111d30);
        self.text(15, 10, "RustOS", 0xffffff);
        self.text(
            self.info.width as i32 - 265,
            10,
            "DESKTOP / APPLICATIONS",
            0xa3bfd6,
        );
        for (i, app) in apps.iter().enumerate() {
            let x = 18 + i as i32 * 100;
            self.rect(x + 12, 48, 44, 38, 0x0a1628);
            self.rect(x + 12, 48, 44, 3, 0x72dac6);
            let symbol = match string(&app.name) {
                "Terminal" => ">_",
                "FORGE" => "sh",
                "HELLO" => "hi",
                "SNAKE" => "S",
                "SPIN" => "@",
                _ => "[]",
            };
            self.text(x + 22, 62, symbol, 0x91e4d2);
            self.text(x, 98, string(&app.name), 0xffffff);
        }
        self.rect(
            0,
            self.info.height as i32 - 32,
            self.info.width as i32,
            32,
            0x101c2e,
        );
        self.text(
            14,
            self.info.height as i32 - 19,
            "R  /  Workspace 1",
            0xc8e7f0,
        );
        self.text(
            self.info.width as i32 - 300,
            self.info.height as i32 - 19,
            "Click an app to open a window",
            0x9bb2c8,
        );
    }
    fn window(&self, win: &mut Window, active: bool) {
        // SAFETY: The validated geometry fits SURFACE, including pitch padding.
        // This single-threaded compositor exclusively owns the scratch buffer.
        let result = unsafe {
            sys::snapshot(
                win.pid,
                &mut (&mut *core::ptr::addr_of_mut!(SURFACE))
                    [..self.info.pitch / 4 * self.info.height],
            )
        };
        win.exited = result == Ok(true);
        self.rect(win.x + 5, win.y + 5, 492, 326, 0x081321);
        self.rect(
            win.x - 1,
            win.y - 1,
            490,
            322,
            if active { 0x86cfc8 } else { 0x45576e },
        );
        self.rect(
            win.x,
            win.y,
            488,
            28,
            if active { 0x233b55 } else { 0x1c2d42 },
        );
        self.text(win.x + 12, win.y + 10, string(&win.app.name), 0xf4f8ff);
        if win.exited {
            self.text(win.x + 200, win.y + 10, "Exited", 0x99b4c9);
        }
        self.text(win.x + 465, win.y + 10, "x", 0xffa9a9);
        let scale = string(&win.app.name) == "SNAKE";
        for y in 0..288 {
            for x in 0..488 {
                let sx = if scale { x * self.info.width / 488 } else { x };
                let sy = if scale { y * self.info.height / 288 } else { y };
                let c = if result.is_err() {
                    0x101827
                } else {
                    // SAFETY: Sampling stays within the validated width,
                    // height and pitch. snapshot's mutable borrow has ended.
                    unsafe { SURFACE[sy * (self.info.pitch / 4) + sx] }
                };
                self.pixel(win.x + x as i32, win.y + 28 + y as i32, c);
            }
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
        Ok(i)
            if (640..=MAX_W).contains(&i.width)
                && (480..=MAX_H).contains(&i.height)
                && i.bpp == 32
                && i.pitch >= i.width * 4
                && i.pitch <= MAX_PITCH
                && i.pitch % 4 == 0 =>
        {
            i
        }
        _ => {
            println!("[desktop] unsupported display geometry");
            sys::exit(1)
        }
    };
    let display = Desktop { info };
    // SAFETY: WALL is exclusively borrowed for this synchronous read. The
    // returned size and header are checked before rendering its pixel data.
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
                        println!("[desktop] drag started: pid={}", w.pid);
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
                w.x = (mx - dx).clamp(0, info.width as i32 - 488);
                w.y = (my - dy).clamp(28, info.height as i32 - 32 - 316);
            }
        }
        display.desktop(&apps[..count], wallpaper);
        for i in 0..used {
            display.window(windows[i].as_mut().unwrap(), i == used - 1);
        }
        for y in 0..14 {
            for x in 0..=y / 2 {
                display.pixel(
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
        // SAFETY: Geometry validation bounds every source row within BACK.
        // The kernel maps the full pitch * height display at FRAMEBUFFER_ADDR;
        // its writable MMIO pages are disjoint from BACK and u32-aligned.
        // Only this process owns the display, and no references are retained.
        unsafe {
            let ptr = core::ptr::addr_of!(BACK) as *const u32;
            for y in 0..info.height {
                core::ptr::copy_nonoverlapping(
                    ptr.add(y * info.width),
                    (sys::FRAMEBUFFER_ADDR + y * info.pitch) as *mut u32,
                    info.width,
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
