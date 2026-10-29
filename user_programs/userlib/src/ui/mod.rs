//! Safe, allocation-free windows and immediate-mode widgets for Ring 3 apps.
//!
//! The compositor owns placement, decorations and materials. Applications draw
//! content-local pixels and keep submitted pixels unchanged until RELEASE.
mod input;
pub mod protocol;
pub mod raster;
use crate::sys;
use core::sync::atomic::{AtomicBool, Ordering};
use font8x8::{UnicodeFonts, BASIC_FONTS};
use input::Input;
pub use protocol::Material;
use protocol::*;
pub use raster::{Canvas, Color, Rect};

// One mapped drawing surface exists per process. Acquire prevents two safe
// Window handles from aliasing it, including after an earlier handle is dropped.
static WINDOW_TAKEN: AtomicBool = AtomicBool::new(false);

/// Window creation, transport or presentation failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidGeometry,
    InvalidTitle,
    AlreadyConnected,
    Unavailable,
    Timeout,
    Rejected,
    Closed,
}

/// Content-local input and window lifecycle events.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Pointer { x: i32, y: i32, buttons: u8 },
    Focus(bool),
    CloseRequested,
}

/// Requested properties. The WM chooses placement and draws decorations.
pub struct WindowOptions<'a> {
    title: &'a str,
    width: usize,
    height: usize,
    material: Material,
}
impl<'a> WindowOptions<'a> {
    /// A glass window with 488x288 content by default.
    pub const fn new(title: &'a str) -> Self {
        Self {
            title,
            width: 488,
            height: 288,
            material: Material::Glass,
        }
    }
    /// Requests content dimensions (160..=640 by 96..=400 pixels).
    pub const fn content_size(mut self, width: usize, height: usize) -> Self {
        self.width = width;
        self.height = height;
        self
    }
    /// Requests a compositor background material.
    pub const fn background(mut self, material: Material) -> Self {
        self.material = material;
        self
    }
}

