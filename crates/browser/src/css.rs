use crate::*;
use alloc::string::ToString;

#[derive(Debug, Clone)]
pub(crate) struct Rule {
    selector: String,
    declarations: Vec<(String, String)>,
    specificity: u32,
}

pub(crate) fn declarations(source: &str) -> Vec<(String, String)> {
    source
        .split(';')
        .filter_map(|part| part.split_once(':'))
        .take(64)
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_ascii_lowercase()))
        .collect()
}
pub(crate) fn parse(source: &str) -> Result<Vec<Rule>, Error> {
    let mut clean = String::new();
    let mut rest = source;
    while let Some(at) = rest.find("/*") {
        clean.push_str(&rest[..at]);
        rest = rest[at + 2..]
            .find("*/")
            .map(|n| &rest[at + 2 + n + 2..])
            .unwrap_or("");
    }
    clean.push_str(rest);
    let mut rules = Vec::new();
    rest = &clean;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open + 1..].find('}').map(|n| n + open + 1) else {
            break;
        };
        let selector = rest[..open].trim();
        // Skip whole at-rule groups, including nested media/supports blocks.
        if selector.starts_with('@') {
            let mut depth = 1;
            let mut end = rest.len();
            for (i, ch) in rest[open + 1..].char_indices() {
                if ch == '{' {
                    depth += 1;
                }
                if ch == '}' {
                    depth -= 1;
                    if depth == 0 {
                        end = open + 1 + i + 1;
                        break;
                    }
                }
            }
            rest = &rest[end..];
            continue;
        }
        for selector in selector.split(',').map(str::trim) {
            if selector.is_empty()
                || selector.len() > 256
                || selector.split_whitespace().count() > 8
            {
                continue;
            }
            if !selector
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"*#.-_ \t\r\n".contains(&b))
            {
                continue;
            }
            let specificity = selector
                .split_whitespace()
                .map(|part| {
                    part.bytes().filter(|b| *b == b'#').count() as u32 * 100
                        + part.bytes().filter(|b| *b == b'.').count() as u32 * 10
                        + u32::from(part.as_bytes()[0].is_ascii_alphabetic())
                })
                .sum();
            if rules.len() == 512 {
                return Err(Error::Limit);
            }
            rules.push(Rule {
                selector: selector.to_string(),
                declarations: declarations(&rest[open + 1..close]),
                specificity,
            });
        }
        rest = &rest[close + 1..];
    }
    Ok(rules)
}
fn compound(node: &Node, mut part: &str) -> bool {
    let end = part.find(['.', '#']).unwrap_or(part.len());
    if end > 0 && &part[..end] != "*" && !node.tag.eq_ignore_ascii_case(&part[..end]) {
        return false;
    }
    part = &part[end..];
    while !part.is_empty() {
        let kind = part.as_bytes()[0];
        part = &part[1..];
        let end = part.find(['.', '#']).unwrap_or(part.len());
        let value = &part[..end];
        if value.is_empty()
            || match kind {
                b'#' => node.attr("id") != Some(value),
                b'.' => !node
                    .attr("class")
                    .is_some_and(|c| c.split_whitespace().any(|c| c == value)),
                _ => true,
            }
        {
            return false;
        }
        part = &part[end..];
    }
    true
}
fn matches(doc: &Document, id: usize, selector: &str) -> bool {
    let mut parts = selector.split_whitespace().rev();
    if !parts.next().is_some_and(|p| compound(&doc.nodes[id], p)) {
        return false;
    }
    let mut parent = doc.nodes[id].parent;
    for part in parts {
        loop {
            let Some(id) = parent else {
                return false;
            };
            parent = doc.nodes[id].parent;
            if compound(&doc.nodes[id], part) {
                break;
            }
        }
    }
    true
}
pub(crate) fn color(value: &str) -> Option<u32> {
    if let Some(hex) = value.strip_prefix('#') {
        return match hex.len() {
            6 => u32::from_str_radix(hex, 16).ok(),
            3 => u32::from_str_radix(hex, 16).ok().map(|v| {
                let red = ((v >> 8) & 15) * 17;
                let green = ((v >> 4) & 15) * 17;
                let blue = (v & 15) * 17;
                (red << 16) | (green << 8) | blue
            }),
            _ => None,
        };
    }
    Some(match value {
        "black" => 0,
        "white" => 0xffffff,
        "red" => 0xff0000,
        "green" => 0x008000,
        "blue" => 0x0000ff,
        "gray" | "grey" => 0x808080,
        "yellow" => 0xffff00,
        "navy" => 0x000080,
        "purple" => 0x800080,
        "silver" => 0xc0c0c0,
        _ => return None,
    })
}
fn pixels(value: &str) -> Option<u32> {
    value
        .strip_suffix("px")
        .unwrap_or(value)
        .parse::<u32>()
        .ok()
        .map(|n| n.min(1024))
}
#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct Edges {
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
    pub left: u32,
}
impl Edges {
    fn all(value: u32) -> Self {
        Self {
            top: value,
            right: value,
            bottom: value,
            left: value,
        }
    }
    fn parse(value: &str, auto: bool) -> Option<(Self, bool, bool)> {
        let parts: Vec<_> = value.split_whitespace().collect();
        let length = |value: &str| {
            if auto && value == "auto" {
                Some(0)
            } else {
                pixels(value)
            }
        };
        let (top, right, bottom, left) = match parts.as_slice() {
            [a] => (*a, *a, *a, *a),
            [a, b] => (*a, *b, *a, *b),
            [a, b, c] => (*a, *b, *c, *b),
            [a, b, c, d] => (*a, *b, *c, *d),
            _ => return None,
        };
        Some((
            Self {
                top: length(top)?,
                right: length(right)?,
                bottom: length(bottom)?,
                left: length(left)?,
            },
            left == "auto",
            right == "auto",
        ))
    }
    pub fn horizontal(self) -> u32 {
        self.left + self.right
    }
    pub fn vertical(self) -> u32 {
        self.top + self.bottom
    }
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Align {
    #[default]
    Start,
    Center,
    End,
}
fn align(value: &str) -> Option<Align> {
    Some(match value {
        "left" | "start" | "flex-start" => Align::Start,
        "center" => Align::Center,
        "right" | "end" | "flex-end" => Align::End,
        _ => return None,
    })
}
fn font_length(value: &str, size: u16) -> Option<u32> {
    let (number, factor) = if let Some(n) = value.strip_suffix("em") {
        (n, f32::from(size))
    } else if let Some(n) = value.strip_suffix('%') {
        (n, f32::from(size) / 100.0)
    } else {
        (value.strip_suffix("px").unwrap_or(value), 1.0)
    };
    let value = number.parse::<f32>().ok()? * factor;
    (value.is_finite() && value >= 0.0).then(|| (value + 0.5).min(1024.0) as u32)
}
#[derive(Clone)]
pub(crate) struct Style {
    pub color: u32,
    pub background: Option<u32>,
    pub scale: u32,
    pub font: rustos_text::Style,
    pub block: bool,
    pub hidden: bool,
    pub pre: bool,
    pub margin: Edges,
    pub padding: Edges,
    pub auto_left: bool,
    pub auto_right: bool,
    pub atomic: bool,
    pub flex: bool,
    pub table_row: bool,
    pub float_right: bool,
    pub align: Align,
    pub justify: Align,
    pub cross_align: Align,
    pub gap: u32,
    pub line_height: Option<u32>,
    pub min_width: u32,
    pub min_height: u32,
    pub border: u32,
    pub border_box: bool,
    pub underline: bool,
    pub radius: u32,
    pub width: Option<u32>,
    pub height: Option<u32>,
}
impl Style {
    pub fn min_content_width(&self) -> u32 {
        if self.border_box {
            self.min_width
                .saturating_sub(self.padding.horizontal() + self.border * 2)
        } else {
            self.min_width
        }
    }
    pub fn min_content_height(&self) -> u32 {
        if self.border_box {
            self.min_height
                .saturating_sub(self.padding.vertical() + self.border * 2)
        } else {
            self.min_height
        }
    }
    pub fn root() -> Self {
        Self {
            color: 0x202124,
            background: None,
            scale: 1,
            font: rustos_text::Style {
                size: 8,
                ..rustos_text::Style::default()
            },
            block: false,
            hidden: false,
            pre: false,
            margin: Edges::default(),
            padding: Edges::default(),
            auto_left: false,
            auto_right: false,
            atomic: false,
            flex: false,
            table_row: false,
            float_right: false,
            align: Align::Start,
            justify: Align::Start,
            cross_align: Align::Start,
            gap: 0,
            line_height: None,
            min_width: 0,
            min_height: 0,
            border: 0,
            border_box: false,
            underline: false,
            radius: 0,
            width: None,
            height: None,
        }
    }
    fn apply(&mut self, key: &str, value: &str) {
        match key {
            "color" => {
                if let Some(c) = color(value) {
                    self.color = c;
                }
            }
            "background" | "background-color" => {
                if let Some(c) = color(value) {
                    self.background = Some(c);
                }
            }
            "font-size" => {
                if let Some(n) = font_length(value, self.font.size).or_else(|| match value {
                    "small" => Some(13),
                    "medium" => Some(16),
                    "large" => Some(18),
                    _ => None,
                }) {
                    self.scale = ((n + 7) / 8).clamp(1, 4);
                    self.font.size = n.clamp(1, 96) as u16;
                }
            }
            "font" => {
                let tokens: Vec<_> = value.split_whitespace().collect();
                if let Some((index, part)) = tokens.iter().enumerate().find(|(_, part)| {
                    part.split('/')
                        .next()
                        .is_some_and(|v| v.ends_with("px") || v.ends_with("em") || v.ends_with('%'))
                }) {
                    let (size, line) = part
                        .split_once('/')
                        .map_or((*part, None), |(a, b)| (a, Some(b)));
                    if let Some(size) = font_length(size, self.font.size) {
                        self.font.bold = tokens[..index]
                            .iter()
                            .any(|v| *v == "bold" || v.parse::<u16>().is_ok_and(|n| n >= 600));
                        self.font.size = size.clamp(1, 96) as u16;
                        self.scale = size.div_ceil(8).clamp(1, 4);
                        self.line_height = None;
                        if let Some(line) = line {
                            self.apply("line-height", line);
                        }
                        self.apply("font-family", &tokens[index + 1..].join(" "));
                    }
                }
            }
            "line-height" => {
                self.line_height = if value == "normal" {
                    None
                } else if value.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
                    value
                        .parse::<f32>()
                        .ok()
                        .filter(|n| n.is_finite() && *n >= 0.0)
                        .map(|n| (n * f32::from(self.font.size) + 0.5).min(1024.0) as u32)
                } else {
                    font_length(value, self.font.size)
                };
            }
            "font-family" => {
                self.font.family = if value.contains("monospace") || value.contains("courier") {
                    rustos_text::Family::Monospace
                } else if value.contains("sans-serif")
                    || value.contains("arial")
                    || value.contains("helvetica")
                {
                    rustos_text::Family::Sans
                } else if value.contains("serif") || value.contains("times") {
                    rustos_text::Family::Serif
                } else {
                    self.font.family
                };
            }
            "font-weight" => {
                self.font.bold = value == "bold" || value.parse::<u16>().is_ok_and(|n| n >= 600)
            }
            "display" => {
                if [
                    "none",
                    "block",
                    "inline",
                    "inline-block",
                    "flex",
                    "inline-flex",
                ]
                .contains(&value)
                {
                    self.hidden = value == "none";
                    self.block = value == "block" || value == "flex";
                    self.atomic = value == "inline-block" || value == "inline-flex";
                    self.flex = value == "flex" || value == "inline-flex";
                }
            }
            "white-space" => self.pre = value == "pre" || value == "pre-wrap",
            "margin" => {
                if let Some((edges, left, right)) = Edges::parse(value, true) {
                    self.margin = edges;
                    self.auto_left = left;
                    self.auto_right = right;
                }
            }
            "padding" => {
                if let Some((edges, _, _)) = Edges::parse(value, false) {
                    self.padding = edges;
                }
            }
            "margin-top" => {
                if let Some(n) = pixels(value) {
                    self.margin.top = n
                }
            }
            "margin-bottom" => {
                if let Some(n) = pixels(value) {
                    self.margin.bottom = n
                }
            }
            "margin-left" => {
                if value == "auto" {
                    self.auto_left = true;
                    self.margin.left = 0
                } else if let Some(n) = pixels(value) {
                    self.margin.left = n;
                    self.auto_left = false
                }
            }
            "margin-right" => {
                if value == "auto" {
                    self.auto_right = true;
                    self.margin.right = 0
                } else if let Some(n) = pixels(value) {
                    self.margin.right = n;
                    self.auto_right = false
                }
            }
            "padding-top" => {
                if let Some(n) = pixels(value) {
                    self.padding.top = n
                }
            }
            "padding-bottom" => {
                if let Some(n) = pixels(value) {
                    self.padding.bottom = n
                }
            }
            "padding-left" => {
                if let Some(n) = pixels(value) {
                    self.padding.left = n
                }
            }
            "padding-right" => {
                if let Some(n) = pixels(value) {
                    self.padding.right = n
                }
            }
            "min-width" => {
                if let Some(n) = pixels(value) {
                    self.min_width = n
                }
            }
            "min-height" => {
                if let Some(n) = pixels(value) {
                    self.min_height = n
                }
            }
            "text-align" => {
                if let Some(n) = align(value) {
                    self.align = n
                }
            }
            "justify-content" => {
                if let Some(n) = align(value) {
                    self.justify = n
                }
            }
            "align-items" => {
                if let Some(n) = align(value) {
                    self.cross_align = n
                }
            }
            "gap" | "column-gap" => {
                if let Some(n) = pixels(value) {
                    self.gap = n
                }
            }
            "float" => self.float_right = value == "right",
            "box-sizing" => self.border_box = value == "border-box",
            "text-decoration" | "text-decoration-line" => {
                self.underline = value.split_whitespace().any(|v| v == "underline")
            }
            "border" | "border-width" => {
                if value == "none" || value == "0" {
                    self.border = 0
                } else if let Some(n) = value.split_whitespace().find_map(pixels) {
                    self.border = n.min(16)
                }
            }
            "width" => self.width = pixels(value),
            "height" => self.height = pixels(value),
            "border-radius" => {
                if let Some(n) = pixels(value) {
                    self.radius = n
                }
            }
            _ => {}
        }
    }
}
pub(crate) fn style(doc: &Document, id: usize, parent: &Style) -> Style {
    let node = &doc.nodes[id];
    let mut result = Style {
        color: parent.color,
        scale: parent.scale,
        font: parent.font,
        pre: parent.pre,
        align: parent.align,
        line_height: parent.line_height,
        underline: parent.underline,
        ..Style::root()
    };
    if node.tag.is_empty() {
        return result;
    }
    result.block = [
        "html",
        "body",
        "div",
        "p",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "ul",
        "ol",
        "li",
        "pre",
        "blockquote",
        "section",
        "article",
        "header",
        "footer",
        "main",
        "dl",
        "dt",
        "dd",
        "table",
        "tr",
        "center",
        "form",
        "tbody",
        "thead",
        "tfoot",
    ]
    .contains(&node.tag.as_str());
    result.hidden = [
        "head", "title", "style", "script", "link", "meta", "base", "template", "noscript",
    ]
    .contains(&node.tag.as_str());
    if ["p", "h1", "h2", "h3", "ul", "ol", "pre", "blockquote"].contains(&node.tag.as_str()) {
        result.margin = Edges::all(8);
    }
    if ["h1", "h2"].contains(&node.tag.as_str()) {
        result.scale = (parent.scale * 2).min(4);
        result.font.size = parent.font.size.saturating_mul(2).min(96);
    }
    if ["ul", "ol", "blockquote", "dd"].contains(&node.tag.as_str()) {
        result.padding = Edges::all(12);
    }
    if ["pre", "code", "kbd", "samp"].contains(&node.tag.as_str()) {
        result.font.family = rustos_text::Family::Monospace;
    }
    if ["b", "strong", "h1", "h2", "h3", "h4", "h5", "h6"].contains(&node.tag.as_str()) {
        result.font.bold = true;
    }
    if node.tag == "pre" {
        result.pre = true;
    }
    if node.tag == "a" && node.attr("href").is_some() {
        result.color = 0x1a56b0;
        result.underline = true;
    }
    if node.tag == "body" {
        result.margin = Edges::all(8);
    }
    if node.tag == "center" {
        result.align = Align::Center;
    }
    if node.tag == "table" {
        result.align = Align::Start;
    }
    if node.tag == "tr" {
        result.flex = true;
        result.table_row = true;
    }
    if let Some(value) = node.attr("align").and_then(align) {
        result.align = value;
    }
    if node.tag == "input" || node.tag == "button" || node.tag == "textarea" {
        result.color = 0;
        result.font.family = rustos_text::Family::Sans;
        result.font.size = 13;
        result.border = 2;
        result.line_height = None;
    }
    let inline = node.attr("style").map(declarations).unwrap_or_default();
    let mut candidates = Vec::new();
    for (order, rule) in doc
        .rules
        .iter()
        .enumerate()
        .filter(|(_, rule)| matches(doc, id, &rule.selector))
    {
        for (declaration, (key, value)) in rule.declarations.iter().enumerate() {
            let (value, important) = priority(value);
            candidates.push((
                important,
                rule.specificity,
                order,
                declaration,
                key.as_str(),
                value,
            ));
        }
    }
    for (declaration, (key, value)) in inline.iter().enumerate() {
        let (value, important) = priority(value);
        candidates.push((
            important,
            u32::MAX,
            usize::MAX,
            declaration,
            key.as_str(),
            value,
        ));
    }
    candidates.sort_by_key(|entry| (entry.0, entry.1, entry.2, entry.3));
    for (_, _, _, _, key, value) in candidates {
        if value == "inherit" {
            match key {
                "color" => result.color = parent.color,
                "font" => result.font = parent.font,
                "font-family" => result.font.family = parent.font.family,
                _ => {}
            }
        } else {
            result.apply(key, value);
        }
    }
    result
}

fn priority(value: &str) -> (&str, bool) {
    match value.rsplit_once('!') {
        Some((value, flag)) if flag.trim() == "important" => (value.trim(), true),
        _ => (value, false),
    }
}
