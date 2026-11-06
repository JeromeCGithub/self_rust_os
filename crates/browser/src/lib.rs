//! Small, bounded HTML/CSS engine. No network, OS, framebuffer or script access.
#![no_std]
extern crate alloc;
mod css;
mod encoding;
mod forms;
pub use encoding::{decode_html, Encoding};
pub use forms::{Control, Field, FieldKind, Form};
mod html;
mod layout;
mod url;
use alloc::{string::String, sync::Arc, vec::Vec};
pub use rustos_web_image::Image;
#[derive(Clone, Debug)]
pub struct ImageResource {
    pub node: usize,
    pub image: Arc<Image>,
}
pub use layout::{Item, Page};
pub use url::{address, resolve};

pub const MAX_HTML: usize = 256 * 1024;
pub const MAX_CSS: usize = 128 * 1024;
pub const MAX_NODES: usize = 4096;
pub const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Limit,
    InvalidUrl,
    UnsupportedForm,
    Encoding,
}

/// Flat arena: indices own relationships, never references into movable storage.
#[derive(Debug, Clone)]
pub struct Node {
    pub tag: String,
    pub text: String,
    pub attributes: Vec<(String, String)>,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
}
impl Node {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Sheet {
    Inline(String),
    External(String),
}
#[derive(Debug, Clone)]
pub struct Document {
    pub title: String,
    pub nodes: Vec<Node>,
    pub stylesheets: Vec<String>,
    pub forms: Vec<Form>,
    rules: Vec<css::Rule>,
    css_bytes: usize,
    images: Vec<ImageResource>,
    sheets: Vec<Sheet>,
}
impl Document {
    /// Parse a UTF-8 HTML subset. Malformed markup recovers; budgets return Limit.
    pub fn parse(html: &str) -> Result<Self, Error> {
        html::parse(html)
    }
    /// Append author CSS, preserving source order. Unsupported syntax is ignored.
    pub fn add_css(&mut self, css: &str) -> Result<(), Error> {
        if css.len() > MAX_CSS.saturating_sub(self.css_bytes) {
            return Err(Error::Limit);
        }
        let rules = css::parse(css)?;
        if self.rules.len() + rules.len() > 512 {
            return Err(Error::Limit);
        }
        self.rules.extend(rules);
        self.css_bytes += css.len();
        Ok(())
    }
    /// Rebuild author rules in HTML source order. The caller supplies resource
    /// policy; missing or oversized optional sheets leave the remaining cascade.
    pub fn apply_stylesheets(&mut self, mut fetch: impl FnMut(&str) -> Option<String>) -> usize {
        self.rules.clear();
        self.css_bytes = 0;
        let mut failed = 0;
        for source in self.sheets.clone() {
            let css = match source {
                Sheet::Inline(css) => Some(css),
                Sheet::External(url) => fetch(&url),
            };
            match css {
                Some(css) => {
                    if self.add_css(&css).is_err() {
                        failed += 1;
                    }
                }
                None => failed += 1,
            }
        }
        failed
    }
    /// Associate one decoded, budgeted image with an existing img element.
    pub fn set_image(&mut self, node: usize, image: Arc<Image>) -> Result<(), Error> {
        if !self.nodes.get(node).is_some_and(|node| node.tag == "img")
            || image.width == 0
            || image.height == 0
            || image.width > 4096
            || image.height > 4096
            || (image.width as usize)
                .checked_mul(image.height as usize)
                .and_then(|n| n.checked_mul(4))
                != Some(image.rgba.len())
            || image.rgba.len() > rustos_web_image::MAX_PIXELS * 4
        {
            return Err(Error::Limit);
        }
        let replacing = self
            .images
            .iter()
            .position(|resource| resource.node == node);
        let used = self
            .images
            .iter()
            .filter(|resource| resource.node != node)
            .fold(0usize, |sum, resource| {
                sum.saturating_add(resource.image.rgba.len())
            });
        if used.saturating_add(image.rgba.len()) > 16 * 1024 * 1024
            || (replacing.is_none() && self.images.len() == 16)
        {
            return Err(Error::Limit);
        }
        if let Some(at) = replacing {
            self.images[at] = ImageResource { node, image };
        } else {
            self.images.push(ImageResource { node, image });
        }
        Ok(())
    }
    /// Layout with the exact font metrics used by the painter.
    pub fn layout_with(&self, width: u32, metrics: &impl rustos_text::Metrics) -> Page {
        layout::layout_with(self, width, metrics)
    }
    /// Block/inline flow in document pixels. A zero width returns an empty page.
    pub fn layout(&self, width: u32) -> Page {
        layout::layout(self, width)
    }
}

#[cfg(feature = "javascript")]
mod script;
#[cfg(feature = "javascript")]
pub use script::{ScriptError, ScriptReport};
pub mod dom;
