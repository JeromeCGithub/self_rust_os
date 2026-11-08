use crate::*;
use alloc::string::ToString;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    Password,
    Hidden,
    Submit,
}
impl FieldKind {
    pub fn is_text(self) -> bool {
        matches!(self, Self::Text | Self::Password)
    }
}
#[derive(Clone)]
pub struct Field {
    pub node: usize,
    pub name: String,
    pub value: String,
    pub kind: FieldKind,
    pub disabled: bool,
    pub readonly: bool,
    pub label: String,
}
impl Field {
    /// Visible control text; password values never reach the painter.
    pub fn display_value(&self) -> String {
        if self.value.is_empty() {
            self.label.clone()
        } else if self.kind == FieldKind::Password {
            "*".repeat(self.value.chars().count())
        } else {
            self.value.clone()
        }
    }
}
impl core::fmt::Debug for Field {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Field")
            .field("node", &self.node)
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("disabled", &self.disabled)
            .field("readonly", &self.readonly)
            .finish_non_exhaustive()
    }
}
#[derive(Debug, Clone)]
pub struct Form {
    pub node: usize,
    pub action: String,
    pub method: String,
    pub enctype: String,
    pub fields: Vec<Field>,
}
/// A navigation request owns its body independently of the visible URL/history.
/// Debug output deliberately excludes the body (which can contain credentials).
#[derive(Clone, PartialEq, Eq)]
pub enum Submission {
    Get(String),
    Post { url: String, body: String },
}
impl Submission {
    pub fn url(&self) -> &str {
        match self {
            Self::Get(url) | Self::Post { url, .. } => url,
        }
    }
}
impl core::fmt::Debug for Submission {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Get(url) => f.debug_tuple("Get").field(url).finish(),
            Self::Post { url, .. } => f
                .debug_struct("Post")
                .field("url", url)
                .finish_non_exhaustive(),
        }
    }
}
fn encoded(value: &str) -> String {
    use core::fmt::Write;
    let mut result = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"*-._".contains(&byte) {
            result.push(byte as char);
        } else if byte == b' ' {
            result.push('+');
        } else {
            let _ = write!(result, "%{byte:02X}");
        }
    }
    result
}
impl Form {
    /// Serialize successful controls for a GET form. Other methods require submission().
    /// GET replaces the action query, preserving its fragment and relative origin.
    pub fn submit(&self, base: &str, submitter: Option<usize>) -> Result<String, Error> {
        match self.submission(base, submitter)? {
            Submission::Get(url) => Ok(url),
            Submission::Post { .. } => Err(Error::UnsupportedForm),
        }
    }
    /// Bounded UTF-8 application/x-www-form-urlencoded GET or POST submission.
    /// GET replaces the action query; POST retains it and carries a separate body.
    pub fn submission(&self, base: &str, submitter: Option<usize>) -> Result<Submission, Error> {
        let post = self.method.eq_ignore_ascii_case("post");
        if (!post && !self.method.eq_ignore_ascii_case("get"))
            || (post
                && !self
                    .enctype
                    .eq_ignore_ascii_case("application/x-www-form-urlencoded"))
        {
            return Err(Error::UnsupportedForm);
        }
        let action = resolve(base, &self.action)?;
        let mut result = String::new();
        let mut first = true;
        for (index, field) in self.fields.iter().enumerate() {
            if field.disabled
                || field.name.is_empty()
                || (field.kind == FieldKind::Submit && submitter != Some(index))
            {
                continue;
            }
            if field.name.len() > 2048 || field.value.len() > 2048 {
                return Err(Error::Limit);
            }
            if !first {
                result.push('&');
            }
            first = false;
            result.push_str(&encoded(&field.name));
            result.push('=');
            result.push_str(&encoded(&field.value));
            if result.len() > if post { 16 * 1024 } else { 2048 } {
                return Err(Error::Limit);
            }
        }
        if post {
            return Ok(Submission::Post {
                url: action,
                body: result,
            });
        }
        let (action, fragment) = action
            .split_once('#')
            .map(|(a, f)| (a, Some(f)))
            .unwrap_or((&action, None));
        let mut url = action
            .split('?')
            .next()
            .ok_or(Error::InvalidUrl)?
            .to_string();
        url.push('?');
        url.push_str(&result);
        if let Some(fragment) = fragment {
            url.push('#');
            url.push_str(fragment);
        }
        if url.len() > 2048 {
            return Err(Error::Limit);
        }
        Ok(Submission::Get(url))
    }
}
#[derive(Debug)]
pub struct Control {
    pub form: usize,
    pub field: usize,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub font: rustos_text::Style,
    pub color: u32,
    pub background: Option<u32>,
    pub border: u32,
    pub padding: [u32; 4],
}
impl Control {
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x
            && y >= self.y
            && i64::from(x) < i64::from(self.x) + i64::from(self.width)
            && i64::from(y) < i64::from(self.y) + i64::from(self.height)
    }
}
pub(crate) fn collect(doc: &Document) -> Result<Vec<Form>, Error> {
    let mut forms = Vec::new();
    for (id, node) in doc
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.tag == "form")
    {
        if forms.len() == 16 {
            return Err(Error::Limit);
        }
        let action = node.attr("action").unwrap_or("");
        if action.len() > 2048 {
            return Err(Error::Limit);
        }
        forms.push(Form {
            node: id,
            action: action.to_string(),
            method: node.attr("method").unwrap_or("get").to_ascii_lowercase(),
            enctype: node
                .attr("enctype")
                .unwrap_or("application/x-www-form-urlencoded")
                .to_ascii_lowercase(),
            fields: Vec::new(),
        });
    }
    for (id, node) in doc.nodes.iter().enumerate() {
        if node.tag != "input" && node.tag != "button" && node.tag != "textarea" {
            continue;
        }
        let kind = match node
            .attr("type")
            .unwrap_or(if node.tag == "button" {
                "submit"
            } else {
                "text"
            })
            .to_ascii_lowercase()
            .as_str()
        {
            "text" | "search" => FieldKind::Text,
            "password" => FieldKind::Password,
            "hidden" => FieldKind::Hidden,
            "submit" => FieldKind::Submit,
            _ => continue,
        };
        let mut parent = node.parent;
        let owner = loop {
            let Some(id) = parent else {
                break None;
            };
            if doc.nodes[id].tag == "form" {
                break Some(id);
            }
            parent = doc.nodes[id].parent;
        };
        let Some(form) = forms.iter_mut().find(|f| Some(f.node) == owner) else {
            continue;
        };
        if form.fields.len() == 64 {
            return Err(Error::Limit);
        }
        let name = node.attr("name").unwrap_or("");
        let value = node.attr("value").unwrap_or(if node.tag == "textarea" {
            &node.text
        } else {
            ""
        });
        if name.len() > 2048 || value.len() > 2048 {
            return Err(Error::Limit);
        }
        let label = node
            .attr("placeholder")
            .unwrap_or(if kind.is_text() { "" } else { "Submit" });
        if label.len() > 2048 {
            return Err(Error::Limit);
        }
        form.fields.push(Field {
            node: id,
            name: name.to_string(),
            value: value.to_string(),
            kind,
            disabled: node.attr("disabled").is_some(),
            readonly: node.attr("readonly").is_some(),
            label: label.to_string(),
        });
    }
    Ok(forms)
}
