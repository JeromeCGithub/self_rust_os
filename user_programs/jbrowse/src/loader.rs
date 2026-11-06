//! Network/resource policy belongs to the application, never the kernel.
use alloc::{
    string::{String, ToString},
    vec::Vec,
};
use rustos_browser::{resolve, Document, Page, MAX_HTML};
use userlib::http;

pub struct Loaded {
    pub url: String,
    pub title: String,
    pub page: Page,
    pub status: u16,
}
fn fetch(url: &str, limit: usize) -> Result<(String, Vec<u8>, u16), String> {
    let mut bytes = Vec::new();
    let mut effective = String::new();
    let options = http::Options {
        max_body: limit,
        ..http::Options::default()
    };
    let response = http::get_with_url(
        url,
        options,
        |part| {
            bytes
                .try_reserve(part.len())
                .map_err(|_| http::Error::TooLarge)?;
            bytes.extend_from_slice(part);
            Ok(())
        },
        |url| effective = url.to_string(),
    )
    .map_err(|e| http::error_message(e).to_string())?;
    Ok((effective, bytes, response.status))
}
pub fn load(url: &str) -> Result<Loaded, String> {
    let (url, bytes, status) = fetch(url, MAX_HTML)?;
    let html = core::str::from_utf8(&bytes).map_err(|_| "Page is not UTF-8".to_string())?;
    let mut doc = Document::parse(html).map_err(|_| "HTML resource limit exceeded".to_string())?;
    // Inline CSS has already been parsed; linked sheets follow in link order.
    // Unsupported/failed optional sheets leave a readable HTML fallback.
    for sheet in doc.stylesheets.clone() {
        if let Ok(sheet) = resolve(&url, &sheet) {
            if let Ok((_, bytes, 200)) = fetch(&sheet, 32 * 1024) {
                if let Ok(css) = core::str::from_utf8(&bytes) {
                    let _ = doc.add_css(css);
                }
            }
        }
    }
    let title = if doc.title.is_empty() {
        url.clone()
    } else {
        doc.title.clone()
    };
    Ok(Loaded {
        url,
        title,
        page: doc.layout(640),
        status,
    })
}
