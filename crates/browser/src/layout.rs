mod formatting;
use crate::css::{self, Style};
use crate::*;
use alloc::string::ToString;
use formatting::{fragment, row};

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
    pub underline: bool,
    pub radius: u32,
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
/// Content-free retained layout counters for optional developer diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Summary {
    pub items: usize,
    pub links: usize,
    pub controls: usize,
    pub images: usize,
    pub truncated: bool,
}
impl Page {
    /// Count distinct rendered destinations; hidden DOM links are not evidence
    /// of visible navigation. No page text or URL is copied into this summary.
    pub fn summary(&self) -> Summary {
        let links: alloc::collections::BTreeSet<_> = self
            .items
            .iter()
            .filter_map(|item| item.link.as_deref())
            .collect();
        Summary {
            items: self.items.len(),
            links: links.len(),
            controls: self.controls.len(),
            images: self.images.len(),
            truncated: self.truncated,
        }
    }
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
    align: css::Align,
    item_start: usize,
    control_start: usize,
}
impl Flow {
    fn new(left: i32, right: i32, y: i32, page: &Page) -> Self {
        Self {
            left,
            right,
            x: left,
            y,
            line: 0,
            space: false,
            align: css::Align::Start,
            item_start: page.items.len(),
            control_start: page.controls.len(),
        }
    }
    fn newline(&mut self, page: &mut Page) {
        let spare = (self.right - self.x).max(0);
        let offset = match self.align {
            css::Align::Start => 0,
            css::Align::Center => spare / 2,
            css::Align::End => spare,
        };
        if offset != 0 {
            for item in &mut page.items[self.item_start..] {
                item.x += offset;
            }
            for control in &mut page.controls[self.control_start..] {
                control.x += offset;
            }
        }
        self.y += self.line as i32;
        self.x = self.left;
        self.line = 0;
        self.space = false;
        self.item_start = page.items.len();
        self.control_start = page.controls.len();
    }
    fn flush(&mut self, page: &mut Page) {
        if self.x > self.left || self.line > 0 {
            self.newline(page);
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
    let height = style
        .line_height
        .unwrap_or_else(|| metrics.line_height(style.font))
        .max(1);
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
            flow.newline(page);
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
            && last.underline == style.underline
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
            underline: style.underline,
            radius: 0,
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
                flow.newline(page);
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
                    flow.newline(page);
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
    let style = css::style(doc, id, parent);
    if style.hidden {
        return;
    }
    if style.atomic || style.float_right {
        let fragment = fragment(
            doc,
            id,
            &style,
            link,
            (flow.right - flow.left).max(0) as u32,
            page,
            metrics,
        );
        if flow.x + fragment.width as i32 > flow.right && flow.x > flow.left {
            flow.newline(page);
        }
        let x = if style.float_right {
            flow.right - fragment.width as i32
        } else {
            flow.x
        };
        fragment.translate(page, x, flow.y);
        flow.x = if style.float_right {
            flow.right
        } else {
            flow.x + fragment.width as i32
        };
        flow.line = flow.line.max(fragment.height);
        return;
    }
    node_styled(doc, id, &style, link, flow, page, metrics);
}

fn node_styled(
    doc: &Document,
    id: usize,
    style: &Style,
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
    if style.hidden {
        return;
    }
    let link = if n.tag == "a" { n.attr("href") } else { link };
    if n.tag.is_empty() {
        text(page, flow, &n.text, style, link, metrics);
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
            let extra = style.margin.horizontal() + style.padding.horizontal();
            let available = ((flow.right - flow.left).max(0) as u32).saturating_sub(extra);
            let fitted = width.min(available);
            let height = ((u64::from(height) * u64::from(fitted)) / u64::from(width)) as u32;
            if style.block {
                flow.flush(page);
            }
            if flow.x + (fitted + extra) as i32 > flow.right {
                flow.newline(page);
            }
            push(
                page,
                Item {
                    x: flow.x + (style.margin.left + style.padding.left) as i32,
                    y: flow.y + (style.margin.top + style.padding.top) as i32,
                    width: fitted,
                    height,
                    text: String::new(),
                    color: 0,
                    background: None,
                    scale: 1,
                    font: style.font,
                    image: Some(resource),
                    link: link.map(str::to_string),
                    underline: false,
                    radius: 0,
                },
            );
            flow.x += (fitted + extra) as i32;
            flow.line = flow
                .line
                .max(height + style.margin.vertical() + style.padding.vertical());
            if style.block {
                flow.newline(page);
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
        let (kind, label_len) = (data.kind, data.value.len());
        if kind == FieldKind::Hidden {
            return;
        }
        let legacy = style.width.is_none() && style.height.is_none();
        if style.block || (legacy && kind.is_text()) {
            flow.flush(page);
        }
        let available = (flow.right - flow.left).max(0) as u32;
        let extra = style.padding.horizontal() + style.border * 2;
        let width = if let Some(width) = style.width {
            (if style.border_box {
                width
            } else {
                width + extra
            })
            .max(style.min_content_width() + extra)
            .min(available)
        } else if kind.is_text() {
            400.min(available)
        } else {
            (label_len.max(6) as u32 * 8 + 20).min(available)
        };
        if flow.x + width as i32 > flow.right {
            flow.newline(page);
        }
        let height = style
            .height
            .map(|h| {
                if style.border_box {
                    h
                } else {
                    h + style.padding.vertical() + style.border * 2
                }
            })
            .unwrap_or(26)
            .max(style.min_content_height() + style.padding.vertical() + style.border * 2);
        page.controls.push(Control {
            form,
            field,
            x: flow.x,
            y: flow.y,
            width,
            height,
            font: style.font,
            color: style.color,
            background: style.background,
            border: style.border,
            padding: [
                style.padding.top,
                style.padding.right,
                style.padding.bottom,
                style.padding.left,
            ],
        });
        flow.x += width as i32 + if legacy { 8 } else { 0 };
        flow.line = flow.line.max(height + if legacy { 8 } else { 0 });
        if style.block || (legacy && kind.is_text()) {
            flow.newline(page);
        }
        return;
    }
    if n.tag == "br" {
        flow.line = flow.line.max(
            style
                .line_height
                .unwrap_or_else(|| metrics.line_height(style.font)),
        );
        flow.newline(page);
        return;
    }
    if n.tag == "hr" {
        flow.flush(page);
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
                underline: false,
                radius: 0,
            },
        );
        flow.y += 12;
        return;
    }
    let (old_left, old_right, old_align) = (flow.left, flow.right, flow.align);
    if style.block {
        flow.flush(page);
        flow.y += style.margin.top as i32;
        let available = (flow.right - flow.left).max(0) as u32;
        let extra = style.padding.horizontal() + style.border * 2;
        let width = style
            .width
            .map(|w| if style.border_box { w } else { w + extra })
            .unwrap_or(available.saturating_sub(style.margin.horizontal()))
            .max(style.min_content_width() + extra)
            .min(available.saturating_sub(style.margin.horizontal()));
        let spare = available.saturating_sub(style.margin.horizontal() + width);
        let auto = if style.auto_left {
            if style.auto_right {
                spare / 2
            } else {
                spare
            }
        } else {
            0
        };
        flow.left += (style.margin.left + auto) as i32;
        flow.right = flow.left + width as i32;
        flow.x = flow.left;
        flow.align = style.align;
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
                link: link.map(str::to_string),
                underline: false,
                radius: style.radius,
            },
        );
        Some(at)
    } else {
        None
    };
    if style.block {
        flow.left += (style.padding.left + style.border) as i32;
        flow.right = flow
            .right
            .saturating_sub((style.padding.right + style.border) as i32)
            .max(flow.left);
        flow.x = flow.left;
        flow.y += (style.padding.top + style.border) as i32;
        flow.item_start = page.items.len();
        flow.control_start = page.controls.len();
    }
    let content_top = flow.y;
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
    if style.flex {
        row(doc, id, style, link, flow, page, metrics);
    } else {
        for &child in &n.children {
            node(doc, child, style, link, flow, page, metrics);
        }
    }
    if style.block {
        flow.flush(page);
        let extra = style.padding.vertical() + style.border * 2;
        let height = style
            .height
            .map(|h| {
                if style.border_box {
                    h.saturating_sub(extra)
                } else {
                    h
                }
            })
            .unwrap_or((flow.y - content_top).max(0) as u32)
            .max(style.min_content_height());
        flow.y = content_top + height as i32 + (style.padding.bottom + style.border) as i32;
    }
    if let Some(index) = background_index {
        if let Some(item) = page.items.get_mut(index) {
            item.height = (flow.y - top)
                .max(if style.block { 0 } else { flow.line as i32 })
                .max(0) as u32;
        }
    }
    if style.block {
        flow.y += style.margin.bottom as i32;
        flow.left = old_left;
        flow.right = old_right;
        flow.x = flow.left;
        flow.align = old_align;
        flow.item_start = page.items.len();
        flow.control_start = page.controls.len();
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
    let mut flow = Flow::new(0, width, 0, &page);
    for (id, n) in doc.nodes.iter().enumerate() {
        if n.parent.is_none() {
            let mut root = Style::root();
            root.font.size = metrics.default_size();
            root.font.family = rustos_text::Family::Serif;
            node(doc, id, &root, None, &mut flow, &mut page, metrics);
        }
    }
    flow.flush(&mut page);
    page.height = flow.y.max(0) as u32;
    page
}