/// One owned window per process, backed by the process's existing private surface.
pub struct Window {
    config: Configuration,
    stride: usize,
    pending: Option<u32>,
    serial: u32,
    accepted: bool,
    closed: bool,
    rejected: bool,
    input: Input,
}
impl Window {
    /// Connects to the WM and negotiates content geometry. No raw pointers escape.
    pub fn create(options: WindowOptions<'_>) -> Result<Self, Error> {
        let config = Configuration {
            width: options.width,
            height: options.height,
            material: options.material,
        };
        if Configuration::decode(config.encode()) != Some(config) {
            return Err(Error::InvalidGeometry);
        }
        if options.title.len() > TITLE_BYTES
            || !options.title.bytes().all(|b| (32..127).contains(&b))
        {
            return Err(Error::InvalidTitle);
        }
        let info = sys::fb_info().map_err(|_| Error::Unavailable)?;
        if info.bpp != 32
            || info.pitch % 4 != 0
            || info.pitch < info.width.checked_mul(4).ok_or(Error::InvalidGeometry)?
            || !config.fits_display(info.width, info.height)
        {
            return Err(Error::InvalidGeometry);
        }
        // Require the complete mapped range, not an address-range guess.
        let bytes = info
            .pitch
            .checked_mul(info.height)
            .ok_or(Error::InvalidGeometry)?;
        if sys::FRAMEBUFFER_ADDR
            .checked_add(bytes)
            .filter(|end| *end <= 0x4000000)
            .is_none()
        {
            return Err(Error::InvalidGeometry);
        }
        if WINDOW_TAKEN
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return Err(Error::AlreadyConnected);
        }
        sys::graphics_mode();
        let mut window = Self {
            config,
            stride: info.pitch / 4,
            pending: None,
            serial: 0,
            accepted: false,
            closed: false,
            rejected: false,
            input: Input::default(),
        };
        send(config.encode())?;
        let mut title = [0u8; TITLE_BYTES];
        title[..options.title.len()].copy_from_slice(options.title.as_bytes());
        for (index, chunk) in title.chunks_exact(3).enumerate() {
            send(
                TITLE
                    | ((index as u32) << 24)
                    | u32::from(chunk[0])
                    | (u32::from(chunk[1]) << 8)
                    | (u32::from(chunk[2]) << 16),
            )?;
        }
        window.wait(false)?;
        Ok(window)
    }
    /// Content width in pixels.
    pub fn width(&self) -> u32 {
        self.config.width as u32
    }
    /// Content height in pixels.
    pub fn height(&self) -> u32 {
        self.config.height as u32
    }
    /// Receives an event without blocking. Frame acknowledgements are internal.
    pub fn poll_event(&mut self) -> Result<Option<Event>, Error> {
        let Some((sender, message)) = sys::receive_message().map_err(|_| Error::Unavailable)?
        else {
            return Ok(None);
        };
        if sender != 0 {
            return Err(Error::Unavailable);
        }
        match message & KIND {
            ACCEPT => self.accepted = true,
            REJECT => self.rejected = true,
            RELEASE if self.pending == Some(message & SERIAL) => self.pending = None,
            POINTER => {
                let x = ((message << 21) as i32) >> 21;
                let y = ((message << 10) as i32) >> 21;
                let buttons = ((message >> 22) & 7) as u8;
                self.input.pointer(x, y, buttons & 1 != 0);
                return Ok(Some(Event::Pointer { x, y, buttons }));
            }
            FOCUS => {
                let focused = message & 1 != 0;
                if !focused {
                    self.input.unfocus();
                }
                return Ok(Some(Event::Focus(focused)));
            }
            CLOSE => {
                self.closed = true;
                return Ok(Some(Event::CloseRequested));
            }
            _ => {}
        }
        Ok(None)
    }
    fn wait(&mut self, released: bool) -> Result<(), Error> {
        let start = sys::ticks();
        while !self.accepted || (released && self.pending.is_some()) {
            for _ in 0..16 {
                self.poll_event()?;
            }
            if self.rejected {
                return Err(Error::Rejected);
            }
            if self.closed {
                return Err(Error::Closed);
            }
            if sys::ticks().wrapping_sub(start) >= 36 {
                return Err(Error::Timeout);
            }
            if !self.accepted || (released && self.pending.is_some()) {
                sys::sleep_ticks(1).map_err(|_| Error::Unavailable)?;
            }
        }
        if self.closed {
            return Err(Error::Closed);
        }
        Ok(())
    }
    /// Clears to transparent, draws one frame, and submits it. The next call
    /// waits for the WM's copy acknowledgement before modifying the buffer.
    pub fn draw(&mut self, paint: impl FnOnce(&mut Canvas<'_>)) -> Result<(), Error> {
        self.wait(true)?;
        let words = (self.config.height - 1) * self.stride + self.config.width;
        // SAFETY: The kernel maps the complete private pitch*height surface at
        // FRAMEBUFFER_ADDR. Creation validates the returned geometry and subset.
        // This process has only one Window, console rendering is disabled, and
        // the previous submission was released. u32 alignment is page alignment.
        // The exclusive slice exists only inside this synchronous draw call and
        // cannot escape via the closure; it ends before publication/scheduling.
        {
            let pixels = unsafe {
                core::slice::from_raw_parts_mut(sys::FRAMEBUFFER_ADDR as *mut u32, words)
            };
            let mut canvas =
                Canvas::new(pixels, self.config.width, self.config.height, self.stride)
                    .ok_or(Error::InvalidGeometry)?;
            canvas.clear(Color::TRANSPARENT);
            paint(&mut canvas);
        }
        self.serial = (self.serial + 1) & SERIAL;
        sys::present_surface().map_err(|_| Error::Unavailable)?;
        self.pending = Some(self.serial);
        send(COMMIT | self.serial)
    }
    /// Runs a widget view on initial display and input changes; idle waits yield
    /// to the scheduler. Labels used as button IDs must be unique within a view.
    pub fn run(mut self, mut view: impl FnMut(&mut Ui<'_, '_>)) -> Result<(), Error> {
        let mut dirty = true;
        loop {
            for _ in 0..16 {
                if let Some(event) = self.poll_event()? {
                    if event == Event::CloseRequested {
                        return Ok(());
                    }
                    dirty = true;
                }
            }
            if dirty {
                self.wait(true)?;
                let mut input = core::mem::take(&mut self.input);
                self.draw(|canvas| {
                    let mut ui = Ui {
                        canvas,
                        input: &mut input,
                        x: 24,
                        y: 24,
                    };
                    view(&mut ui);
                })?;
                input.finish_frame();
                self.input = input;
                dirty = false;
            }
            sys::sleep_ticks(1).map_err(|_| Error::Unavailable)?;
        }
    }
}
impl Drop for Window {
    fn drop(&mut self) {
        // Best effort: process exit independently releases the WM's cache slot.
        let _ = sys::send_message(0, CLOSE);
    }
}
fn send(message: u32) -> Result<(), Error> {
    let start = sys::ticks();
    loop {
        match sys::send_message(0, message) {
            Ok(()) => return Ok(()),
            Err(sys::MessageError::Unavailable) => return Err(Error::Unavailable),
            Err(sys::MessageError::Full) => {
                if sys::ticks().wrapping_sub(start) >= 36 {
                    return Err(Error::Timeout);
                }
                sys::sleep_ticks(1).map_err(|_| Error::Unavailable)?;
            }
        }
    }
}

