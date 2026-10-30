//! Desktop compositor. Only PID 0 owns the hardware framebuffer; applications
//! draw into private surfaces which are composed in window stacking order.
#![no_std]
#![no_main]
use font8x8::{UnicodeFonts, BASIC_FONTS};
use userlib::{println, sys};
mod damage;
use damage::Damage;
const MAX_W: usize = 1920;
const MAX_H: usize = 1080;
const MAX_PITCH: usize = 8192;
struct Desktop {
    info: sys::FbInfo,
    damage: Damage,
}
const MAX: usize = 6;
// Installed launchers and simultaneous window backing have separate limits.
const MAX_APPS: usize = 16;
static mut BACK: [u32; MAX_W * MAX_H] = [0; MAX_W * MAX_H];
static mut SURFACE: [u32; MAX_PITCH / 4 * MAX_H] = [0; MAX_PITCH / 4 * MAX_H];
static mut CACHE: [[u32; 488 * 288]; MAX] = [[0; 488 * 288]; MAX];
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
    cache_slot: usize,
    revision: Option<(u64, bool)>,
}
fn string(bytes: &[u8]) -> &str {
    core::str::from_utf8(&bytes[..bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len())])
        .unwrap_or("")
}
impl Desktop {
    fn pixel(&mut self, x: i32, y: i32, c: u32) {
        if x >= 0 && y >= 0 && x < self.info.width as i32 && y < self.info.height as i32 {
            // SAFETY: Validated dimensions and the coordinate bounds keep
            // this index inside BACK. Only this compositor writes its buffer.
            unsafe {
                let pixel = core::ptr::addr_of_mut!(BACK)
                    .cast::<u32>()
                    .add(y as usize * self.info.width + x as usize);
                if pixel.read() != c {
                    pixel.write(c);
                    self.damage.mark(x as usize, y as usize, 1, 1);
                }
            }
        }
    }
    fn present(&mut self, mx: i32, my: i32) {
        // Copy adjacent dirty tiles as runs, clipping the partial right/bottom
        // tiles to the actual display. BACK holds the scene without the cursor.
        let info = self.info;
        self.damage.flush(info.width, info.height, |x, y, width| {
            // SAFETY: Validated geometry and clipped tile bounds keep source
            // rows inside exclusively owned BACK and destination rows inside
            // the mapped, aligned pitch * height MMIO region. They are disjoint;
            // no borrow survives presentation.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    core::ptr::addr_of!(BACK)
                        .cast::<u32>()
                        .add(y * info.width + x),
                    (sys::FRAMEBUFFER_ADDR + y * info.pitch + x * 4) as *mut u32,
                    width,
                );
            }
        });
        for y in 0..14 {
            for x in 0..=y / 2 {
                let (px, py) = (mx + x, my + y);
                if px >= 0 && py >= 0 && px < self.info.width as i32 && py < self.info.height as i32
                {
                    let color = if x == 0 || x == y / 2 {
                        0x081321
                    } else {
                        0xffffff
                    };
                    // SAFETY: Clipped coordinates address one aligned pixel in
                    // this process's mapped display; the compositor is its sole
                    // owner. The cursor is deliberately absent from BACK so its
                    // previous position can be restored without repainting.
                    unsafe {
                        ((sys::FRAMEBUFFER_ADDR + py as usize * self.info.pitch + px as usize * 4)
                            as *mut u32)
                            .write_volatile(color);
                    }
                }
            }
        }
    }

    fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: u32) {
        for yy in y.max(0)..(y + h).min(self.info.height as i32) {
            for xx in x.max(0)..(x + w).min(self.info.width as i32) {
                self.pixel(xx, yy, c);
            }
        }
    }
    fn text(&mut self, x: i32, y: i32, s: &str, c: u32) {
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
fn discover(apps: &mut [App; MAX_APPS]) -> usize {
    let mut entries = [0u8; 12 * 32];
    let mut count = 0;
    if let Some(n) = sys::ls("/BIN", &mut entries, 32) {
        for e in entries.chunks_exact(12).take(n.min(32)) {
            if &e[8..11] != b"BIN" || &e[..8] == b"WM      " || count == MAX_APPS {
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
    fn desktop(&mut self, apps: &[App], wallpaper: bool, bounds: (usize, usize, usize, usize)) {
        // Restore only the workspace exposed by a changed window layout.
        let (left, top, right, bottom) = bounds;
        for y in top..bottom {
            let wall_row = 15 + (y * 120 / self.info.height) * 160 * 3;
            for x in left..right {
                let c = if wallpaper {
                    let i = wall_row + (x * 160 / self.info.width) * 3;
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
    fn update_surface(&self, win: &mut Window) -> bool {
        let status = sys::surface_status(win.pid);
        if win.revision == status.ok() && win.revision.is_some() {
            return false;
        }
        let scale = string(&win.app.name) == "SNAKE";
        let words = if scale {
            self.info.pitch / 4 * self.info.height
        } else {
            287 * (self.info.pitch / 4) + 488
        };
        // SAFETY: Validated geometry bounds the full scratch surface, including
        // pitch padding. This process exclusively owns SURFACE and CACHE;
        // cache_slot is unique among live windows and bounded by MAX.
        let result = unsafe {
            sys::snapshot(
                win.pid,
                &mut (&mut *core::ptr::addr_of_mut!(SURFACE))[..words],
            )
        };
        win.exited = result == Ok(true);
        win.revision = status.ok();
        for y in 0..288 {
            for x in 0..488 {
                let sx = if scale { x * self.info.width / 488 } else { x };
                let sy = if scale { y * self.info.height / 288 } else { y };
                // SAFETY: Sampling is within the validated scratch dimensions;
                // the exclusive snapshot borrow has ended. The cache index is
                // bounded by 488x288, and only this compositor accesses it.
                unsafe {
                    CACHE[win.cache_slot][y * 488 + x] = if result.is_err() {
                        0x101827
                    } else {
                        SURFACE[sy * (self.info.pitch / 4) + sx]
                    };
                }
            }
        }
        true
    }
    fn window(&mut self, win: &Window, active: bool) {
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
        for y in 0..288 {
            for x in 0..488 {
                // SAFETY: cache_slot uniquely belongs to this live window;
                // update_surface initialized all in-bounds pixels before use.
                let c = unsafe { CACHE[win.cache_slot][y * 488 + x] };
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
    let mut display = Desktop {
        info,
        damage: Damage::new(),
    };
    // SAFETY: WALL is exclusively borrowed for this synchronous read. The
    // returned size and header are checked before rendering its pixel data.
    let wallpaper = unsafe {
        sys::read_file("/ETC/WALL.PPM", &mut *core::ptr::addr_of_mut!(WALL)) == Some(57615)
            && &WALL[..15] == b"P6\n160 120\n255\n"
    };
    println!("[desktop] wallpaper loaded: {}", wallpaper);
    let mut apps = [EMPTY_APP; MAX_APPS];
    let count = discover(&mut apps);
    println!("[desktop] ready: {} applications", count);
    let mut windows: [Option<Window>; MAX] = [None; MAX];
    let mut used = 0;
    let mut previous = 0;
    let mut drag: Option<(i32, i32)> = None;
    let mut scene_dirty = true;
    let mut old_bounds = (0, 0, info.width, info.height);
    display.damage.mark(0, 0, info.width, info.height);
    let mut old_mouse = None;
    loop {
        let (mx, my, buttons) = sys::mouse();
        let down = buttons & 1 != 0;
        let click = down && previous & 1 == 0;
        if click {
            scene_dirty = true;
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
                            let cache_slot = (0..MAX)
                                .find(|slot| {
                                    !windows[..used]
                                        .iter()
                                        .flatten()
                                        .any(|window| window.cache_slot == *slot)
                                })
                                .expect("free window cache slot");
                            windows[used] = Some(Window {
                                pid,
                                app,
                                x: 80 + used as i32 * 12,
                                y: 126 + (used as i32 * 2).min(6),
                                exited: false,
                                cache_slot,
                                revision: None,
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
                let x = (mx - dx).clamp(0, info.width as i32 - 488);
                let y = (my - dy).clamp(28, info.height as i32 - 32 - 316);
                scene_dirty |= (w.x, w.y) != (x, y);
                (w.x, w.y) = (x, y);
            }
        }
        let mut compose = scene_dirty;
        for window in windows[..used].iter_mut().flatten() {
            compose |= display.update_surface(window);
        }
        if scene_dirty {
            let mut bounds = (info.width, info.height, 0, 0);
            for window in windows[..used].iter().flatten() {
                bounds.0 = bounds.0.min((window.x - 1).max(0) as usize);
                bounds.1 = bounds.1.min((window.y - 1).max(0) as usize);
                bounds.2 = bounds.2.max((window.x as usize + 497).min(info.width));
                bounds.3 = bounds.3.max((window.y as usize + 331).min(info.height));
            }
            display.desktop(
                &apps[..count],
                wallpaper,
                (
                    old_bounds.0.min(bounds.0),
                    old_bounds.1.min(bounds.1),
                    old_bounds.2.max(bounds.2),
                    old_bounds.3.max(bounds.3),
                ),
            );
            old_bounds = bounds;
            scene_dirty = false;
        }
        if compose {
            for i in 0..used {
                display.window(windows[i].as_ref().unwrap(), i == used - 1);
            }
        }
        let moved = old_mouse != Some((mx, my));
        if moved {
            if let Some((x, y)) = old_mouse {
                display.damage.mark(x as usize, y as usize, 8, 14);
            }
            display.damage.mark(mx as usize, my as usize, 8, 14);
        }
        display.present(mx, my);
        old_mouse = Some((mx, my));
        previous = buttons;
        sys::sleep_ticks(1).expect("desktop timed wait");
    }
}
