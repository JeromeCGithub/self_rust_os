use crate::*;
use alloc::{string::ToString, vec};

pub(crate) fn entities(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let end = rest.find(';').filter(|n| *n <= 16);
        let value = end.and_then(|end| match &rest[1..end] {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            number => number
                .strip_prefix("#x")
                .or_else(|| number.strip_prefix("#X"))
                .and_then(|n| u32::from_str_radix(n, 16).ok())
                .or_else(|| number.strip_prefix('#').and_then(|n| n.parse().ok()))
                .and_then(char::from_u32),
        });
        if let (Some(end), Some(ch)) = (end, value) {
            out.push(ch);
            rest = &rest[end + 1..];
        } else {
            out.push('&');
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    out
}

fn attributes(mut rest: &str) -> Result<Vec<(String, String)>, Error> {
    let mut result = Vec::new();
    while !rest.trim_start().is_empty() {
        rest = rest.trim_start();
        let end = rest
            .find(|c: char| c.is_ascii_whitespace() || c == '=')
            .unwrap_or(rest.len());
        if end == 0 {
            rest = &rest[1..];
            continue;
        }
        let name = rest[..end].to_ascii_lowercase();
        rest = rest[end..].trim_start();
        let value = if let Some(value) = rest.strip_prefix('=') {
            rest = value.trim_start();
            if rest.starts_with(['\'', '"']) {
                let quote = rest.as_bytes()[0] as char;
                rest = &rest[1..];
                let end = rest.find(quote).unwrap_or(rest.len());
                let value = entities(&rest[..end]);
                rest = &rest[(end + 1).min(rest.len())..];
                value
            } else {
                let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
                let value = entities(&rest[..end]);
                rest = &rest[end..];
                value
            }
        } else {
            String::new()
        };
        if result.len() == 64 {
            return Err(Error::Limit);
        }
        // HTML keeps the first duplicate attribute.
        if !result.iter().any(|(key, _)| key == &name) {
            result.push((name, value));
        }
    }
    Ok(result)
}

fn append(doc: &mut Document, stack: &[usize], node: Node) -> Result<usize, Error> {
    if doc.nodes.len() == MAX_NODES {
        return Err(Error::Limit);
    }
    let index = doc.nodes.len();
    if let Some(&parent) = stack.last() {
        doc.nodes[parent].children.push(index);
    }
    doc.nodes.push(node);
    Ok(index)
}

pub(crate) fn parse(source: &str) -> Result<Document, Error> {
    if source.len() > MAX_HTML {
        return Err(Error::Limit);
    }
    let mut doc = Document {
        title: String::new(),
        nodes: Vec::new(),
        stylesheets: Vec::new(),
        rules: Vec::new(),
        css_bytes: 0,
        forms: Vec::new(),
    };
    let mut stack: Vec<usize> = vec![];
    let mut rest = source;
    while !rest.is_empty() {
        if rest.starts_with("<!--") {
            rest = rest.find("-->").map(|end| &rest[end + 3..]).unwrap_or("");
            continue;
        }
        if rest.starts_with('<') {
            let mut quote = None;
            let end = rest.char_indices().skip(1).find_map(|(i, ch)| {
                if Some(ch) == quote {
                    quote = None;
                } else if quote.is_none() {
                    if ch == '\'' || ch == '"' {
                        quote = Some(ch);
                    } else if ch == '>' {
                        return Some(i);
                    }
                }
                None
            });
            if let Some(end) = end {
                let token = rest[1..end].trim();
                rest = &rest[end + 1..];
                if token.starts_with(['!', '?']) {
                    continue;
                }
                if let Some(close) = token.strip_prefix('/') {
                    let tag = close.trim().to_ascii_lowercase();
                    if let Some(at) = stack.iter().rposition(|&id| doc.nodes[id].tag == tag) {
                        stack.truncate(at);
                    }
                    continue;
                }
                let name_end = token
                    .find(|c: char| c.is_ascii_whitespace() || c == '/')
                    .unwrap_or(token.len());
                if name_end == 0 {
                    continue;
                }
                let tag = token[..name_end].to_ascii_lowercase();
                if !tag.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
                    continue;
                }
                if tag == "p" && stack.last().is_some_and(|&id| doc.nodes[id].tag == "p") {
                    stack.pop();
                }
                // li and dt/dd end tags are optional within their list scope.
                let scope = match tag.as_str() {
                    "li" => Some(("ul", "ol")),
                    "dt" | "dd" => Some(("dl", "dl")),
                    _ => None,
                };
                if let Some((a, b)) = scope {
                    for at in (0..stack.len()).rev() {
                        let open = doc.nodes[stack[at]].tag.as_str();
                        if open == a || open == b {
                            break;
                        }
                        if (tag == "li" && open == "li")
                            || (tag != "li" && (open == "dt" || open == "dd"))
                        {
                            stack.truncate(at);
                            break;
                        }
                    }
                }
                let node = Node {
                    tag: tag.clone(),
                    text: String::new(),
                    attributes: attributes(token[name_end..].trim_end_matches('/'))?,
                    parent: stack.last().copied(),
                    children: vec![],
                };
                if tag == "link"
                    && node
                        .attr("rel")
                        .is_some_and(|r| r.eq_ignore_ascii_case("stylesheet"))
                {
                    if let Some(href) = node.attr("href") {
                        if doc.stylesheets.len() == 4 {
                            return Err(Error::Limit);
                        }
                        doc.stylesheets.push(href.to_string());
                    }
                }
                let id = append(&mut doc, &stack, node)?;
                if ["script", "style", "title", "textarea"].contains(&tag.as_str()) {
                    let lower = rest.to_ascii_lowercase();
                    let marker = alloc::format!("</{tag}");
                    let end = lower.find(&marker).unwrap_or(rest.len());
                    let raw = &rest[..end];
                    if tag == "textarea" {
                        doc.nodes[id].text = entities(raw);
                    }
                    #[cfg(feature = "javascript")]
                    if tag == "script" {
                        doc.nodes[id].text = raw.to_string();
                    }
                    if tag == "style" {
                        doc.add_css(raw)?;
                    }
                    if tag == "title" {
                        doc.title = entities(raw)
                            .split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" ");
                    }
                    rest = rest[end..]
                        .find('>')
                        .map(|n| &rest[end + n + 1..])
                        .unwrap_or("");
                } else if ![
                    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta",
                    "param", "source", "track", "wbr",
                ]
                .contains(&tag.as_str())
                    && !token.ends_with('/')
                {
                    if stack.len() == MAX_DEPTH {
                        return Err(Error::Limit);
                    }
                    stack.push(id);
                }
                continue;
            }
        }
        // Always consume at least one Unicode character on malformed '<'.
        let first = rest.chars().next().map(char::len_utf8).unwrap_or(0);
        let end = rest[first..]
            .find('<')
            .map(|n| n + first)
            .unwrap_or(rest.len());
        append(
            &mut doc,
            &stack,
            Node {
                tag: String::new(),
                text: entities(&rest[..end]),
                attributes: vec![],
                parent: stack.last().copied(),
                children: vec![],
            },
        )?;
        rest = &rest[end..];
    }
    // HTML permits omitted body tags. Preserve explicit bodies, otherwise wrap
    // visible roots (or children of an explicit html element) in an implied body.
    if !doc.nodes.iter().any(|node| node.tag == "body") {
        let html = doc.nodes.iter().position(|node| node.tag == "html");
        let children: Vec<usize> = doc
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| node.parent == html && node.tag != "html")
            .map(|(id, _)| id)
            .collect();
        if doc.nodes.len() == MAX_NODES {
            return Err(Error::Limit);
        }
        let body = doc.nodes.len();
        for &id in &children {
            doc.nodes[id].parent = Some(body);
        }
        if let Some(html) = html {
            doc.nodes[html].children = vec![body];
        }
        doc.nodes.push(Node {
            tag: "body".into(),
            text: String::new(),
            attributes: vec![],
            parent: html,
            children,
        });
    }
    doc.forms = crate::forms::collect(&doc)?;
    Ok(doc)
}