/// Draws sharp text above the compositor's material. Returns the next x position.
pub fn text(canvas: &mut Canvas<'_>, x: i32, y: i32, text: &str, color: Color, scale: u32) -> i32 {
    let scale = scale.clamp(1, 4);
    let mut cursor = x;
    for ch in text.chars() {
        if let Some(glyph) = BASIC_FONTS.get(ch) {
            canvas.glyph(cursor, y, &glyph, color, scale);
        }
        cursor = cursor.saturating_add(8 * scale as i32);
    }
    cursor
}

/// Immediate-mode vertical layout. All drawing is clipped to window content.
pub struct Ui<'a, 'buffer> {
    canvas: &'a mut Canvas<'buffer>,
    input: &'a mut Input,
    x: i32,
    y: i32,
}
impl<'a, 'buffer> Ui<'a, 'buffer> {
    /// Large heading in the foreground text color.
    pub fn heading(&mut self, label: &str) {
        text(self.canvas, self.x, self.y, label, Color::rgb(0xf6f9ff), 2);
        self.y += 36;
    }
    /// Secondary text in the current column.
    pub fn label(&mut self, label: &str) {
        text(self.canvas, self.x, self.y, label, Color::rgb(0xc8d8e9), 1);
        self.y += 22;
    }
    /// Adds a translucent information card to the current column.
    pub fn card(&mut self, title: &str, detail: &str) {
        let width = self.canvas.width().saturating_sub(48);
        self.canvas.rounded_rect(
            Rect::new(self.x, self.y, width, 72),
            14,
            Color::rgba(225, 240, 255, 20),
        );
        text(
            self.canvas,
            self.x + 16,
            self.y + 16,
            title,
            Color::rgb(0xe7f2ff),
            1,
        );
        text(
            self.canvas,
            self.x + 16,
            self.y + 40,
            detail,
            Color::rgb(0xb7cbe1),
            1,
        );
        self.y += 88;
    }
    /// Adds vertical whitespace.
    pub fn space(&mut self, pixels: u32) {
        self.y = self.y.saturating_add(pixels.min(400) as i32);
    }
    /// Groups content in the current vertical layout.
    pub fn column(&mut self, content: impl FnOnce(&mut Self)) {
        content(self);
    }
    /// A rounded, keyboard-independent pointer button. Activation occurs on
    /// release inside a button that captured the press.
    pub fn button(&mut self, label: &str) -> Response {
        let bounds = Rect::new(self.x, self.y, self.canvas.width().saturating_sub(48), 38);
        let id = label.bytes().fold(2166136261u32, |hash, byte| {
            (hash ^ u32::from(byte)).wrapping_mul(16777619)
        });
        let clicked = self.input.button(id, bounds);
        let hovered = bounds.contains(self.input.x, self.input.y);
        let color = if hovered && self.input.down {
            Color::rgba(125, 185, 255, 160)
        } else if hovered {
            Color::rgba(150, 200, 255, 110)
        } else {
            Color::rgba(150, 190, 235, 62)
        };
        self.canvas
            .rounded_rect(bounds, 11, Color::rgba(220, 237, 255, 100));
        self.canvas.rounded_rect(
            Rect::new(bounds.x + 1, bounds.y + 1, bounds.width - 2, 36),
            10,
            color,
        );
        text(
            self.canvas,
            self.x + 14,
            self.y + 15,
            label,
            Color::rgb(0xffffff),
            1,
        );
        self.y += 50;
        Response { clicked }
    }
    /// Access custom clipped drawing without giving up the window API.
    pub fn canvas(&mut self) -> &mut Canvas<'buffer> {
        self.canvas
    }
}
/// Observable result of a widget interaction.
pub struct Response {
    clicked: bool,
}
impl Response {
    pub fn clicked(&self) -> bool {
        self.clicked
    }
}
