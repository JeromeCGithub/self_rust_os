// jbrowse is an ordinary Ring 3 application. The OS supplies mechanisms only.
extern crate alloc;
mod loader;
use alloc::{
    format,
    string::{String, ToString},
};
use rustos_browser::{address, resolve, Page};
use userlib::{
    println,
    sync::{Arc, Mutex},
    sys, thread,
    ui::{self, text_input::TextInput, Color, Event, Material, Rect, Window, WindowOptions},
};

struct Shared {
    request: Option<(u64, String)>,
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
}
impl Browser {
    fn new() -> Self {
        Self { address:TextInput::new("http://info.cern.ch/hypertext/WWW/TheProject.html"),current:String::new(),status:"Click the address, type a URL or search, then Enter".into(),title:"New page".into(),page:rustos_browser::Document::parse("<h1>jbrowse</h1><p>A small window onto the web.</p><p>Enter an HTTP or HTTPS URL above. Words open web search.</p><p>Click links to navigate. Use Up / Down or j / k to scroll.</p>").expect("built-in page").layout(640),scroll:0,editing:true,loading:false,generation:0,pressed:None }
    }
    fn navigate(&mut self, shared: &Mutex<Shared>, input: &str) {
        match address(input) {
            Ok(url) => {
                self.generation = self.generation.wrapping_add(1);
                self.address.set(&url);
                self.editing = false;
                self.status = "Loading...".into();
                self.loading = true;
                // One worker, one replaceable request and one result. No app lock
                // is held across network I/O or canvas publication.
                shared.lock().request = Some((self.generation, url));
            }
            Err(_) => {
                self.status = "Invalid address; use HTTP or HTTPS".into();
                println!("[jbrowse] error: invalid address");
            }
        }
    }
    fn scroll_by(&mut self, delta: i32) {
        let max = self.page.height.saturating_sub(308);
        self.scroll = (self.scroll as i64 + delta as i64).clamp(0, max as i64) as u32;
    }
    fn click(&mut self, shared: &Mutex<Shared>, x: i32, y: i32) {
        if Rect::new(8, 30, 516, 26).contains(x, y) {
            self.editing = true;
            self.address.select_all();
            return;
        }
        self.editing = false;
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
    }));
    let transport = Arc::clone(&shared);
    let _worker = match thread::Builder::new().spawn(move || loop {
        let request = transport.lock().request.take();
        if let Some((generation, url)) = request {
            let loaded = loader::load(&url);
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
    if let Some(url) = argv.get(1) {
        browser.navigate(&shared, url);
    }
    window
        .draw(|canvas| browser.paint(canvas))
        .expect("browser first frame");
    println!("[jbrowse] ready");
    loop {
        let mut dirty = false;
        for _ in 0..32 {
            match window.poll_event().expect("browser events") {
                Some(Event::CloseRequested) => return,
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
                let url = browser.address.as_str().to_string();
                browser.navigate(&shared, &url);
                dirty = true;
            } else if byte == 27 {
                browser.editing = true;
                browser.address.select_all();
                dirty = true;
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
                        browser.address.set(&browser.current);
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
