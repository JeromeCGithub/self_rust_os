use crate::*;
use alloc::string::ToString;

#[derive(Debug)]
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
#[derive(Clone)]
pub(crate) struct Style {
    pub color: u32,
    pub background: Option<u32>,
    pub scale: u32,
    pub font: rustos_text::Style,
    pub block: bool,
    pub hidden: bool,
    pub pre: bool,
    pub margin: u32,
    pub padding: u32,
    pub width: Option<u32>,
    pub height: Option<u32>,
}
impl Style {
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
            margin: 0,
            padding: 0,
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
                if let Some(n) = pixels(value) {
                    self.scale = ((n + 7) / 8).clamp(1, 4);
                    self.font.size = n.clamp(1, 96) as u16;
                }
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
            "display" => match value {
                "none" => self.hidden = true,
                "block" => self.block = true,
                "inline" => self.block = false,
                _ => {}
            },
            "white-space" => self.pre = value == "pre" || value == "pre-wrap",
            "margin" => {
                if let Some(n) = pixels(value) {
                    self.margin = n;
                }
            }
            "padding" => {
                if let Some(n) = pixels(value) {
                    self.padding = n;
                }
            }
            "width" => self.width = pixels(value),
            "height" => self.height = pixels(value),
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
    ]
    .contains(&node.tag.as_str());
    result.hidden = [
        "head", "title", "style", "script", "link", "meta", "base", "template", "noscript",
    ]
    .contains(&node.tag.as_str());
    if ["p", "h1", "h2", "h3", "ul", "ol", "pre", "blockquote"].contains(&node.tag.as_str()) {
        result.margin = 8;
    }
    if ["h1", "h2"].contains(&node.tag.as_str()) {
        result.scale = (parent.scale * 2).min(4);
        result.font.size = parent.font.size.saturating_mul(2).min(96);
    }
    if ["ul", "ol", "blockquote", "dd"].contains(&node.tag.as_str()) {
        result.padding = 12;
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
        result.apply(key, value);
    }
    result
}

fn priority(value: &str) -> (&str, bool) {
    match value.rsplit_once('!') {
        Some((value, flag)) if flag.trim() == "important" => (value.trim(), true),
        _ => (value, false),
    }
}
