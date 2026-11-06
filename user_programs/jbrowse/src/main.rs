// jbrowse is an ordinary Ring 3 application. The OS supplies mechanisms only.
extern crate alloc;
mod loader;
mod navigation;
use alloc::{
    format,
    string::{String, ToString},
};
use rustos_browser::{address, resolve, FieldKind, Page, Submission};
const HOME_PAGE: &str = "https://www.google.com/";
use userlib::{
    println,
    sync::{Arc, Mutex},
    sys, thread,
    ui::{self, text_input::TextInput, Color, Event, Material, Rect, Window, WindowOptions},
};

struct Shared {
    request: Option<(u64, Submission, userlib::io::Cancellation)>,
    active: Option<userlib::io::Cancellation>,
    result: Option<(u64, Result<loader::Loaded, String>)>,
}
struct Browser {
    document: rustos_browser::Document,
    fonts: Arc<ui::font::FontSet>,
    size: (u32, u32),
    font_painter: ui::font::Painter,
    address: TextInput,
    current: String,
    history: navigation::History,
    history_mode: navigation::Mode,
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
    fn new(fonts: Arc<ui::font::FontSet>) -> Self {
        let document = rustos_browser::Document::parse("<p>Loading the start page...</p>")
            .expect("built-in page");
        let initial = document.layout_with(640, fonts.as_ref());
        Self {
            document,
            fonts: Arc::clone(&fonts),
            size: (640, 400),
            font_painter: ui::font::Painter::new(fonts),
            address: TextInput::new(HOME_PAGE),
            current: String::new(),
            history: navigation::History::default(),
            history_mode: navigation::Mode::Push,
            status: "Loading start page...".into(),
            title: "New page".into(),
            page: initial,
            scroll: 0,
            editing: false,
            loading: false,
            generation: 0,
            pressed: None,
            active_field: None,
            field_editor: TextInput::default(),
        }
    }
    fn reflow(&mut self, width: u32, height: u32) {
        if self.size == (width, height) {
            return;
        }
        for (form, saved) in self.document.forms.iter_mut().zip(&self.page.forms) {
            for (field, edited) in form.fields.iter_mut().zip(&saved.fields) {
                field.value = edited.value.clone();
            }
        }
        self.page = self.document.layout_with(width, self.fonts.as_ref());
        self.size = (width, height);
        self.pressed = None;
        self.scroll_by(0);
    }
    fn view_height(&self) -> u32 {
        self.size.1.saturating_sub(92)
    }
    fn navigate(&mut self, shared: &Mutex<Shared>, input: &str) {
        match address(input) {
            Ok(url) => {
                self.begin_navigation(shared, Submission::Get(url));
            }
            Err(_) => {
                self.status = "Invalid address; use HTTP or HTTPS".into();
                println!("[jbrowse] error: invalid address");
            }
        }
    }
    fn begin_navigation(&mut self, shared: &Mutex<Shared>, submission: Submission) {
        self.history_mode = navigation::Mode::Push;
        self.generation = self.generation.wrapping_add(1);
        self.address.set(submission.url());
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
        shared.request = Some((self.generation, submission, cancel));
    }
    fn submit_form(&mut self, shared: &Mutex<Shared>, form: usize, submitter: Option<usize>) {
        match self.page.forms[form].submission(&self.current, submitter) {
            Ok(submission) => {
                self.begin_navigation(shared, submission);
                self.field_editor.set("");
                // Do not keep submitted password values in the displayed page
                // or its reflow copy. The queued request alone owns the body.
                for fields in [
                    &mut self.page.forms[form].fields,
                    &mut self.document.forms[form].fields,
                ] {
                    for field in fields {
                        if field.kind == FieldKind::Password {
                            field.value.clear();
                        }
                    }
                }
            }
            Err(_) => self.status = "Unsupported form or search URL too long".into(),
        }
    }
    fn scroll_by(&mut self, delta: i32) {
        let max = self.page.height.saturating_sub(self.view_height());
        self.scroll = (self.scroll as i64 + delta as i64).clamp(0, max as i64) as u32;
    }
    fn click(&mut self, shared: &Mutex<Shared>, x: i32, y: i32) {
        let (w, h) = (self.size.0 as i32, self.size.1 as i32);
        if Rect::new(72, 30, self.size.0.saturating_sub(188), 26).contains(x, y) {
            self.editing = true;
            self.active_field = None;
            self.address.select_all();
            return;
        }
        self.editing = false;
        self.active_field = None;
        if (64..h - 28).contains(&y) {
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
                    kind if kind.is_text() && !data.readonly => {
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
        if Rect::new(8, 30, 26, 26).contains(x, y) {
            if let Some(url) = self.history.back().map(str::to_string) {
                self.navigate(shared, &url);
                self.history_mode = navigation::Mode::Back;
            }
        } else if Rect::new(40, 30, 26, 26).contains(x, y) {
            if let Some(url) = self.history.forward().map(str::to_string) {
                self.navigate(shared, &url);
                self.history_mode = navigation::Mode::Forward;
            }
        } else if Rect::new(w - 108, 30, 48, 26).contains(x, y) {
            let url = self.address.as_str().to_string();
            self.navigate(shared, &url);
        } else if Rect::new(w - 52, 30, 44, 26).contains(x, y)
            && (!self.current.is_empty() || self.loading)
        {
            if self.loading {
                let mut shared = shared.lock();
                if let Some(active) = shared.active.take() {
                    active.cancel();
                }
                shared.request = None;
                self.generation = self.generation.wrapping_add(1);
                self.loading = false;
                self.status = "Stopped".into();
                self.address.set(&self.current);
                println!("[jbrowse] stopped");
            } else {
                let url = self.current.clone();
                self.navigate(shared, &url);
                self.history_mode = navigation::Mode::Reload;
            }
        } else if Rect::new(w - 108, h - 24, 48, 20).contains(x, y) {
            self.scroll_by(-240);
        } else if Rect::new(w - 52, h - 24, 44, 20).contains(x, y) {
            self.scroll_by(240);
        } else if (64..h - 28).contains(&y) {
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
    fn paint(&mut self, canvas: &mut ui::Canvas<'_>) {
        self.reflow(canvas.width(), canvas.height());
        let (w, h) = (self.size.0 as i32, self.size.1 as i32);
        let view_height = self.view_height();
        canvas.clear(Color::rgb(0x15243a));
        canvas.rounded_rect(Rect::new(8, 4, 180, 22), 6, Color::rgb(0x263d59));
        ui::text(canvas, 18, 11, "jbrowse  /  1 tab", Color::rgb(0xd9e8f8), 1);
        canvas.rounded_rect(
            Rect::new(72, 30, self.size.0.saturating_sub(188), 26),
            6,
            Color::rgb(if self.editing { 0xffffff } else { 0xe5edf5 }),
        );
        if let Some(mut entry) =
            canvas.viewport(Rect::new(80, 37, self.size.0.saturating_sub(204), 12))
        {
            let max = (entry.width() as usize / 8).saturating_sub(1);
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
            (8, 30, 26, 26, "<"),
            (40, 30, 26, 26, ">"),
            (w - 108, 30, 48, 26, "Go"),
            (w - 52, 30, 44, 26, if self.loading { "Stop" } else { "R" }),
            (w - 108, h - 24, 48, 20, "Up"),
            (w - 52, h - 24, 44, 20, "Down"),
        ] {
            canvas.rounded_rect(Rect::new(x, y, w, h), 5, Color::rgb(0x314c6d));
            ui::text(canvas, x + 7, y + 7, label, Color::rgb(0xffffff), 1);
        }
        if let Some(mut viewport) = canvas.viewport(Rect::new(0, 64, self.size.0, view_height)) {
            viewport.clear(Color::rgb(self.page.background));
            for item in &self.page.items {
                let y = item.y - self.scroll as i32;
                if y + item.height as i32 <= 0 || y >= view_height as i32 {
                    continue;
                }
                if let Some(color) = item.background {
                    viewport.fill_rect(
                        Rect::new(item.x, y, item.width, item.height),
                        Color::rgb(color),
                    );
                }
                if let Some(image) = item.image.and_then(|index| self.page.images.get(index)) {
                    let _ = viewport.rgba_image(
                        Rect::new(item.x, y, item.width, item.height),
                        image.image.width,
                        image.image.height,
                        &image.image.rgba,
                    );
                }
                let left = item.x.max(0);
                let top = y.max(0);
                let right = (i64::from(item.x) + i64::from(item.width))
                    .min(i64::from(self.size.0))
                    .max(0) as i32;
                let bottom = (i64::from(y) + i64::from(item.height))
                    .min(i64::from(view_height))
                    .max(0) as i32;
                if right > left && bottom > top {
                    if let Some(mut fragment) = viewport.viewport(Rect::new(
                        left,
                        top,
                        (right - left) as u32,
                        (bottom - top) as u32,
                    )) {
                        let _ = self.font_painter.draw(
                            &mut fragment,
                            item.x - left,
                            y - top,
                            &item.text,
                            item.font,
                            Color::rgb(item.color),
                        );
                    }
                }
                if item.link.is_some() && item.image.is_none() {
                    viewport.fill_rect(
                        Rect::new(
                            item.x,
                            y + item.height.saturating_sub(2) as i32,
                            item.width,
                            1,
                        ),
                        Color::rgb(item.color),
                    );
                }
            }
            for control in &self.page.controls {
                let y = control.y - self.scroll as i32;
                if y + control.height as i32 <= 0 || y >= view_height as i32 {
                    continue;
                }
                let field = &self.page.forms[control.form].fields[control.field];
                let active = self.active_field == Some((control.form, control.field));
                let border = if active { 0x397ac0 } else { 0x7e9bb9 };
                viewport.fill_rect(
                    Rect::new(control.x, y, control.width, control.height),
                    Color::rgb(border),
                );
                let fill = if field.kind.is_text() {
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
                if y >= 0 && y + control.height as i32 <= view_height as i32 && width > 0 {
                    if let Some(mut input) = viewport.viewport(Rect::new(left, y + 7, width, 12)) {
                        let label = field.display_value();
                        if active && self.field_editor.is_selected() {
                            input.clear(Color::rgb(0xc2d8f5));
                        }
                        ui::text(
                            &mut input,
                            0,
                            1,
                            &label,
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
        if let Some(mut status) =
            canvas.viewport(Rect::new(8, h - 20, self.size.0.saturating_sub(124), 14))
        {
            ui::text(&mut status, 0, 1, &self.status, Color::rgb(0xc6d9ed), 1);
        }
    }
}

fn main(_argc: usize, argv: &[&str]) {
    let fonts = match ui::font::FontSet::load(
        include_bytes!("../../../third_party/fonts/DejaVuSans.ttf"),
        include_bytes!("../../../third_party/fonts/DejaVuSans-Bold.ttf"),
        include_bytes!("../../../third_party/fonts/DejaVuSerif.ttf"),
        include_bytes!("../../../third_party/fonts/DejaVuSansMono.ttf"),
    )
    .and_then(|fonts| {
        fonts.with_bold_faces(
            include_bytes!("../../../third_party/fonts/DejaVuSerif-Bold.ttf"),
            include_bytes!("../../../third_party/fonts/DejaVuSansMono-Bold.ttf"),
        )
    }) {
        Ok(fonts) => Arc::new(fonts),
        Err(_) => {
            println!("[jbrowse] font assets unavailable");
            return;
        }
    };
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
    let layout_fonts = Arc::clone(&fonts);
    let _worker = match thread::Builder::new().spawn(move || loop {
        let request = transport.lock().request.take();
        if let Some((generation, url, cancel)) = request {
            let loaded = loader::load(&url, &cancel, layout_fonts.as_ref());
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
    let mut browser = Browser::new(fonts);
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
                Some(Event::Resized { width, height }) => {
                    browser.reflow(width, height);
                    dirty = true;
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
                        browser
                            .history
                            .commit(&browser.current, &loaded.url, browser.history_mode);
                        browser.current = loaded.url;
                        navigation::commit_address(
                            &mut browser.address,
                            browser.editing,
                            &browser.current,
                        );
                        browser.title = loaded.title;
                        browser.page = loaded.page;
                        browser.document = loaded.document;
                        let (width, height) = (window.width(), window.height());
                        browser.size = (0, 0);
                        browser.reflow(width, height);
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
                        if loaded.image_errors > 0 {
                            browser.status.push_str(&format!(
                                " | {} images unavailable",
                                loaded.image_errors
                            ));
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
