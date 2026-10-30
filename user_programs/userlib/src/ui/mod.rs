//! Safe, allocation-free windows and immediate-mode widgets for Ring 3 apps.
//!
//! The compositor owns placement, decorations and materials. Applications draw
//! content-local pixels and keep submitted pixels unchanged until RELEASE.
pub mod declarative;
pub mod desktop_protocol;
mod input;
pub use declarative::{Style, View};
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
    InvalidView(declarative::RenderError),
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
    desktop: bool,
}
impl Window {
    /// Connects to the WM and negotiates content geometry. No raw pointers escape.
    pub fn create(options: WindowOptions<'_>) -> Result<Self, Error> {
        Self::connect(options, false)
    }
    fn connect(options: WindowOptions<'_>, desktop: bool) -> Result<Self, Error> {
        let info = sys::fb_info().map_err(|_| Error::Unavailable)?;
        let config = Configuration {
            width: if desktop { info.width } else { options.width },
            height: if desktop { info.height } else { options.height },
            material: options.material,
        };
        if !desktop && Configuration::decode(config.encode()) != Some(config) {
            return Err(Error::InvalidGeometry);
        }
        if options.title.len() > TITLE_BYTES
            || !options.title.bytes().all(|b| (32..127).contains(&b))
        {
            return Err(Error::InvalidTitle);
        }
        if info.bpp != 32
            || info.pitch % 4 != 0
            || info.pitch < info.width.checked_mul(4).ok_or(Error::InvalidGeometry)?
            || (!desktop && !config.fits_display(info.width, info.height))
            || info.width > 1920
            || info.height > 1080
            || info.width == 0
            || info.height == 0
            || info.pitch > 8192
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
            desktop,
        };
        send(if desktop {
            desktop_protocol::DESKTOP
        } else {
            config.encode()
        })?;
        let mut title = [0u8; TITLE_BYTES];
        title[..options.title.len()].copy_from_slice(options.title.as_bytes());
        for (index, chunk) in title.chunks_exact(3).enumerate().filter(|_| !desktop) {
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
            desktop_protocol::DESKTOP_POINTER if self.desktop => {
                let (x, y, buttons) = desktop_protocol::decode_pointer(message);
                return Ok(Some(Event::Pointer { x, y, buttons }));
            }
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
        self.draw_frame(|canvas| {
            paint(canvas);
            Ok(())
        })
    }
    fn draw_frame(
        &mut self,
        paint: impl FnOnce(&mut Canvas<'_>) -> Result<(), Error>,
    ) -> Result<(), Error> {
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
            paint(&mut canvas)?;
        }
        self.serial = (self.serial + 1) & SERIAL;
        sys::present_surface().map_err(|_| Error::Unavailable)?;
        self.pending = Some(self.serial);
        send(COMMIT | self.serial)
    }
    /// Runs a widget view on initial display and input changes; idle waits yield
    /// to the scheduler. Labels used as button IDs must be unique within a view.
    pub fn run(self, mut view: impl FnMut(&mut Ui<'_, '_>)) -> Result<(), Error> {
        self.run_frames(|ui| {
            view(ui);
            Ok(())
        })
    }
    /// Mounts pure component functions and an explicit state-update handler.
    /// Actions are delivered once after rendering, then the new state is painted
    /// before publication. Components return ordinary typed values, without a heap.
    pub fn mount<S, M: Clone, V: View<M>>(
        self,
        mut state: S,
        view: impl Fn(&S) -> V,
        mut update: impl FnMut(&mut S, M),
    ) -> Result<(), Error> {
        self.run_frames(|ui| {
            let action = render_component(ui, &view(&state))?;
            if let Some(message) = action {
                ui.input.finish_frame();
                update(&mut state, message);
                ui.canvas.clear(Color::TRANSPARENT);
                render_component(ui, &view(&state))?;
            }
            Ok(())
        })
    }
    fn run_frames(
        mut self,
        mut view: impl FnMut(&mut Ui<'_, '_>) -> Result<(), Error>,
    ) -> Result<(), Error> {
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
                self.draw_frame(|canvas| {
                    let mut ui = Ui {
                        canvas,
                        input: &mut input,
                        x: 24,
                        y: 24,
                    };
                    view(&mut ui)
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

/// A shell-owned background layer. Only the compositor-spawned DE may connect.
/// Uses the same private mapping and copy/release lifetime as ordinary windows.
pub struct DesktopSurface {
    window: Window,
    pending_event: Option<Event>,
}
impl DesktopSurface {
    /// Claims the full-display background role; ordinary applications are rejected.
    pub fn connect() -> Result<Self, Error> {
        Ok(Self {
            window: Window::connect(WindowOptions::new("Desktop"), true)?,
            pending_event: None,
        })
    }
    /// Draws and submits the entire desktop; the compositor caches it before RELEASE.
    pub fn draw(&mut self, paint: impl FnOnce(&mut Canvas<'_>)) -> Result<(), Error> {
        self.window.draw(paint)
    }
    /// Receives unoccluded desktop pointer events in full-display coordinates.
    pub fn poll_event(&mut self) -> Result<Option<Event>, Error> {
        if self.pending_event.is_some() {
            return Ok(self.pending_event.take());
        }
        self.window.poll_event()
    }
    /// Requests vertical workspace reservations for panels.
    pub fn reserve_work_area(&mut self, top: u16, bottom: u16) -> Result<(), Error> {
        let area = desktop_protocol::WorkArea { top, bottom };
        if desktop_protocol::WorkArea::decode(area.encode(), self.window.config.height)
            != Some(area)
        {
            return Err(Error::InvalidGeometry);
        }
        self.request(area.encode(), desktop_protocol::WORK_REPLY)
    }
    /// Asks the compositor to manage/focus a spawned legacy client. Native clients
    /// still negotiate their own windows. Registration replies are consumed here.
    pub fn manage(&mut self, pid: u32, title: &str, large: bool) -> Result<(), Error> {
        if title.len() > TITLE_BYTES || !title.bytes().all(|byte| (32..127).contains(&byte)) {
            return Err(Error::InvalidTitle);
        }
        self.request(
            desktop_protocol::Managed { pid, large }
                .encode()
                .ok_or(Error::Rejected)?,
            pid,
        )?;
        let mut bytes = [0u8; TITLE_BYTES];
        bytes[..title.len()].copy_from_slice(title.as_bytes());
        for (index, chunk) in bytes.chunks_exact(3).enumerate() {
            send(
                desktop_protocol::MANAGED_TITLE
                    | ((index as u32) << 24)
                    | u32::from(chunk[0])
                    | (u32::from(chunk[1]) << 8)
                    | (u32::from(chunk[2]) << 16),
            )?;
        }
        Ok(())
    }
    fn request(&mut self, message: u32, tag: u32) -> Result<(), Error> {
        send(message)?;
        let start = sys::ticks();
        loop {
            if let Some((sender, message)) =
                sys::receive_message().map_err(|_| Error::Unavailable)?
            {
                if sender != 0 {
                    return Err(Error::Unavailable);
                }
                if message == (REJECT | tag) {
                    return Err(Error::Rejected);
                }
                if message == (ACCEPT | tag) {
                    break;
                }
                if message & KIND == RELEASE && self.window.pending == Some(message & SERIAL) {
                    self.window.pending = None;
                }
                if message & KIND == desktop_protocol::DESKTOP_POINTER {
                    let (x, y, buttons) = desktop_protocol::decode_pointer(message);
                    self.pending_event = Some(Event::Pointer { x, y, buttons });
                }
            }
            if sys::ticks().wrapping_sub(start) >= 36 {
                return Err(Error::Timeout);
            }
            sys::sleep_ticks(1).map_err(|_| Error::Unavailable)?;
        }
        Ok(())
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
        let response = self.button_control(label, label, bounds, Color::rgb(0xffffff));
        self.y += 50;
        response
    }
    fn button_control(
        &mut self,
        key: &str,
        label: &str,
        bounds: Rect,
        foreground: Color,
    ) -> Response {
        if bounds.width < 2 {
            return Response { clicked: false };
        }
        // Input is clipped too: a pointer outside the content must not activate
        // a component whose layout extends beyond the window.
        let left = bounds.x.max(0);
        let top = bounds.y.max(0);
        let right = (i64::from(bounds.x) + i64::from(bounds.width))
            .clamp(0, i64::from(self.canvas.width())) as i32;
        let bottom = (i64::from(bounds.y) + i64::from(bounds.height))
            .clamp(0, i64::from(self.canvas.height())) as i32;
        let hit_bounds = Rect::new(
            left,
            top,
            right.saturating_sub(left).max(0) as u32,
            bottom.saturating_sub(top).max(0) as u32,
        );
        let id = key.bytes().fold(2166136261u32, |hash, byte| {
            (hash ^ u32::from(byte)).wrapping_mul(16777619)
        });
        let clicked = self.input.button(id, hit_bounds);
        let hovered = bounds.contains(self.input.x, self.input.y);
        let pressed = self.input.down && self.input.active == Some(id);
        let color = if pressed {
            Color::rgba(130, 190, 255, 110)
        } else if hovered {
            Color::rgba(150, 210, 255, 76)
        } else {
            Color::rgba(145, 190, 235, 37)
        };
        self.canvas
            .rounded_rect(bounds, 11, Color::rgba(220, 237, 255, 30));
        self.canvas.rounded_rect(
            Rect::new(bounds.x + 1, bounds.y + 1, bounds.width - 2, 36),
            10,
            color,
        );
        text(
            self.canvas,
            bounds.x.saturating_add(14),
            bounds.y.saturating_add(if pressed { 16 } else { 15 }),
            label,
            foreground,
            1,
        );
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

struct ViewRenderer<'a, 'ui, 'buffer, M> {
    ui: &'a mut Ui<'ui, 'buffer>,
    action: Option<M>,
}
impl<M: Clone> declarative::Renderer<M> for ViewRenderer<'_, '_, '_, M> {
    fn text(&mut self, bounds: Rect, label: &str, color: Color, scale: u32) {
        text(self.ui.canvas, bounds.x, bounds.y, label, color, scale);
    }
    fn button(&mut self, bounds: Rect, key: &str, label: &str, action: &M, color: Option<Color>) {
        if self
            .ui
            .button_control(key, label, bounds, color.unwrap_or(Color::rgb(0xffffff)))
            .clicked()
            && self.action.is_none()
        {
            self.action = Some(action.clone());
        }
    }
    fn card(&mut self, bounds: Rect, title: &str, detail: &str, color: Option<Color>) {
        self.ui
            .canvas
            .rounded_rect(bounds, 14, Color::rgba(225, 240, 255, 20));
        text(
            self.ui.canvas,
            bounds.x.saturating_add(16),
            bounds.y.saturating_add(16),
            title,
            color.unwrap_or(Color::rgb(0xe7f2ff)),
            1,
        );
        text(
            self.ui.canvas,
            bounds.x.saturating_add(16),
            bounds.y.saturating_add(40),
            detail,
            color.unwrap_or(Color::rgb(0xb7cbe1)),
            1,
        );
    }
}
fn render_component<M: Clone>(
    ui: &mut Ui<'_, '_>,
    view: &impl View<M>,
) -> Result<Option<M>, Error> {
    let (width, height) = (ui.canvas.width(), ui.canvas.height());
    let mut renderer = ViewRenderer { ui, action: None };
    declarative::render(view, &mut renderer, width, height).map_err(Error::InvalidView)?;
    Ok(renderer.action)
}
