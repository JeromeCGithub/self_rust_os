//! Desktop shell: wallpaper, panels, launchers and application discovery in Ring 3.
#![no_std]
#![no_main]
use userlib::{
    println, sys,
    ui::{self, Canvas, Color, DesktopSurface, Event, Rect},
};
const MAX: usize = 6;
const WALL_HEADER: &[u8] = b"P6\n1920 1080\n255\n";
const WALL_BYTES: usize = 17 + 1920 * 1080 * 3;
static mut WALL: [u8; WALL_BYTES] = [0; WALL_BYTES];
#[derive(Clone, Copy)]
struct App {
    name: [u8; 12],
    path: [u8; 32],
}
const EMPTY_APP: App = App {
    name: [0; 12],
    path: [0; 32],
};
fn string(bytes: &[u8]) -> &str {
    core::str::from_utf8(&bytes[..bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len())])
        .unwrap_or("")
}
fn discover(apps: &mut [App; MAX]) -> usize {
    let mut entries = [0u8; 12 * 32];
    let mut count = 0;
    if let Some(n) = sys::ls("/BIN", &mut entries, 32) {
        for e in entries.chunks_exact(12).take(n.min(32)) {
            if &e[8..11] != b"BIN"
                || (&e[..8] == b"WM      " || &e[..8] == b"DE      ")
                || count == MAX
            {
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

struct Painter<'a, 'b> {
    canvas: &'a mut Canvas<'b>,
    info: sys::FbInfo,
}
impl Painter<'_, '_> {
    fn pixel(&mut self, x: i32, y: i32, color: u32) {
        self.canvas.pixel(x, y, Color::rgb(color));
    }
    fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: u32) {
        self.canvas.fill_rect(
            Rect::new(x, y, w.max(0) as u32, h.max(0) as u32),
            Color::rgb(color),
        );
    }
    fn text(&mut self, x: i32, y: i32, text: &str, color: u32) {
        ui::text(self.canvas, x, y, text, Color::rgb(color), 1);
    }
    fn background(&mut self, wallpaper: Option<&[u8]>) {
        // Restore only the workspace exposed by a changed window layout.
        let (left, top, right, bottom) = (0, 0, self.info.width, self.info.height);
        for y in top..bottom {
            let wall_row = (y * 1080 / self.info.height) * 1920 * 3;
            for x in left..right {
                let c = if let Some(wallpaper) = wallpaper {
                    let i = wall_row + (x * 1920 / self.info.width) * 3;
                    ((wallpaper[i] as u32) << 16)
                        | ((wallpaper[i + 1] as u32) << 8)
                        | wallpaper[i + 2] as u32
                } else {
                    0x16364e
                };
                self.pixel(x as i32, y as i32, c);
            }
        }
    }
    fn desktop(&mut self, apps: &[App], wallpaper: Option<&[u8]>) {
        self.background(wallpaper);
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
            self.canvas.rounded_rect(
                Rect::new(x + 8, 44, 52, 48),
                14,
                Color::rgba(225, 240, 255, 34),
            );
            self.canvas.rounded_rect(
                Rect::new(x + 10, 46, 48, 44),
                12,
                Color::rgba(15, 26, 48, 190),
            );
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
        self.text(
            self.info.width as i32 / 2 - 160,
            self.info.height as i32 - 19,
            "NASA, ESA, CSA, and STScI",
            0xb6cadf,
        );
    }
}

#[no_mangle]
pub extern "C" fn user_main() {
    let info = sys::fb_info().expect("desktop geometry");
    let mut surface = DesktopSurface::connect().expect("desktop role");
    surface
        .reserve_work_area(28, 32)
        .expect("desktop workspace");
    // SAFETY: This process alone owns WALL, borrowed exclusively for the
    // synchronous file read. The borrow ends before immutable render access;
    // the loaded size/header are validated before reading any RGB payload.
    let loaded = unsafe {
        sys::read_file("/ETC/WALL.PPM", &mut *core::ptr::addr_of_mut!(WALL)) == Some(WALL_BYTES)
            && &WALL[..17] == WALL_HEADER
    };
    println!("[desktop] wallpaper loaded: {}", loaded);
    // Publish the wallpaper promptly, then the discovered shell controls. The
    // second draw must wait for the first cached copy's RELEASE before writing.
    surface
        .draw(|canvas| {
            // SAFETY: Same immutable validated payload as the complete desktop draw.
            let wallpaper = if loaded {
                Some(unsafe { &(&*core::ptr::addr_of!(WALL))[17..] })
            } else {
                None
            };
            Painter { canvas, info }.background(wallpaper);
        })
        .expect("wallpaper frame");
    let mut apps = [EMPTY_APP; MAX];
    let count = discover(&mut apps);
    surface
        .draw(|canvas| {
            // SAFETY: WALL stays immutable after the completed read. These bounds
            // are the validated P6 payload. No aliases can mutate it during drawing.
            let wallpaper = if loaded {
                Some(unsafe { &(&*core::ptr::addr_of!(WALL))[17..] })
            } else {
                None
            };
            Painter { canvas, info }.desktop(&apps[..count], wallpaper);
        })
        .expect("desktop frame");
    println!("[desktop] ready: {} applications", count);
    let mut previous = 0;
    loop {
        for _ in 0..16 {
            match surface.poll_event().expect("desktop events") {
                Some(Event::Pointer { x, y, buttons }) => {
                    let click = buttons & 1 != 0 && previous & 1 == 0;
                    previous = buttons;
                    if click && (40..120).contains(&y) && x >= 18 {
                        let index = ((x - 18) / 100) as usize;
                        if index < count {
                            let app = apps[index];
                            match sys::spawn(string(&app.path)) {
                                Ok(pid) => {
                                    // Executable-specific compatibility policy stays
                                    // in the shell, outside the compositor.
                                    let large = string(&app.name) == "SNAKE";
                                    if surface.manage(pid, string(&app.name), large).is_ok() {
                                        println!(
                                            "[desktop] launched {} pid={}",
                                            string(&app.name),
                                            pid
                                        );
                                    } else {
                                        let _ = sys::kill(pid);
                                        let _ = sys::waitpid(pid);
                                        println!("[desktop] window request rejected");
                                    }
                                }
                                Err(()) => {
                                    println!("[desktop] launch failed: {}", string(&app.path))
                                }
                            }
                        }
                    }
                }
                Some(Event::CloseRequested) => sys::exit(0),
                _ => {}
            }
        }
        sys::sleep_ticks(1).expect("desktop idle");
    }
}
