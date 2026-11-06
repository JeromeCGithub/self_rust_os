//! Small, bounded HTML/CSS engine. No network, OS, framebuffer or script access.
#![no_std]
extern crate alloc;
mod css;
mod html;
mod layout;
mod url;
use alloc::{string::String, vec::Vec};
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
}

/// Flat arena: indices own relationships, never references into movable storage.
#[derive(Debug)]
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

#[derive(Debug)]
pub struct Document {
    pub title: String,
    pub nodes: Vec<Node>,
    pub stylesheets: Vec<String>,
    rules: Vec<css::Rule>,
    css_bytes: usize,
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
    /// Block/inline flow in document pixels. A zero width returns an empty page.
    pub fn layout(&self, width: u32) -> Page {
        layout::layout(self, width)
    }
}

#[cfg(feature = "javascript")]
mod script;
#[cfg(feature = "javascript")]
pub use script::{ScriptError, ScriptReport};
