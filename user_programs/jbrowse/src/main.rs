// jbrowse is an ordinary Ring 3 application. The OS supplies mechanisms only.
extern crate alloc;
mod loader;
mod navigation;
use alloc::{
    format,
    string::{String, ToString},
};
use rustos_browser::{address, resolve, FieldKind, Page};
const HOME_PAGE: &str = "https://www.google.com/";
use userlib::{
    println,
    sync::{Arc, Mutex},
    sys, thread,
    ui::{self, text_input::TextInput, Color, Event, Material, Rect, Window, WindowOptions},
};

struct Shared {
    request: Option<(u64, String, userlib::io::Cancellation)>,
    active: Option<userlib::io::Cancellation>,
    result: Option<(u64, Result<loader::Loaded, String>)>,
}
struct Browser {
    address: TextInput,
    current: String,
    status: String,
    title: String,
    page: Page,
    scroll: u32,
    editing: bool,
    loading: bool,
    generation: u64,
    pressed: Option<(i32, i32)>,
    active_field: Option<(usize, usize)>,
    field_editor: TextInput,
}
impl Browser {
    fn new() -> Self {
        Self {
            address: TextInput::new(HOME_PAGE),
            current: String::new(),
            status: "Loading start page...".into(),
            title: "New page".into(),
            page: rustos_browser::Document::parse("<p>Loading the start page...</p>")
                .expect("built-in page")
                .layout(640),
            scroll: 0,
            editing: false,
            loading: false,
            generation: 0,
            pressed: None,
            active_field: None,
            field_editor: TextInput::default(),
        }
    }
    fn navigate(&mut self, shared: &Mutex<Shared>, input: &str) {
        match address(input) {
            Ok(url) => {
                self.generation = self.generation.wrapping_add(1);
                self.address.set(&url);
                self.editing = false;
                self.active_field = None;
                self.status = "Loading...".into();
                self.loading = true;
                // One worker, one replaceable request and one result. No app lock
                // is held across network I/O or canvas publication.
                let cancel = userlib::io::Cancellation::new();
                let mut shared = shared.lock();
                if let Some(previous) = shared.active.replace(cancel.clone()) {
                    previous.cancel();
                }
                shared.request = Some((self.generation, url, cancel));
            }
            Err(_) => {
                self.status = "Invalid address; use HTTP or HTTPS".into();
                println!("[jbrowse] error: invalid address");
            }
        }
    }
    fn submit_form(&mut self, shared: &Mutex<Shared>, form: usize, submitter: Option<usize>) {
        match self.page.forms[form].submit(&self.current, submitter) {
            Ok(url) => self.navigate(shared, &url),
            Err(_) => self.status = "Unsupported form or search URL too long".into(),
        }
    }
    fn scroll_by(&mut self, delta: i32) {
        let max = self.page.height.saturating_sub(308);
        self.scroll = (self.scroll as i64 + delta as i64).clamp(0, max as i64) as u32;
    }
    fn click(&mut self, shared: &Mutex<Shared>, x: i32, y: i32) {
        if Rect::new(8, 30, 516, 26).contains(x, y) {
            self.editing = true;
            self.active_field = None;
            self.address.select_all();
            return;
        }
        self.editing = false;
        self.active_field = None;
        if (64..372).contains(&y) {
            if let Some(control) = self
                .page
                .controls
                .iter()
                .rev()
                .find(|control| control.contains(x, y - 64 + self.scroll as i32))
            {
                let (form, field) = (control.form, control.field);
                let data = &self.page.forms[form].fields[field];
                if data.disabled {
                    return;
                }
                match data.kind {
                    FieldKind::Text if !data.readonly => {
                        self.field_editor.set(&data.value);
                        self.field_editor.select_all();
                        self.active_field = Some((form, field));
                    }
                    FieldKind::Submit => self.submit_form(shared, form, Some(field)),
                    _ => {}
                }
                return;
            }
        }
        if Rect::new(532, 30, 48, 26).contains(x, y) {
            let url = self.address.as_str().to_string();
            self.navigate(shared, &url);
        } else if Rect::new(588, 30, 44, 26).contains(x, y) && !self.current.is_empty() {
            let url = self.current.clone();
            self.navigate(shared, &url);
        } else if Rect::new(532, 376, 48, 20).contains(x, y) {
            self.scroll_by(-240);
        } else if Rect::new(588, 376, 44, 20).contains(x, y) {
            self.scroll_by(240);
        } else if (64..372).contains(&y) {
            if let Some(link) = self.page.hit(x, y - 64 + self.scroll as i32) {
                match resolve(&self.current, link) {
                    Ok(url) => self.navigate(shared, &url),
                    Err(_) => {
                        self.status = "Unsupported link".into();
                    }
                }
            }
        }
    }
    fn paint(&self, canvas: &mut ui::Canvas<'_>) {
        canvas.clear(Color::rgb(0x15243a));
        canvas.rounded_rect(Rect::new(8, 4, 180, 22), 6, Color::rgb(0x263d59));
        ui::text(canvas, 18, 11, "jbrowse  /  1 tab", Color::rgb(0xd9e8f8), 1);
        canvas.rounded_rect(
            Rect::new(8, 30, 516, 26),
            6,
            Color::rgb(if self.editing { 0xffffff } else { 0xe5edf5 }),
        );
        if let Some(mut entry) = canvas.viewport(Rect::new(16, 37, 500, 12)) {
            let max = 61;
            let text = self.address.as_str();
            let start = text.len().saturating_sub(max);
            let start = (start..=text.len())
                .find(|at| text.is_char_boundary(*at))
                .unwrap_or(0);
            if self.editing && self.address.is_selected() {
                entry.clear(Color::rgb(0xc2d8f5));
            }
            ui::text(&mut entry, 0, 1, &text[start..], Color::rgb(0x172b43), 1);
            if self.editing && !self.address.is_selected() {
                let cursor = text[start..self.address.cursor().max(start)]
                    .chars()
                    .count()
                    .min(max) as i32
                    * 8;
                entry.fill_rect(Rect::new(cursor, 0, 1, 10), Color::rgb(0x172b43));
            }
        }
        for (x, y, w, h, label) in [
            (532, 30, 48, 26, "Go"),
            (588, 30, 44, 26, "R"),
            (532, 376, 48, 20, "Up"),
            (588, 376, 44, 20, "Down"),
        ] {
            canvas.rounded_rect(Rect::new(x, y, w, h), 5, Color::rgb(0x314c6d));
            ui::text(canvas, x + 7, y + 7, label, Color::rgb(0xffffff), 1);
        }
        if let Some(mut viewport) = canvas.viewport(Rect::new(0, 64, 640, 308)) {
            viewport.clear(Color::rgb(self.page.background));
            for item in &self.page.items {
                let y = item.y - self.scroll as i32;
                if y + item.height as i32 <= 0 || y >= 308 {
                    continue;
                }
                if let Some(color) = item.background {
                    viewport.fill_rect(
                        Rect::new(item.x, y, item.width, item.height),
                        Color::rgb(color),
                    );
                }
                ui::text(
                    &mut viewport,
                    item.x,
                    y,
                    &item.text,
                    Color::rgb(item.color),
                    item.scale,
                );
                if item.link.is_some() {
                    viewport.fill_rect(
                        Rect::new(item.x, y + (8 * item.scale) as i32, item.width, 1),
                        Color::rgb(item.color),
                    );
                }
            }
            for control in &self.page.controls {
                let y = control.y - self.scroll as i32;
                if y + control.height as i32 <= 0 || y >= 308 {
                    continue;
                }
                let field = &self.page.forms[control.form].fields[control.field];
                let active = self.active_field == Some((control.form, control.field));
                let border = if active { 0x397ac0 } else { 0x7e9bb9 };
                viewport.fill_rect(
                    Rect::new(control.x, y, control.width, control.height),
                    Color::rgb(border),
                );
                let fill = if field.kind == FieldKind::Text {
                    0xf7fbff
                } else {
                    0xe3edf9
                };
                viewport.fill_rect(
                    Rect::new(
                        control.x + 1,
                        y + 1,
                        control.width.saturating_sub(2),
                        control.height.saturating_sub(2),
                    ),
                    Color::rgb(fill),
                );
                let left = control.x + 6;
                let width = control.width.saturating_sub(12);
                // Keep the complete control visible area inside the page viewport.
                if y >= 0 && y + control.height as i32 <= 308 && width > 0 {
                    if let Some(mut input) = viewport.viewport(Rect::new(left, y + 7, width, 12)) {
                        let label = if field.value.is_empty() {
                            &field.label
                        } else {
                            &field.value
                        };
                        if active && self.field_editor.is_selected() {
                            input.clear(Color::rgb(0xc2d8f5));
                        }
                        ui::text(
                            &mut input,
                            0,
                            1,
                            label,
                            Color::rgb(if field.disabled { 0x8a8a8a } else { 0x172b43 }),
                            1,
                        );
                        if active && !self.field_editor.is_selected() {
                            let cursor = (self.field_editor.as_str()[..self.field_editor.cursor()]
                                .chars()
                                .count()
                                .min((width / 8) as usize)
                                * 8) as i32;
                            input.fill_rect(Rect::new(cursor, 0, 1, 10), Color::rgb(0x172b43));
                        }
                    }
                }
            }
        }
        if let Some(mut status) = canvas.viewport(Rect::new(8, 380, 516, 14)) {
            ui::text(&mut status, 0, 1, &self.status, Color::rgb(0xc6d9ed), 1);
        }
    }
}

