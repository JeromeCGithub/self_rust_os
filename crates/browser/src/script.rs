//! Narrow DOM capability adapter. No script can access sockets, files or syscalls.
use crate::*;
use alloc::string::ToString;
pub use rustos_script::Error as ScriptError;
use rustos_script::{Element, Host, Runtime, Target, Value};
#[derive(Default)]
pub struct ScriptReport {
    pub executed: usize,
    pub failures: Vec<ScriptError>,
}
struct Dom<'a>(&'a mut Document);
impl Dom<'_> {
    fn element(&self, element: &Element) -> Result<usize, ScriptError> {
        // Walk only connected nodes: textContent replacement detaches children.
        let mut todo: Vec<_> = self
            .0
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.parent.is_none())
            .map(|(i, _)| i)
            .collect();
        while let Some(id) = todo.pop() {
            let n = &self.0.nodes[id];
            if match element {
                Element::Body => n.tag == "body",
                Element::Id(value) => n.attr("id") == Some(value.as_str()),
            } {
                return Ok(id);
            }
            todo.extend(n.children.iter().rev().copied());
        }
        Err(ScriptError::MissingElement)
    }
    fn text(&self, id: usize) -> Result<String, ScriptError> {
        let mut result = String::new();
        let mut todo = alloc::vec![id];
        while let Some(id) = todo.pop() {
            let n = &self.0.nodes[id];
            if result.len() + n.text.len() > 8192 {
                return Err(ScriptError::Budget);
            }
            result.push_str(&n.text);
            todo.extend(n.children.iter().rev().copied());
        }
        Ok(result)
    }
}
impl Host for Dom<'_> {
    fn read(&mut self, target: &Target) -> Result<Value, ScriptError> {
        match target {
            Target::Title => Ok(Value::Text(self.0.title.clone())),
            Target::Text(element) => Ok(Value::Text(self.text(self.element(element)?)?)),
            Target::Style(_, _) => Err(ScriptError::Unsupported),
        }
    }
    fn write(&mut self, target: &Target, value: Value) -> Result<(), ScriptError> {
        let value = value.text();
        if value.len() > 8192 {
            return Err(ScriptError::Budget);
        }
        match target {
            Target::Title => self.0.title = value,
            Target::Text(element) => {
                let id = self.element(element)?;
                self.0.nodes[id].text = value;
                self.0.nodes[id].children.clear();
            }
            Target::Style(element, property) => {
                let id = self.element(element)?;
                let (property, value) = match property.as_str() {
                    "color" | "backgroundColor" => {
                        let color = crate::css::color(&value.to_ascii_lowercase())
                            .ok_or(ScriptError::Type)?;
                        (
                            if property == "color" {
                                "color"
                            } else {
                                "background-color"
                            },
                            alloc::format!("#{color:06x}"),
                        )
                    }
                    "display" if ["none", "block", "inline"].contains(&value.as_str()) => {
                        ("display", value)
                    }
                    _ => return Err(ScriptError::Unsupported),
                };
                let node = &mut self.0.nodes[id];
                let declaration = alloc::format!(";{property}:{value}");
                if let Some((_, style)) = node.attributes.iter_mut().find(|(key, _)| key == "style")
                {
                    if style.len() + declaration.len() > 8192 {
                        return Err(ScriptError::Budget);
                    }
                    style.push_str(&declaration);
                } else {
                    if node.attributes.len() >= 64 {
                        return Err(ScriptError::Budget);
                    }
                    node.attributes.push(("style".to_string(), declaration));
                }
            }
        }
        Ok(())
    }
}
impl Document {
    /// Execute a small straight-line classic-script subset after parsing.
    /// Each invocation creates a fresh realm. Unsupported features report errors
    /// and leave completed effects in place; subsequent script elements continue.
    pub fn run_scripts(&mut self) -> ScriptReport {
        let scripts: Vec<_> = self
            .nodes
            .iter()
            .filter(|node| {
                node.tag == "script"
                    && node
                        .attr("type")
                        .map(|t| {
                            t.is_empty()
                                || t.eq_ignore_ascii_case("text/javascript")
                                || t.eq_ignore_ascii_case("application/javascript")
                        })
                        .unwrap_or(true)
            })
            .map(|node| (node.attr("src").is_some(), node.text.clone()))
            .collect();
        let mut runtime = Runtime::new();
        let mut report = ScriptReport::default();
        for (external, source) in scripts {
            if external {
                report.failures.push(ScriptError::Unsupported);
                continue;
            }
            match runtime.execute(&source, &mut Dom(self)) {
                Ok(()) => report.executed += 1,
                Err(error) => report.failures.push(error),
            }
        }
        report
    }
}
