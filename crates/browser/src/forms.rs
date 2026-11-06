use crate::*;
use alloc::string::ToString;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    Hidden,
    Submit,
}
#[derive(Debug, Clone)]
pub struct Field {
    pub node: usize,
    pub name: String,
    pub value: String,
    pub kind: FieldKind,
    pub disabled: bool,
    pub readonly: bool,
    pub label: String,
}
#[derive(Debug, Clone)]
pub struct Form {
    pub node: usize,
    pub action: String,
    pub method: String,
    pub fields: Vec<Field>,
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
    /// Serialize successful controls for a GET form. POST is explicitly unsupported.
    /// GET replaces the action query, preserving its fragment and relative origin.
    pub fn submit(&self, base: &str, submitter: Option<usize>) -> Result<String, Error> {
        if !self.method.eq_ignore_ascii_case("get") {
            return Err(Error::UnsupportedForm);
        }
        let action = resolve(base, &self.action)?;
        let (action, fragment) = action
            .split_once('#')
            .map(|(a, f)| (a, Some(f)))
            .unwrap_or((&action, None));
        let mut result = action
            .split('?')
            .next()
            .ok_or(Error::InvalidUrl)?
            .to_string();
        result.push('?');
        let mut first = true;
        for (index, field) in self.fields.iter().enumerate() {
            if field.disabled
                || field.name.is_empty()
                || (field.kind == FieldKind::Submit && submitter != Some(index))
            {
                continue;
            }
            if !first {
                result.push('&');
            }
            first = false;
            result.push_str(&encoded(&field.name));
            result.push('=');
            result.push_str(&encoded(&field.value));
            if result.len() > 2048 {
                return Err(Error::Limit);
            }
        }
        if let Some(fragment) = fragment {
            result.push('#');
            result.push_str(fragment);
        }
        if result.len() > 2048 {
            return Err(Error::Limit);
        }
        Ok(result)
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
            .or_else(|| node.attr("title"))
            .or_else(|| node.attr("aria-label"))
            .unwrap_or(if kind == FieldKind::Text {
                "Text"
            } else {
                "Submit"
            });
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
