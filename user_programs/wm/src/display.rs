//! Framebuffer composition, surface snapshots, decorations and cursor presentation.
use crate::window::{Window, MAX};
use crate::{damage::Damage, glass};
use font8x8::{UnicodeFonts, BASIC_FONTS};
use userlib::{
    sys,
    ui::{
        protocol::{self, Material},
        raster::{coverage, opacity, over, Color},
    },
};
pub(super) const MAX_W: usize = 1920;
pub(super) const MAX_H: usize = 1080;
pub(super) const MAX_PITCH: usize = 8192;
pub(super) struct Compositor {
    pub(super) info: sys::FbInfo,
    pub(super) damage: Damage,
    pub(super) cursor: Option<(i32, i32)>,
    pub(super) cursor_under: [u32; 8 * 14],
}
static mut BACK: [u32; MAX_PITCH / 4 * MAX_H] = [0; MAX_PITCH / 4 * MAX_H];
static mut BASE: [u32; MAX_W * MAX_H] = [0; MAX_W * MAX_H];
static mut SURFACE: [u32; MAX_PITCH / 4 * 480] = [0; MAX_PITCH / 4 * 480];
pub(super) const CONTENT_W: usize = protocol::MAX_WIDTH;
pub(super) const CONTENT_H: usize = protocol::MAX_HEIGHT;
pub(super) const TITLE_H: i32 = 28;
pub(super) const HALO: i32 = 16;
pub(super) const BLUR_LEN: usize = 168 * 115;
static mut CACHE: [[u32; CONTENT_W * CONTENT_H]; MAX] = [[0; CONTENT_W * CONTENT_H]; MAX];
static mut BLUR_A: [u32; BLUR_LEN] = [0; BLUR_LEN];
static mut BLUR_B: [u32; BLUR_LEN] = [0; BLUR_LEN];
pub(super) fn string(bytes: &[u8]) -> &str {
    core::str::from_utf8(&bytes[..bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len())])
        .unwrap_or("")
}
impl Compositor {
    pub(super) fn clear_background(&mut self) {
        // SAFETY: Validated display dimensions bound this exclusively WM-owned
        // packed cache. No shell/user references alias it or survive this fill.
        unsafe {
            (&mut *core::ptr::addr_of_mut!(BASE))[..self.info.width * self.info.height]
                .fill(0x0e172b);
        }
    }
    pub(super) fn pixel(&mut self, x: i32, y: i32, c: u32) {
        if x >= 0 && y >= 0 && x < self.info.width as i32 && y < self.info.height as i32 {
            // SAFETY: Validated dimensions and the coordinate bounds keep
            // this index inside BACK. Only this compositor writes its buffer.
            unsafe {
                let pixel = core::ptr::addr_of_mut!(BACK)
                    .cast::<u32>()
                    .add(y as usize * (self.info.pitch / 4) + x as usize);
                if pixel.read() != c {
                    pixel.write(c);
                    self.damage.mark(x as usize, y as usize, 1, 1);
                }
            }
        }
    }
    pub(super) fn present(&mut self, _mx: i32, _my: i32) {
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
                        .add(y * (info.pitch / 4) + x),
                    (sys::FRAMEBUFFER_ADDR + y * info.pitch + x * 4) as *mut u32,
                    width,
                );
            }
        });
        let (mx, my, _) = sys::mouse();
        self.draw_cursor(mx, my);
    }
    pub(super) fn restore_cursor(&mut self) {
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
    pub(super) fn draw_cursor(&mut self, mx: i32, my: i32) {
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
    pub(super) fn refresh_cursor(&mut self) {
        let (x, y, _) = sys::mouse();
        if self.cursor != Some((x, y)) {
            self.restore_cursor();
            self.draw_cursor(x, y);
        }
    }

    pub(super) fn text(&mut self, x: i32, y: i32, s: &str, c: u32) {
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
impl Compositor {
    pub(super) fn restore_background(&mut self, bounds: (usize, usize, usize, usize)) {
        let (left, top, right, bottom) = bounds;
        for y in top..bottom {
            if y % 32 == 0 {
                self.refresh_cursor();
            }
            // SAFETY: Clipped workspace bounds fit the exclusively owned packed
            // BASE and pitched BACK buffers. The copies are disjoint; no borrow
            // survives presentation. The shell's frame was cached before RELEASE.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    core::ptr::addr_of!(BASE)
                        .cast::<u32>()
                        .add(y * self.info.width + left),
                    core::ptr::addr_of_mut!(BACK)
                        .cast::<u32>()
                        .add(y * (self.info.pitch / 4) + left),
                    right - left,
                );
            }
        }
        self.damage.mark(
            left,
            top,
            right.saturating_sub(left),
            bottom.saturating_sub(top),
        );
    }
    pub(super) fn update_surface(&self, win: &mut Window) -> bool {
        let status = sys::surface_status(win.pid);
        win.exited = status.map(|(_, exited)| exited).unwrap_or(true);
        if win.native && win.commit.is_none() {
            return false;
        }
        if !win.native && win.revision == status.ok() && win.revision.is_some() {
            return false;
        }
        let scale = !win.native && win.large;
        let words = if scale {
            self.info.pitch / 4 * 480
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
                let sx = if scale { x * 640 / win.width } else { x };
                let sy = if scale { y * 480 / win.height } else { y };
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
    pub(super) fn window(&mut self, win: &Window, active: bool) {
        let w = win.width as i32;
        let h = win.height as i32 + TITLE_H;
        let blur_width = (w + HALO * 2 + 3) as usize / 4;
        let blur_height = ((if win.native { h } else { TITLE_H }) + HALO * 2 + 3) as usize / 4;
        let title_color = Color::rgba(225, 239, 255, if active { 10 } else { 5 }).0;
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
                    BLUR_A[y * blur_width + x] = BACK[sy * (self.info.pitch / 4) + sx];
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
                *pixel = if win.material == Material::Opaque {
                    0xff101b2c
                } else {
                    glass::shade(*pixel, win.material == Material::GlassClear)
                };
            }
        }
        // A restrained shadow outside the rounded window's bounding box.
        for y in -8..h + 16 {
            for x in -12..w + 12 {
                if x >= 0 && y >= 0 && x < w && y < h {
                    continue;
                }
                let dx = (-x).max(x - w + 1).max(0);
                let dy = (-y + 2).max(y - h - 2).max(0);
                let alpha = ((256 - dx * dx - dy * dy).max(0) / 12).min(21) as u8;
                self.blend(win.x + x, win.y + y, Color::rgba(3, 8, 18, alpha).0);
            }
        }
        for y in 0..h {
            if y % 32 == 0 {
                self.refresh_cursor();
            }
            for x in 0..w {
                let mask = coverage(x, y, w, h, 20);
                if mask == 0 {
                    continue;
                }
                let sx = (x + HALO) as usize;
                let sy = (y + HALO) as usize;
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
                        over(content, unsafe {
                            glass::sample(
                                &*core::ptr::addr_of!(BLUR_A),
                                blur_width,
                                blur_height,
                                sx,
                                sy,
                            )
                        })
                    }
                } else {
                    // SAFETY: Every window initializes at least its title backdrop.
                    over(title_color, unsafe {
                        glass::sample(
                            &*core::ptr::addr_of!(BLUR_A),
                            blur_width,
                            blur_height,
                            sx,
                            sy,
                        )
                    })
                };
                let inner = coverage(x - 1, y - 1, w - 2, h - 2, 19);
                let edge = mask.saturating_sub(inner);
                if edge > 0 {
                    pixel = over(
                        Color::rgba(225, 240, 255, glass::rim(x, y, w, h, active)).0,
                        pixel,
                    );
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
    pub(super) fn blend(&mut self, x: i32, y: i32, color: u32) {
        if x < 0 || y < 0 || x >= self.info.width as i32 || y >= self.info.height as i32 {
            return;
        }
        // SAFETY: Clipped coordinates index the WM's exclusively owned BACK.
        let old = unsafe { BACK[y as usize * (self.info.pitch / 4) + x as usize] | 0xff000000 };
        self.pixel(x, y, over(color, old) & 0xffffff);
    }
}

impl Compositor {
    /// Copy a submitted desktop frame before the service releases its producer.
    pub(super) fn snapshot_desktop(&mut self, pid: u32) -> bool {
        let info = self.info;
        // SAFETY: Validated display geometry bounds the complete
        // pitched BACK buffer. The shell keeps submitted pixels
        // immutable. Snapshot ends before copying into packed BASE;
        // only PID 0 writes these disjoint buffers. Cache ownership
        // transfers before RELEASE, permitting later shell redraws.
        let result = unsafe {
            sys::snapshot(
                pid,
                &mut (&mut *core::ptr::addr_of_mut!(BACK))[..info.pitch / 4 * info.height],
            )
        };
        if result != Ok(false) {
            return false;
        }
        // SAFETY: These disjoint static allocations are owned
        // exclusively by the compositor; the snapshot borrow has
        // ended. The safe copier validates both complete layouts
        // and preserves padding before the frame is acknowledged.
        let copied = unsafe {
            userlib::ui::raster::copy_surface(
                &*core::ptr::addr_of!(BACK),
                &mut *core::ptr::addr_of_mut!(BASE),
                info.width,
                info.height,
                info.pitch / 4,
                info.width,
            )
        };
        copied
    }
    /// A live window owns its cache slot until removal from the session.
    pub(super) fn reset_content(&mut self, window: &Window) {
        // SAFETY: The session assigns a unique in-range slot to every live window.
        unsafe {
            CACHE[window.cache_slot].fill(0);
        }
    }
}
