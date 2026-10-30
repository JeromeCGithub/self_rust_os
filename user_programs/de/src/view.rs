//! Desktop presentation; draws only through the SDK canvas.
use crate::applications::{string, App};
use userlib::{
    sys,
    ui::{self, Canvas, Color, Rect},
};
pub(super) struct Painter<'a, 'b> {
    pub(super) canvas: &'a mut Canvas<'b>,
    pub(super) info: sys::FbInfo,
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
    pub(super) fn background(&mut self, wallpaper: Option<&[u8]>) {
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
    pub(super) fn desktop(&mut self, apps: &[App], wallpaper: Option<&[u8]>) {
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
