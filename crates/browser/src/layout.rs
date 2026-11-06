use crate::css::{self, Style};
use crate::*;
use alloc::string::ToString;

/// Retained paint/hit-test record in document coordinates; colors are RGB.
#[derive(Debug)]
pub struct Item {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub text: String,
    pub color: u32,
    pub background: Option<u32>,
    pub scale: u32,
    pub link: Option<String>,
}
pub struct Page {
    pub items: Vec<Item>,
    pub height: u32,
    pub background: u32,
    pub truncated: bool,
}
impl Page {
    pub fn hit(&self, x: i32, y: i32) -> Option<&str> {
        self.items
            .iter()
            .rev()
            .find(|i| {
                x >= i.x
                    && y >= i.y
                    && i64::from(x) < i64::from(i.x) + i64::from(i.width)
                    && i64::from(y) < i64::from(i.y) + i64::from(i.height)
                    && i.link.is_some()
            })
            .and_then(|i| i.link.as_deref())
    }
}
struct Flow {
    left: i32,
    right: i32,
    x: i32,
    y: i32,
    line: u32,
    space: bool,
}
impl Flow {
    fn newline(&mut self) {
        self.y += self.line.max(12) as i32;
        self.x = self.left;
        self.line = 0;
        self.space = false;
    }
    fn flush(&mut self) {
        if self.x > self.left || self.line > 0 {
            self.newline();
        }
    }
}
fn push(page: &mut Page, item: Item) {
    if page.items.len() == 16384 {
        page.truncated = true;
    } else {
        page.items.push(item);
    }
}
fn run(page: &mut Page, flow: &mut Flow, value: &str, style: &Style, link: Option<&str>) {
    if value.is_empty() {
        return;
    }
    let cell = 8 * style.scale;
    let available = (flow.right - flow.left).max(1) as u32;
    let scale = style.scale.min((available / 8).max(1));
    let cell = cell.min(8 * scale);
    let height = cell + 4;
    let mut text = String::new();
    for ch in value.chars() {
        if flow.x + cell as i32 > flow.right {
            if !text.is_empty() {
                let width = text.chars().count() as u32 * cell;
                push(
                    page,
                    Item {
                        x: flow.x - width as i32,
                        y: flow.y,
                        width,
                        height,
                        text: core::mem::take(&mut text),
                        color: style.color,
                        background: None,
                        scale,
                        link: link.map(str::to_string),
                    },
                );
            }
            flow.newline();
        }
        // A viewport narrower than one glyph has no visible text cells.
        if available < 8 {
            continue;
        }
        text.push(ch);
        flow.x += cell as i32;
        flow.line = flow.line.max(height);
    }
    if !text.is_empty() {
        let width = text.chars().count() as u32 * cell;
        let x = flow.x - width as i32;
        // Merge adjacent text, preserving observable words, links and spacing.
        if let Some(last) = page.items.last_mut() {
            if last.y == flow.y
                && last.x + last.width as i32 == x
                && last.color == style.color
                && last.scale == scale
                && last.link.as_deref() == link
                && last.background.is_none()
                && last.text != " "
            {
                last.text.push_str(&text);
                last.width += width;
                return;
            }
        }
        push(
            page,
            Item {
                x,
                y: flow.y,
                width,
                height,
                text,
                color: style.color,
                background: None,
                scale,
                link: link.map(str::to_string),
            },
        );
    }
}
fn text(page: &mut Page, flow: &mut Flow, value: &str, style: &Style, link: Option<&str>) {
    if style.pre {
        for (i, line) in value.split('\n').enumerate() {
            if i > 0 {
                flow.newline();
            }
            run(page, flow, &line.replace('\t', "    "), style, link);
        }
        return;
    }
    // Whitespace collapses across element boundaries, too.
    for word in value.split_inclusive(char::is_whitespace) {
        let trimmed = word.trim_end_matches(char::is_whitespace);
        if !trimmed.is_empty() {
            let width = (trimmed.chars().count() as u32).saturating_mul(8 * style.scale);
            if flow.space && flow.x > flow.left {
                if flow.x as i64 + 8 * style.scale as i64 + width as i64 > flow.right as i64 {
                    flow.newline();
                } else {
                    run(page, flow, " ", style, link);
                }
            }
            run(page, flow, trimmed, style, link);
            flow.space = false;
        }
        if word.len() != trimmed.len() {
            flow.space = true;
        }
    }
}
fn node(
    doc: &Document,
    id: usize,
    parent: &Style,
    link: Option<&str>,
    flow: &mut Flow,
    page: &mut Page,
) {
    if page.truncated || flow.y > 1_000_000 {
        page.truncated = true;
        return;
    }
    let n = &doc.nodes[id];
    let style = css::style(doc, id, parent);
    if style.hidden {
        return;
    }
    let link = if n.tag == "a" { n.attr("href") } else { link };
    if n.tag.is_empty() {
        text(page, flow, &n.text, &style, link);
        return;
    }
    if n.tag == "br" {
        flow.newline();
        return;
    }
    if n.tag == "hr" {
        flow.flush();
        push(
            page,
            Item {
                x: flow.left,
                y: flow.y,
                width: (flow.right - flow.left).max(0) as u32,
                height: 1,
                text: String::new(),
                color: 0,
                background: Some(0xaaaaaa),
                scale: 1,
                link: None,
            },
        );
        flow.y += 12;
        return;
    }
    let (old_left, old_right) = (flow.left, flow.right);
    if style.block {
        flow.flush();
        let inset = style
            .margin
            .min(((flow.right - flow.left).max(0) / 4) as u32) as i32;
        flow.y += style.margin as i32;
        flow.left += inset;
        flow.right -= inset;
        flow.x = flow.left;
    }
    if let Some(width) = style.width {
        flow.right = flow.right.min(flow.left + width.max(8) as i32);
    }
    let top = flow.y;
    let background_index = if let Some(color) = style.background {
        if n.tag == "body" || n.tag == "html" {
            page.background = color;
        }
        let at = page.items.len();
        push(
            page,
            Item {
                x: flow.left,
                y: top,
                width: (flow.right - flow.left).max(0) as u32,
                height: 0,
                text: String::new(),
                color: 0,
                background: Some(color),
                scale: 1,
                link: None,
            },
        );
        Some(at)
    } else {
        None
    };
    let padding = style
        .padding
        .min(((flow.right - flow.left).max(0) / 4) as u32) as i32;
    if style.block {
        flow.left += padding;
        flow.right -= padding;
        flow.x = flow.left;
        flow.y += padding;
    }
    if n.tag == "li" {
        text(page, flow, "* ", &style, None);
    }
    if n.tag == "img" {
        if let Some(alt) = n.attr("alt") {
            text(page, flow, alt, &style, link);
        }
    }
    for &child in &n.children {
        node(doc, child, &style, link, flow, page);
    }
    if style.block {
        flow.flush();
        flow.y += padding;
    }
    if let Some(index) = background_index {
        if let Some(item) = page.items.get_mut(index) {
            item.height = (flow.y - top).max(flow.line as i32) as u32;
        }
    }
    if style.block {
        flow.y += style.margin as i32;
        flow.left = old_left;
        flow.right = old_right;
        flow.x = flow.left;
    } else {
        flow.right = old_right;
    }
}
pub(crate) fn layout(doc: &Document, width: u32) -> Page {
    let mut page = Page {
        items: Vec::new(),
        height: 0,
        background: 0xffffff,
        truncated: false,
    };
    if width == 0 {
        return page;
    }
    let width = width.min(16384) as i32;
    let margin = 8.min(width / 4);
    let mut flow = Flow {
        left: margin,
        right: width - margin,
        x: margin,
        y: 8,
        line: 0,
        space: false,
    };
    for (id, n) in doc.nodes.iter().enumerate() {
        if n.parent.is_none() {
            node(doc, id, &Style::root(), None, &mut flow, &mut page);
        }
    }
    flow.flush();
    page.height = (flow.y + 8) as u32;
    page
}
