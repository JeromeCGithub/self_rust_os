//! Pointer-free document effects exchanged with the restricted script worker.
use crate::*;
use alloc::string::ToString;
pub const MAX_CHANGES: usize = 256;
pub const MAX_CHANGE_BYTES: usize = 8192;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    Title(String),
    Text {
        node: u32,
        value: String,
    },
    Style {
        node: u32,
        property: String,
        value: String,
    },
}
#[derive(Default, Debug, PartialEq, Eq)]
pub struct Changes {
    pub failures: u32,
    pub changes: Vec<Change>,
}
impl Changes {
    /// Serialize bounded UTF-8 effects. No native pointer/handle enters the wire.
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        if self.changes.len() > MAX_CHANGES {
            return Err(Error::Limit);
        }
        let mut bytes = Vec::from(&b"JDM1"[..]);
        bytes.extend_from_slice(&self.failures.to_le_bytes());
        bytes.extend_from_slice(&(self.changes.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&[0; 2]);
        for change in &self.changes {
            let (kind, node, property, value) = match change {
                Change::Title(value) => (0, 0, "", value),
                Change::Text { node, value } => (1, *node, "", value),
                Change::Style {
                    node,
                    property,
                    value,
                } => (2, *node, property.as_str(), value),
            };
            if property.len() > 128 || value.len() > MAX_CHANGE_BYTES {
                return Err(Error::Limit);
            }
            let length = 9 + property.len() + value.len();
            if length > MAX_CHANGE_BYTES.saturating_sub(bytes.len()) {
                return Err(Error::Limit);
            }
            bytes.push(kind);
            bytes.extend_from_slice(&node.to_le_bytes());
            bytes.extend_from_slice(&(property.len() as u16).to_le_bytes());
            bytes.extend_from_slice(&(value.len() as u16).to_le_bytes());
            bytes.extend_from_slice(property.as_bytes());
            bytes.extend_from_slice(value.as_bytes());
        }
        Ok(bytes)
    }
    /// Reject truncation, reserved fields, unknown kinds, invalid UTF-8 and tails.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 12
            || bytes.len() > MAX_CHANGE_BYTES
            || &bytes[..4] != b"JDM1"
            || bytes[10..12] != [0; 2]
        {
            return Err(Error::Limit);
        }
        let failures = u32::from_le_bytes(bytes[4..8].try_into().map_err(|_| Error::Limit)?);
        let count = u16::from_le_bytes(bytes[8..10].try_into().map_err(|_| Error::Limit)?) as usize;
        if count > MAX_CHANGES {
            return Err(Error::Limit);
        }
        let mut changes = Vec::new();
        let mut at = 12;
        for _ in 0..count {
            let header = bytes.get(at..at + 9).ok_or(Error::Limit)?;
            let node = u32::from_le_bytes(header[1..5].try_into().map_err(|_| Error::Limit)?);
            let property = u16::from_le_bytes([header[5], header[6]]) as usize;
            let length = u16::from_le_bytes([header[7], header[8]]) as usize;
            at += 9;
            if property > 128 {
                return Err(Error::Limit);
            }
            let key = core::str::from_utf8(bytes.get(at..at + property).ok_or(Error::Limit)?)
                .map_err(|_| Error::Limit)?;
            at += property;
            let value = core::str::from_utf8(bytes.get(at..at + length).ok_or(Error::Limit)?)
                .map_err(|_| Error::Limit)?
                .to_string();
            at += length;
            changes.push(match header[0] {
                0 if node == 0 && property == 0 => Change::Title(value),
                1 if property == 0 => Change::Text { node, value },
                2 if property > 0 => Change::Style {
                    node,
                    property: key.to_string(),
                    value,
                },
                _ => return Err(Error::Limit),
            });
        }
        if at != bytes.len() {
            return Err(Error::Limit);
        }
        Ok(Self { failures, changes })
    }
}
impl Document {
    /// Validate all effects on a bounded copy before publishing any page mutation.
    pub fn apply_changes(&mut self, changes: &Changes) -> Result<(), Error> {
        changes.encode()?;
        let mut next = self.clone();
        for change in &changes.changes {
            next.apply_change(change)?;
        }
        *self = next;
        Ok(())
    }
    /// Apply one already-owned bounded effect, with atomic validation on failure.
    pub fn apply_change(&mut self, change: &Change) -> Result<(), Error> {
        match change {
            Change::Title(value) => {
                if value.len() > MAX_CHANGE_BYTES {
                    return Err(Error::Limit);
                }
                self.title = value.clone();
            }
            Change::Text { node, value } => {
                if value.len() > MAX_CHANGE_BYTES {
                    return Err(Error::Limit);
                }
                let node = self.nodes.get_mut(*node as usize).ok_or(Error::Limit)?;
                node.text = value.clone();
                // Detached arena entries keep their IDs/lifetimes but are no
                // longer reachable through this node or rendered as new roots.
                node.children.clear();
            }
            Change::Style {
                node,
                property,
                value,
            } => {
                if ![
                    "color",
                    "background-color",
                    "display",
                    "font-size",
                    "font-family",
                    "font-weight",
                    "width",
                    "height",
                ]
                .contains(&property.as_str())
                    || value.len() > 128
                    || value.contains([';', '{', '}'])
                {
                    return Err(Error::Limit);
                }
                let node = self.nodes.get_mut(*node as usize).ok_or(Error::Limit)?;
                let index = node.attributes.iter().position(|(key, _)| key == "style");
                if index.is_none() && node.attributes.len() >= 64 {
                    return Err(Error::Limit);
                }
                let mut declarations = index
                    .map(|i| css::declarations(&node.attributes[i].1))
                    .unwrap_or_default();
                declarations.retain(|(key, _)| key != property);
                declarations.push((property.clone(), value.clone()));
                let mut style = String::new();
                for (key, value) in declarations {
                    style.push_str(&key);
                    style.push(':');
                    style.push_str(&value);
                    style.push(';');
                }
                if style.len() > MAX_CHANGE_BYTES {
                    return Err(Error::Limit);
                }
                if let Some(i) = index {
                    node.attributes[i].1 = style;
                } else {
                    node.attributes.push(("style".to_string(), style));
                }
            }
        }
        Ok(())
    }
    /// Stable IDs of currently connected nodes, in document tree order.
    pub fn connected_nodes(&self) -> Vec<usize> {
        let mut todo: Vec<_> = self
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.parent.is_none())
            .map(|(id, _)| id)
            .rev()
            .collect();
        let mut result = Vec::new();
        while let Some(id) = todo.pop() {
            result.push(id);
            todo.extend(self.nodes[id].children.iter().rev().copied());
        }
        result
    }
    /// Bounded textContent of a live arena entry, including its connected subtree.
    pub fn text_content(&self, node: usize) -> Result<String, Error> {
        if node >= self.nodes.len() {
            return Err(Error::Limit);
        }
        let mut todo = alloc::vec![node];
        let mut text = String::new();
        while let Some(id) = todo.pop() {
            let node = self.nodes.get(id).ok_or(Error::Limit)?;
            if node.text.len() > MAX_CHANGE_BYTES.saturating_sub(text.len()) {
                return Err(Error::Limit);
            }
            text.push_str(&node.text);
            todo.extend(node.children.iter().rev().copied());
        }
        Ok(text)
    }
}