fn main(_argc: usize, argv: &[&str]) {
    let mut window = Window::create(
        WindowOptions::new("jbrowse")
            .content_size(640, 400)
            .background(Material::Opaque),
    )
    .expect("browser window");
    let shared = Arc::new(Mutex::new(Shared {
        request: None,
        result: None,
        active: None,
    }));
    let transport = Arc::clone(&shared);
    let _worker = match thread::Builder::new().spawn(move || loop {
        let request = transport.lock().request.take();
        if let Some((generation, url, cancel)) = request {
            let loaded = loader::load(&url, &cancel);
            transport.lock().result = Some((generation, loaded));
        } else {
            let _ = sys::sleep_ticks(1);
        }
    }) {
        Ok(worker) => worker,
        Err(_) => {
            println!("[jbrowse] network worker unavailable");
            return;
        }
    };
    let mut browser = Browser::new();
    browser.navigate(&shared, argv.get(1).copied().unwrap_or(HOME_PAGE));
    window
        .draw(|canvas| browser.paint(canvas))
        .expect("browser first frame");
    println!("[jbrowse] ready");
    loop {
        let mut dirty = false;
        for _ in 0..32 {
            match window.poll_event().expect("browser events") {
                Some(Event::CloseRequested) => {
                    if let Some(active) = shared.lock().active.take() {
                        active.cancel();
                    }
                    return;
                }
                Some(Event::Focus(false)) => {
                    browser.pressed = None;
                }
                Some(Event::Pointer { x, y, buttons }) => {
                    if buttons & 1 != 0 {
                        if browser.pressed.is_none() {
                            browser.pressed = Some((x, y));
                        }
                    } else if let Some((px, py)) = browser.pressed.take() {
                        if (x - px).abs() < 6 && (y - py).abs() < 6 {
                            browser.click(&shared, x, y);
                            dirty = true;
                        }
                    }
                }
                _ => {}
            }
        }
        let mut bytes = [0u8; 64];
        let n = sys::read_nb(sys::STDIN_FD, &mut bytes);
        for &byte in &bytes[..n] {
            if byte == b'\n' || byte == b'\r' {
                if let Some((form, _)) = browser.active_field {
                    browser.submit_form(&shared, form, None);
                } else {
                    let url = browser.address.as_str().to_string();
                    browser.navigate(&shared, &url);
                }
                dirty = true;
            } else if byte == 27 {
                browser.editing = true;
                browser.active_field = None;
                browser.address.select_all();
                dirty = true;
            } else if let Some((form, field)) = browser.active_field {
                if browser.field_editor.byte(byte) {
                    browser.page.forms[form].fields[field].value =
                        browser.field_editor.as_str().to_string();
                    dirty = true;
                }
            } else if browser.editing {
                dirty |= browser.address.byte(byte);
            } else {
                match byte {
                    b'j' | b' ' => browser.scroll_by(240),
                    b'k' => browser.scroll_by(-240),
                    _ => continue,
                }
                dirty = true;
            }
        }
        let result = shared.lock().result.take();
        if let Some((generation, result)) = result {
            if generation == browser.generation {
                browser.loading = false;
                match result {
                    Ok(loaded) => {
                        browser.current = loaded.url;
                        navigation::commit_address(
                            &mut browser.address,
                            browser.editing,
                            &browser.current,
                        );
                        browser.title = loaded.title;
                        browser.page = loaded.page;
                        browser.scroll = 0;
                        browser.status = format!(
                            "HTTP {}  |  {}{}",
                            loaded.status,
                            browser.title,
                            if browser.page.truncated {
                                " (layout limit)"
                            } else {
                                ""
                            }
                        );
                        if loaded.script_errors > 0 {
                            browser
                                .status
                                .push_str(&format!(" | {} script errors", loaded.script_errors));
                        }
                        println!("[jbrowse] loaded: {}", browser.title);
                    }
                    Err(message) => {
                        println!("[jbrowse] error: {}", message);
                        browser.status = message;
                    }
                }
                dirty = true;
            }
        }
        if dirty {
            window
                .draw(|canvas| browser.paint(canvas))
                .expect("browser frame");
        }
        sys::wait_events(sys::Events::KEYBOARD | sys::Events::MAILBOX, 1).expect("browser idle");
    }
}
