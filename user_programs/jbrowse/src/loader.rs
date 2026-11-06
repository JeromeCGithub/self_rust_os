//! Network/resource policy belongs to the application, never the kernel.
use alloc::{
    string::{String, ToString},
    vec::Vec,
};
use rustos_browser::{decode_html, resolve, Document, Encoding, Page, Submission, MAX_HTML};
use userlib::http;

pub struct Loaded {
    pub url: String,
    pub title: String,
    pub page: Page,
    pub document: Document,
    pub status: u16,
    pub script_errors: usize,
    pub image_errors: usize,
}
fn fetch(
    url: &str,
    limit: usize,
    cancel: &userlib::io::Cancellation,
) -> Result<(String, Vec<u8>, http::Response), String> {
    fetch_request(url, http::Request::Get, limit, cancel)
}
fn fetch_request(
    url: &str,
    request: http::Request<'_>,
    limit: usize,
    cancel: &userlib::io::Cancellation,
) -> Result<(String, Vec<u8>, http::Response), String> {
    let mut bytes = Vec::new();
    let mut effective = String::new();
    let options = http::Options {
        max_body: limit,
        ..http::Options::default()
    };
    let response = http::request_cancellable(
        url,
        request,
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
pub fn load(
    submission: &Submission,
    cancel: &userlib::io::Cancellation,
    fonts: &userlib::ui::font::FontSet,
) -> Result<Loaded, String> {
    let request = match submission {
        Submission::Get(_) => http::Request::Get,
        Submission::Post { body, .. } => http::Request::FormPost(body.as_bytes()),
    };
    let (url, bytes, response) = fetch_request(submission.url(), request, MAX_HTML, cancel)?;
    let encoding = match response.charset {
        Some(http::Charset::Utf8) => Some(Encoding::Utf8),
        Some(http::Charset::Windows1252) => Some(Encoding::Windows1252),
        None => None,
        Some(http::Charset::Other) => return Err("Unsupported page character encoding".to_string()),
    };
    let html =
        decode_html(&bytes, encoding).map_err(|_| "Page character decoding failed".to_string())?;
    let mut doc = Document::parse(&html).map_err(|_| "HTML resource limit exceeded".to_string())?;
    // Source-order author sheets; resource transport policy stays in the app.
    let _style_errors = doc.apply_stylesheets(|source| {
        let source = resolve(&url, source).ok()?;
        let (_, bytes, response) = fetch(&source, 32 * 1024, cancel).ok()?;
        if response.status != 200 {
            return None;
        }
        core::str::from_utf8(&bytes).ok().map(str::to_string)
    });
    if cancel.is_cancelled() {
        return Err("request cancelled".to_string());
    }
    // Page scripts execute only in a restricted child. Reply data is validated
    // and committed atomically; no JS engine or native binding lives in this app.
    let script_errors = if doc.nodes.iter().any(|node| node.tag == "script") {
        let result = rustos_browser_worker::native::Native::start(|| cancel.is_cancelled())
            .and_then(|mut worker| worker.render_document(&html, || cancel.is_cancelled()));
        match result {
            Ok(bytes) => match rustos_browser::dom::Changes::decode(&bytes) {
                Ok(changes) if doc.apply_changes(&changes).is_ok() => changes.failures as usize,
                _ => {
                    userlib::println!("[jbrowse] native DOM reply rejected");
                    1
                }
            },
            Err(error) => {
                userlib::println!("[jbrowse] native worker failure: {:?}", error);
                1
            }
        }
    } else {
        0
    };
    if cancel.is_cancelled() {
        return Err("request cancelled".to_string());
    }
    if script_errors > 0 {
        userlib::println!("[jbrowse] script errors: {}", script_errors);
    }
    let mut image_errors = 0;
    let requests: Vec<_> = doc
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| node.tag == "img")
        .filter_map(|(id, node)| node.attr("src").map(|src| (id, src.to_string())))
        .take(16)
        .collect();
    for (node, source) in requests {
        if cancel.is_cancelled() {
            return Err("request cancelled".to_string());
        }
        let image = resolve(&url, &source)
            .ok()
            .and_then(|source| fetch(&source, 1024 * 1024, cancel).ok())
            .filter(|(_, _, response)| response.status == 200)
            .and_then(|(_, bytes, _)| rustos_browser::Image::decode(&bytes).ok());
        if image
            .and_then(|image| doc.set_image(node, alloc::sync::Arc::new(image)).ok())
            .is_none()
        {
            image_errors += 1;
        }
    }
    let title = if doc.title.is_empty() {
        url.clone()
    } else {
        doc.title.clone()
    };
    let mut page = doc.layout_with(640, fonts);
    if script_errors > 0 && page.is_empty() {
        userlib::println!("[jbrowse] unsupported script page");
        doc=Document::parse("<h1>Page needs more JavaScript support</h1><p>The request succeeded, but this browser cannot display this page yet.</p><p>The current native DOM bindings do not run full web applications.</p><p>You can edit the address to visit another page.</p>").map_err(|_|"Could not create error page".to_string())?;
        page = doc.layout_with(640, fonts);
    }
    Ok(Loaded {
        url,
        title,
        page,
        document: doc,
        status: response.status,
        script_errors,
        image_errors,
    })
}
