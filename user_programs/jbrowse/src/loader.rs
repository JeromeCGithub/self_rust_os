//! Network/resource policy belongs to the application, never the kernel.
use alloc::{
    string::{String, ToString},
    vec::Vec,
};
use rustos_browser::{decode_html, resolve, Document, Encoding, Page, MAX_HTML};
use userlib::http;

pub struct Loaded {
    pub url: String,
    pub title: String,
    pub page: Page,
    pub status: u16,
    pub script_errors: usize,
}
fn fetch(
    url: &str,
    limit: usize,
    cancel: &userlib::io::Cancellation,
) -> Result<(String, Vec<u8>, http::Response), String> {
    let mut bytes = Vec::new();
    let mut effective = String::new();
    let options = http::Options {
        max_body: limit,
        ..http::Options::default()
    };
    let response = http::get_cancellable(
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
        cancel.clone(),
    )
    .map_err(|e| http::error_message(e).to_string())?;
    Ok((effective, bytes, response))
}
pub fn load(url: &str, cancel: &userlib::io::Cancellation) -> Result<Loaded, String> {
    let (url, bytes, response) = fetch(url, MAX_HTML, cancel)?;
    let encoding = match response.charset {
        Some(http::Charset::Utf8) => Some(Encoding::Utf8),
        Some(http::Charset::Windows1252) => Some(Encoding::Windows1252),
        None => None,
        Some(http::Charset::Other) => return Err("Unsupported page character encoding".to_string()),
    };
    let html =
        decode_html(&bytes, encoding).map_err(|_| "Page character decoding failed".to_string())?;
    let mut doc = Document::parse(&html).map_err(|_| "HTML resource limit exceeded".to_string())?;
    // Inline CSS has already been parsed; linked sheets follow in link order.
    // Unsupported/failed optional sheets leave a readable HTML fallback.
    for sheet in doc.stylesheets.clone() {
        if let Ok(sheet) = resolve(&url, &sheet) {
            if let Ok((_, bytes, response)) = fetch(&sheet, 32 * 1024, cancel) {
                if response.status != 200 {
                    continue;
                }
                if let Ok(css) = core::str::from_utf8(&bytes) {
                    let _ = doc.add_css(css);
                }
            }
        }
    }
    if cancel.is_cancelled() {
        return Err("request cancelled".to_string());
    }
    let scripts = doc.run_scripts();
    let script_errors = scripts.failures.len();
    if script_errors > 0 {
        userlib::println!("[jbrowse] script errors: {}", script_errors);
    }
    let title = if doc.title.is_empty() {
        url.clone()
    } else {
        doc.title.clone()
    };
    let mut page = doc.layout(640);
    if script_errors > 0 && page.is_empty() {
        userlib::println!("[jbrowse] unsupported script page");
        page=Document::parse("<h1>Page needs more JavaScript support</h1><p>The request succeeded, but this browser cannot display this page yet.</p><p>The current scripting prototype does not run full web applications.</p><p>You can edit the address to visit another page.</p>").map_err(|_|"Could not create error page".to_string())?.layout(640);
    }
    Ok(Loaded {
        url,
        title,
        page,
        status: response.status,
        script_errors,
    })
}
