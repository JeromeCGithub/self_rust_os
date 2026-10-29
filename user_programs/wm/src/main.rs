//! Desktop compositor. Only PID 0 owns the hardware framebuffer; applications
//! draw into private surfaces which are composed in window stacking order.
#![no_std]
#![no_main]
use font8x8::{UnicodeFonts, BASIC_FONTS};
use userlib::ui::{
    protocol::{self, Configuration, Material},
    raster::{coverage, opacity, over, Color},
};
use userlib::{println, sys};
mod damage;
mod glass;
use damage::Damage;
const MAX_W: usize = 1920;
const MAX_H: usize = 1080;
const MAX_PITCH: usize = 8192;
struct Desktop {
    info: sys::FbInfo,
    damage: Damage,
    cursor: Option<(i32, i32)>,
    cursor_under: [u32; 8 * 14],
}
const MAX: usize = 6;
static mut BACK: [u32; MAX_W * MAX_H] = [0; MAX_W * MAX_H];
static mut SURFACE: [u32; MAX_PITCH / 4 * MAX_H] = [0; MAX_PITCH / 4 * MAX_H];
const CONTENT_W: usize = protocol::MAX_WIDTH;
const CONTENT_H: usize = protocol::MAX_HEIGHT;
const TITLE_H: i32 = 28;
const HALO: i32 = 16;
const BLUR_LEN: usize = 168 * 115;
static mut CACHE: [[u32; CONTENT_W * CONTENT_H]; MAX] = [[0; CONTENT_W * CONTENT_H]; MAX];
static mut BLUR_A: [u32; BLUR_LEN] = [0; BLUR_LEN];
static mut BLUR_B: [u32; BLUR_LEN] = [0; BLUR_LEN];
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
    width: usize,
    height: usize,
    title: [u8; protocol::TITLE_BYTES],
    native: bool,
    material: Material,
    commit: Option<u32>,
    reply: Option<u32>,
    focus_event: Option<bool>,
    pointer: Option<(i32, i32, u8)>,
    closing: Option<u64>,
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
    fn present(&mut self, _mx: i32, _my: i32) {
        self.restore_cursor();
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
        let (mx, my, _) = sys::mouse();
        self.draw_cursor(mx, my);
    }
    fn restore_cursor(&mut self) {
        let Some((mx, my)) = self.cursor.take() else {
            return;
        };
        for y in 0..14usize {
            for x in 0..8usize {
                let (px, py) = (mx + x as i32, my + y as i32);
                if px < 0 || py < 0 || px >= self.info.width as i32 || py >= self.info.height as i32
                {
                    continue;
                }
                // SAFETY: Clipped coordinates address aligned pixels within the
                // WM's exclusive pitch*height display mapping. Saved pixels came
                // from this same rectangle before drawing the last cursor. Only
                // this WM writes display memory; no reference spans scheduling.
                unsafe {
                    ((sys::FRAMEBUFFER_ADDR + py as usize * self.info.pitch + px as usize * 4)
                        as *mut u32)
                        .write_volatile(self.cursor_under[y * 8 + x]);
                }
            }
        }
    }
    fn draw_cursor(&mut self, mx: i32, my: i32) {
        for y in 0..14usize {
            for x in 0..8usize {
                let (px, py) = (mx + x as i32, my + y as i32);
                if px < 0 || py < 0 || px >= self.info.width as i32 || py >= self.info.height as i32
                {
                    continue;
                }
                // SAFETY: Validated geometry and clipping bound aligned MMIO
                // accesses. The WM is the sole writer. Save the displayed scene,
                // not the in-progress composition, before overlaying the cursor.
                unsafe {
                    let pixel = (sys::FRAMEBUFFER_ADDR
                        + py as usize * self.info.pitch
                        + px as usize * 4) as *mut u32;
                    self.cursor_under[y * 8 + x] = pixel.read_volatile();
                    if x <= y / 2 {
                        pixel.write_volatile(if x == 0 || x == y / 2 {
                            0x081321
                        } else {
                            0xffffff
                        });
                    }
                }
            }
        }
        self.cursor = Some((mx, my));
    }
    fn refresh_cursor(&mut self) {
        let (x, y, _) = sys::mouse();
        if self.cursor != Some((x, y)) {
            self.restore_cursor();
            self.draw_cursor(x, y);
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
    fn desktop(&mut self, apps: &[App], wallpaper: bool, bounds: (usize, usize, usize, usize)) {
        // Restore only the workspace exposed by a changed window layout.
        let (left, top, right, bottom) = bounds;
        for y in top..bottom {
            if y % 32 == 0 {
                self.refresh_cursor();
            }
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
        win.exited = status.map(|(_, exited)| exited).unwrap_or(true);
        if win.native && win.commit.is_none() {
            return false;
        }
        if !win.native && win.revision == status.ok() && win.revision.is_some() {
            return false;
        }
        let scale = !win.native && string(&win.app.name) == "SNAKE";
        let words = if scale {
            self.info.pitch / 4 * self.info.height
        } else {
            (win.height - 1) * (self.info.pitch / 4) + win.width
        };
        // SAFETY: Display geometry and accepted content dimensions bound this
        // prefix of the private surface. The WM alone owns SURFACE and CACHE;
        // every live window has a unique bounded cache slot. Native clients keep
        // submitted pixels immutable until the RELEASE sent after this copy.
        let result = unsafe {
            sys::snapshot(
                win.pid,
                &mut (&mut *core::ptr::addr_of_mut!(SURFACE))[..words],
            )
        };
        win.revision = status.ok();
        for y in 0..win.height {
            for x in 0..win.width {
                let sx = if scale {
                    x * self.info.width / win.width
                } else {
                    x
                };
                let sy = if scale {
                    y * self.info.height / win.height
                } else {
                    y
                };
                // SAFETY: Sampling is bounded by the validated surface geometry;
                // the snapshot borrow has ended. Only this WM accesses the cache.
                unsafe {
                    let pixel = if result.is_err() {
                        0xff101827
                    } else {
                        SURFACE[sy * (self.info.pitch / 4) + sx]
                    };
                    CACHE[win.cache_slot][y * CONTENT_W + x] = if win.native {
                        pixel
                    } else {
                        pixel | 0xff000000
                    };
                }
            }
        }
        if let Some(serial) = win.commit.take() {
            win.reply = Some(protocol::RELEASE | serial);
        }
        true
    }
    fn window(&mut self, win: &Window, active: bool) {
        let w = win.width as i32;
        let h = win.height as i32 + TITLE_H;
        let blur_width = (w + HALO * 2 + 3) as usize / 4;
        let blur_height = ((if win.native { h } else { TITLE_H }) + HALO * 2 + 3) as usize / 4;
        let tint = match win.material {
            Material::GlassClear => 112,
            Material::Glass => 168,
            Material::Opaque => 255,
        };
        let tint_color = Color::rgba(20, 30, 48, tint).0;
        let title_color = Color::rgba(225, 239, 255, if active { 18 } else { 8 }).0;
        let edge_color = Color::rgba(220, 237, 255, if active { 100 } else { 48 }).0;
        // Capture only layers below this window, before its own shadow/material.
        // SAFETY: Validated display dimensions bound clamped BACK reads. Accepted
        // content bounds guarantee blur_width*blur_height <= BLUR_LEN. This WM is
        // the sole owner of all three buffers; no references survive this block.
        unsafe {
            for y in 0..blur_height {
                for x in 0..blur_width {
                    let sx = (win.x - HALO + x as i32 * 4 + 2).clamp(0, self.info.width as i32 - 1)
                        as usize;
                    let sy = (win.y - HALO + y as i32 * 4 + 2).clamp(0, self.info.height as i32 - 1)
                        as usize;
                    BLUR_A[y * blur_width + x] = BACK[sy * self.info.width + sx];
                }
            }
            glass::blur(
                &*core::ptr::addr_of!(BLUR_A),
                &mut *core::ptr::addr_of_mut!(BLUR_B),
                blur_width,
                blur_height,
                true,
            );
            glass::blur(
                &*core::ptr::addr_of!(BLUR_B),
                &mut *core::ptr::addr_of_mut!(BLUR_A),
                blur_width,
                blur_height,
                false,
            );
            // Tint at low resolution once, instead of blending the same material
            // for every output pixel. Opaque legacy content needs only the title.
            for pixel in
                (&mut *core::ptr::addr_of_mut!(BLUR_A))[..blur_width * blur_height].iter_mut()
            {
                *pixel = over(tint_color, *pixel | 0xff000000);
            }
        }
        // A restrained shadow outside the rounded window's bounding box.
        for y in -8..h + 16 {
            for x in -12..w + 12 {
                if x >= 0 && y >= 0 && x < w && y < h {
                    continue;
                }
                let distance = (-x).max(x - w + 1).max(-y).max(y - h + 1).max(0);
                let alpha = ((16 - distance).max(0) * 3).min(42) as u8;
                self.blend(win.x + x, win.y + y, Color::rgba(3, 8, 18, alpha).0);
            }
        }
        for y in 0..h {
            if y % 32 == 0 {
                self.refresh_cursor();
            }
            for x in 0..w {
                let mask = coverage(x, y, w, h, 16);
                if mask == 0 {
                    continue;
                }
                let sx = (x + HALO) as usize / 4;
                let sy = (y + HALO) as usize / 4;
                let mut pixel = if y >= TITLE_H {
                    // SAFETY: y-TITLE_H and x are bounded by accepted dimensions.
                    let content = unsafe {
                        CACHE[win.cache_slot][(y - TITLE_H) as usize * CONTENT_W + x as usize]
                    };
                    if content >> 24 == 255 {
                        content
                    } else {
                        // SAFETY: Only native content has alpha; native windows
                        // initialize the complete backdrop, including this index.
                        over(content, unsafe { BLUR_A[sy * blur_width + sx] })
                    }
                } else {
                    // SAFETY: Every window initializes at least its title backdrop.
                    over(title_color, unsafe { BLUR_A[sy * blur_width + sx] })
                };
                let inner = coverage(x - 1, y - 1, w - 2, h - 2, 15);
                let edge = mask.saturating_sub(inner);
                if edge > 0 {
                    pixel = over(edge_color, pixel);
                }
                if mask == 255 {
                    self.pixel(win.x + x, win.y + y, pixel & 0xffffff);
                } else {
                    self.blend(win.x + x, win.y + y, opacity(pixel, mask));
                }
            }
        }
        self.text(
            win.x + 18,
            win.y + 11,
            string(&win.title),
            if active { 0xf2f6ff } else { 0x9aaec5 },
        );
        // Close control keeps a fixed, observable hit target for every material.
        for y in -5..=5 {
            for x in -5..=5 {
                if x * x + y * y <= 25 {
                    self.pixel(win.x + w - 18 + x, win.y + 14 + y, 0xff6b79);
                }
            }
        }
    }
    fn blend(&mut self, x: i32, y: i32, color: u32) {
        if x < 0 || y < 0 || x >= self.info.width as i32 || y >= self.info.height as i32 {
            return;
        }
        // SAFETY: Clipped coordinates index the WM's exclusively owned BACK.
        let old = unsafe { BACK[y as usize * self.info.width + x as usize] | 0xff000000 };
        self.pixel(x, y, over(color, old) & 0xffffff);
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
        cursor: None,
        cursor_under: [0; 8 * 14],
    };
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
    let mut scene_dirty = true;
    let mut old_bounds = (0, 0, info.width, info.height);
    let mut focused = 0;
    let mut announced_focus = 0;
    display.damage.mark(0, 0, info.width, info.height);
    let mut old_mouse = None;
    loop {
        // Opaque service messages are decoded here in Ring 3, never in the kernel.
        // Each iteration is bounded even if a malicious client floods its channel.
        for _ in 0..64 {
            let Ok(Some((pid, message))) = sys::receive_message() else {
                break;
            };
            let existing = windows[..used]
                .iter()
                .position(|window| window.map(|w| w.pid) == Some(pid));
            match message & protocol::KIND {
                protocol::CONFIGURE => {
                    let config = Configuration::decode(message)
                        .filter(|c| c.fits_display(info.width, info.height));
                    if config.is_none()
                        || sys::surface_status(pid)
                            .map(|(_, dead)| dead)
                            .unwrap_or(true)
                        || existing
                            .map(|i| windows[i].unwrap().native)
                            .unwrap_or(false)
                    {
                        let _ = sys::send_message(pid, protocol::REJECT);
                        continue;
                    }
                    let index = if let Some(index) = existing {
                        index
                    } else {
                        if used == MAX {
                            let _ = sys::send_message(pid, protocol::REJECT);
                            continue;
                        }
                        let slot = free_slot(&windows[..used]);
                        windows[used] = Some(new_window(pid, EMPTY_APP, slot, used));
                        used += 1;
                        focused = pid;
                        focus(pid);
                        used - 1
                    };
                    let config = config.expect("validated configuration");
                    let window = windows[index].as_mut().expect("registered window");
                    window.width = config.width;
                    window.height = config.height;
                    window.native = true;
                    window.material = config.material;
                    window.reply = Some(protocol::ACCEPT);
                    window.focus_event = Some(window.pid == focused);
                    window.x = window.x.clamp(0, info.width as i32 - window.width as i32);
                    window.y = window
                        .y
                        .clamp(28, info.height as i32 - 32 - TITLE_H - window.height as i32);
                    // SAFETY: The cache slot uniquely belongs to this live window.
                    // Clear it before exposing native alpha pixels or reusing it.
                    unsafe {
                        CACHE[window.cache_slot].fill(0);
                    }
                    scene_dirty = true;
                }
                protocol::TITLE => {
                    if let Some(index) = existing {
                        let window = windows[index].as_mut().expect("registered window");
                        let chunk = ((message >> 24) & 15) as usize;
                        if window.native && chunk < 8 {
                            for byte in 0..3 {
                                let value = ((message >> (byte * 8)) & 255) as u8;
                                window.title[chunk * 3 + byte] =
                                    if value == 0 || (32..127).contains(&value) {
                                        value
                                    } else {
                                        b'?'
                                    };
                            }
                            scene_dirty = true;
                        }
                    }
                }
                protocol::COMMIT => {
                    if let Some(index) = existing {
                        let window = windows[index].as_mut().expect("registered window");
                        if window.native && window.commit.is_none() {
                            window.commit = Some(message & protocol::SERIAL);
                        }
                    }
                }
                protocol::CLOSE => {
                    if let Some(index) = existing {
                        let window = windows[index].as_mut().expect("registered window");
                        if window.native {
                            window.closing = Some(sys::ticks());
                        }
                    }
                }
                _ => {}
            }
        }
        // Native exit/close frees the WM cache slot; legacy text apps retain their
        // final output until closed. Existing kernel frame/stack leaks are separate.
        let mut index = 0;
        while index < used {
            let window = windows[index].expect("registered window");
            let dead = sys::surface_status(window.pid)
                .map(|(_, dead)| dead)
                .unwrap_or(true);
            if window.native
                && (dead
                    || window
                        .closing
                        .map(|tick| sys::ticks().wrapping_sub(tick) >= 18)
                        .unwrap_or(false))
            {
                if !dead {
                    let _ = sys::kill(window.pid);
                }
                let _ = sys::waitpid(window.pid);
                for j in index..used - 1 {
                    windows[j] = windows[j + 1];
                }
                used -= 1;
                windows[used] = None;
                if focused == window.pid {
                    focused = if used > 0 {
                        windows[used - 1].unwrap().pid
                    } else {
                        0
                    };
                    focus(focused);
                }
                drag = None;
                scene_dirty = true;
            } else {
                index += 1;
            }
        }
        let (mx, my, buttons) = sys::mouse();
        let down = buttons & 1 != 0;
        let click = down && previous & 1 == 0;
        if click {
            let window_hit = (0..used).rev().find(|&i| {
                let w = windows[i].unwrap();
                hit(mx, my, w.x, w.y, w.width as i32, w.height as i32 + TITLE_H)
            });
            if window_hit.is_none() && my >= 40 && my < 120 {
                let index = ((mx - 18).max(0) / 100) as usize;
                if index < count && used < MAX {
                    let app = apps[index];
                    match sys::spawn(string(&app.path)) {
                        Ok(pid) => {
                            let slot = free_slot(&windows[..used]);
                            windows[used] = Some(new_window(pid, app, slot, used));
                            used += 1;
                            focused = pid;
                            focus(pid);
                            println!("[desktop] launched {} pid={}", string(&app.name), pid);
                            scene_dirty = true;
                        }
                        Err(()) => println!("[desktop] launch failed: {}", string(&app.path)),
                    }
                }
            } else if let Some(i) = window_hit {
                let w = windows[i].unwrap();
                for j in i..used - 1 {
                    windows[j] = windows[j + 1];
                }
                windows[used - 1] = Some(w);
                focused = w.pid;
                focus(w.pid);
                scene_dirty = true;
                if hit(mx, my, w.x + w.width as i32 - 33, w.y, 33, TITLE_H) {
                    if w.native && !w.exited {
                        let _ = sys::send_message(w.pid, protocol::CLOSE);
                        windows[used - 1].as_mut().unwrap().closing = Some(sys::ticks());
                    } else {
                        let _ = sys::kill(w.pid);
                        let _ = sys::waitpid(w.pid);
                        used -= 1;
                        windows[used] = None;
                        focused = if used > 0 {
                            windows[used - 1].unwrap().pid
                        } else {
                            0
                        };
                        focus(focused);
                    }
                } else if hit(mx, my, w.x, w.y, w.width as i32 - 33, TITLE_H) {
                    drag = Some((mx - w.x, my - w.y));
                    println!("[desktop] drag started: pid={}", w.pid);
                }
            } else {
                focused = 0;
                focus(0);
                scene_dirty = true;
            }
        }
        if !down {
            drag = None;
        }
        if let Some((dx, dy)) = drag {
            if used > 0 {
                let w = windows[used - 1].as_mut().unwrap();
                let x = (mx - dx).clamp(0, info.width as i32 - w.width as i32);
                let y = (my - dy).clamp(28, info.height as i32 - 32 - TITLE_H - w.height as i32);
                scene_dirty |= (w.x, w.y) != (x, y);
                (w.x, w.y) = (x, y);
            }
        }
        if focused != announced_focus {
            for window in windows[..used].iter_mut().flatten().filter(|w| w.native) {
                window.focus_event = Some(window.pid == focused);
                window.pointer = None;
            }
            announced_focus = focused;
            scene_dirty = true;
        }
        let mut compose = scene_dirty;
        for window in windows[..used].iter_mut().flatten() {
            compose |= display.update_surface(window);
            // Frame/config acknowledgements have priority over coalesced motion.
            if let Some(reply) = window.reply {
                if sys::send_message(window.pid, reply).is_ok() {
                    window.reply = None;
                }
            }
            if let Some(focused) = window.focus_event {
                if sys::send_message(window.pid, protocol::FOCUS | u32::from(focused)).is_ok() {
                    window.focus_event = None;
                }
            }
            if window.native && window.pid == focused && drag.is_none() {
                let pointer = (
                    (mx - window.x).clamp(-1024, 1023),
                    (my - window.y - TITLE_H).clamp(-1024, 1023),
                    buttons,
                );
                if window.pointer != Some(pointer) {
                    let payload = protocol::POINTER
                        | (pointer.0 as u32 & 2047)
                        | ((pointer.1 as u32 & 2047) << 11)
                        | (u32::from(buttons & 7) << 22);
                    if sys::send_message(window.pid, payload).is_ok() {
                        window.pointer = Some(pointer);
                    }
                }
            }
        }
        if compose {
            let mut bounds = (info.width, info.height, 0, 0);
            for window in windows[..used].iter().flatten() {
                bounds.0 = bounds.0.min((window.x - 12).max(0) as usize);
                bounds.1 = bounds.1.min((window.y - 8).max(0) as usize);
                bounds.2 = bounds
                    .2
                    .max((window.x as usize + window.width + 12).min(info.width));
                bounds.3 = bounds.3.max(
                    (window.y as usize + window.height + TITLE_H as usize + 16).min(info.height),
                );
            }
            // Restore and compose bottom-to-top on *any* content change. Glass
            // depends on lower layers even if its own client's revision is stable.
            // This also prevents repeatedly blurring the previous glass result.
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
            for window in windows[..used].iter().flatten() {
                display.window(window, window.pid == focused);
            }
            old_bounds = bounds;
            scene_dirty = false;
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
fn free_slot(windows: &[Option<Window>]) -> usize {
    (0..MAX)
        .find(|slot| !windows.iter().flatten().any(|w| w.cache_slot == *slot))
        .expect("free cache slot")
}
fn new_window(pid: u32, app: App, cache_slot: usize, index: usize) -> Window {
    let mut title = [0; protocol::TITLE_BYTES];
    title[..12].copy_from_slice(&app.name);
    Window {
        pid,
        app,
        x: 80 + index as i32 * 12,
        y: 126 + (index as i32 * 2).min(6),
        exited: false,
        cache_slot,
        revision: None,
        width: 488,
        height: 288,
        title,
        native: false,
        material: Material::Glass,
        commit: None,
        reply: None,
        pointer: None,
        closing: None,
        focus_event: None,
    }
}
