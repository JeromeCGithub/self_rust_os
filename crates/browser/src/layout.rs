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
    pub font: rustos_text::Style,
    pub image: Option<usize>,
    pub link: Option<String>,
}
pub struct Page {
    pub items: Vec<Item>,
    pub height: u32,
    pub background: u32,
    pub truncated: bool,
    pub forms: Vec<Form>,
    pub controls: Vec<Control>,
    pub images: Vec<ImageResource>,
}
impl Page {
    /// Whether layout exposes any readable text or interactive control.
    pub fn is_empty(&self) -> bool {
        self.controls.is_empty()
            && !self
                .items
                .iter()
                .any(|item| (!item.text.trim().is_empty() || item.image.is_some()))
    }
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
fn run(
    page: &mut Page,
    flow: &mut Flow,
    value: &str,
    style: &Style,
    link: Option<&str>,
    metrics: &impl rustos_text::Metrics,
) {
    let available = (flow.right - flow.left).max(0) as u32;
    if available < 8 {
        return;
    }
    let height = metrics.line_height(style.font).max(1);
    let mut text = String::new();
    let mut width = 0u32;
    for ch in value.chars() {
        let advance = metrics.advance(ch, style.font).min(available);
        if flow.x + advance as i32 > flow.right {
            if !text.is_empty() {
                emit_text(
                    page,
                    flow.x - width as i32,
                    flow.y,
                    width,
                    height,
                    core::mem::take(&mut text),
                    style,
                    link,
                );
                width = 0;
            }
            flow.newline();
        }
        text.push(ch);
        width += advance;
        flow.x += advance as i32;
        flow.line = flow.line.max(height);
    }
    if !text.is_empty() {
        emit_text(
            page,
            flow.x - width as i32,
            flow.y,
            width,
            height,
            text,
            style,
            link,
        );
    }
}
fn emit_text(
    page: &mut Page,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    text: String,
    style: &Style,
    link: Option<&str>,
) {
    if let Some(last) = page.items.last_mut() {
        if last.y == y
            && last.x + last.width as i32 == x
            && last.color == style.color
            && last.font == style.font
            && last.link.as_deref() == link
            && last.background.is_none()
            && last.image.is_none()
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
            y,
            width,
            height,
            text,
            color: style.color,
            background: None,
            scale: style.scale,
            font: style.font,
            image: None,
            link: link.map(str::to_string),
        },
    );
}
fn text(
    page: &mut Page,
    flow: &mut Flow,
    value: &str,
    style: &Style,
    link: Option<&str>,
    metrics: &impl rustos_text::Metrics,
) {
    if style.pre {
        for (i, line) in value.split('\n').enumerate() {
            if i > 0 {
                flow.newline();
            }
            run(
                page,
                flow,
                &line.replace('\t', "    "),
                style,
                link,
                metrics,
            );
        }
        return;
    }
    // Whitespace collapses across element boundaries, too.
    for word in value.split_inclusive(char::is_whitespace) {
        let trimmed = word.trim_end_matches(char::is_whitespace);
        if !trimmed.is_empty() {
            let width = trimmed.chars().fold(0u32, |sum, ch| {
                sum.saturating_add(metrics.advance(ch, style.font))
            });
            if flow.space && flow.x > flow.left {
                if flow.x as i64 + metrics.advance(' ', style.font) as i64 + width as i64
                    > flow.right as i64
                {
                    flow.newline();
                } else {
                    run(page, flow, " ", style, link, metrics);
                }
            }
            run(page, flow, trimmed, style, link, metrics);
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
    metrics: &impl rustos_text::Metrics,
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
        text(page, flow, &n.text, &style, link, metrics);
        return;
    }
    if n.tag == "img" {
        if let Some((resource, data)) = page
            .images
            .iter()
            .enumerate()
            .find(|(_, asset)| asset.node == id)
        {
            let intrinsic_width = data.image.width;
            let intrinsic_height = data.image.height;
            let width = style
                .width
                .or_else(|| n.attr("width").and_then(|n| n.parse::<u32>().ok()))
                .unwrap_or(intrinsic_width)
                .min(4096);
            let height = style
                .height
                .or_else(|| n.attr("height").and_then(|n| n.parse::<u32>().ok()))
                .unwrap_or_else(|| {
                    ((u64::from(intrinsic_height) * u64::from(width)) / u64::from(intrinsic_width))
                        as u32
                })
                .min(4096);
            if width == 0 || height == 0 {
                return;
            }
            let available = (flow.right - flow.left).max(0) as u32;
            let fitted = width.min(available);
            let height = ((u64::from(height) * u64::from(fitted)) / u64::from(width)) as u32;
            if style.block {
                flow.flush();
            }
            if flow.x + fitted as i32 > flow.right {
                flow.newline();
            }
            push(
                page,
                Item {
                    x: flow.x,
                    y: flow.y,
                    width: fitted,
                    height,
                    text: String::new(),
                    color: 0,
                    background: None,
                    scale: 1,
                    font: style.font,
                    image: Some(resource),
                    link: link.map(str::to_string),
                },
            );
            flow.x += fitted as i32;
            flow.line = flow.line.max(height);
            if style.block {
                flow.newline();
            }
            return;
        }
    }
    if let Some((form, field)) = page.forms.iter().enumerate().find_map(|(form, f)| {
        f.fields
            .iter()
            .position(|field| field.node == id)
            .map(|field| (form, field))
    }) {
        let data = &page.forms[form].fields[field];
        if data.kind == FieldKind::Hidden {
            return;
        }
        if data.kind == FieldKind::Text {
            flow.flush();
        }
        let available = (flow.right - flow.left).max(0) as u32;
        let width = if data.kind == FieldKind::Text {
            400.min(available)
        } else {
            (data.value.len().max(6) as u32 * 8 + 20).min(available)
        };
        if flow.x + width as i32 > flow.right {
            flow.newline();
        }
        page.controls.push(Control {
            form,
            field,
            x: flow.x,
            y: flow.y,
            width,
            height: 26,
        });
        flow.x += width as i32 + 8;
        flow.line = flow.line.max(34);
        if data.kind == FieldKind::Text {
            flow.newline();
        }
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
                font: style.font,
                image: None,
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
                font: style.font,
                image: None,
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
        text(page, flow, "* ", &style, None, metrics);
    }
    if n.tag == "img" {
        if let Some(alt) = n.attr("alt") {
            text(page, flow, alt, &style, link, metrics);
        }
    }
    if !n.text.is_empty() {
        text(page, flow, &n.text, &style, link, metrics);
    }
    for &child in &n.children {
        node(doc, child, &style, link, flow, page, metrics);
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
    layout_with(doc, width, &rustos_text::Bitmap)
}
pub(crate) fn layout_with(doc: &Document, width: u32, metrics: &impl rustos_text::Metrics) -> Page {
    let mut page = Page {
        items: Vec::new(),
        height: 0,
        background: 0xffffff,
        truncated: false,
        forms: doc.forms.clone(),
        controls: Vec::new(),
        images: doc.images.clone(),
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
            let mut root = Style::root();
            root.font.size = metrics.default_size();
            root.font.family = rustos_text::Family::Serif;
            node(doc, id, &root, None, &mut flow, &mut page, metrics);
        }
    }
    flow.flush();
    page.height = (flow.y + 8) as u32;
    page
}
