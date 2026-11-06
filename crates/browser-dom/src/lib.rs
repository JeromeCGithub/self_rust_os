//! Owned native document scripting. No sockets, files, process APIs or UI handles.
#![no_std]
extern crate alloc;
use alloc::{string::ToString, vec::Vec};
use rustos_browser::{
    dom::{Change, Changes},
    Document,
};
use rustos_quickjs::{Error, Host, Limits, Reply, Runtime};
struct Dom {
    document: Document,
    changes: Changes,
}
impl Dom {
    fn record(&mut self, change: Change) -> Result<Reply, Error> {
        self.changes.changes.push(change.clone());
        if self.changes.encode().is_err() || self.document.apply_change(&change).is_err() {
            self.changes.changes.pop();
            return Err(Error::OutputLimit);
        }
        Ok(Reply::Unit)
    }
}
impl Host for Dom {
    fn call(
        &mut self,
        operation: u32,
        node: u32,
        first: &str,
        second: &str,
    ) -> Result<Reply, Error> {
        match operation {
            0 | 1 => {
                let id = self.document.connected_nodes().into_iter().find(|&id| {
                    let n = &self.document.nodes[id];
                    if operation == 0 {
                        n.attr("id") == Some(first)
                    } else {
                        n.tag == "body"
                    }
                });
                Ok(id.map(|id| Reply::Node(id as u32)).unwrap_or(Reply::Null))
            }
            2 => Ok(Reply::Text(self.document.title.clone())),
            3 => self.record(Change::Title(first.to_string())),
            4 => Ok(Reply::Text(
                self.document
                    .text_content(node as usize)
                    .map_err(|_| Error::InvalidInput)?,
            )),
            5 => self.record(Change::Text {
                node,
                value: first.to_string(),
            }),
            6 => {
                let node = self
                    .document
                    .nodes
                    .get(node as usize)
                    .ok_or(Error::InvalidInput)?;
                let value = node
                    .attr("style")
                    .unwrap_or("")
                    .split(';')
                    .rev()
                    .filter_map(|part| part.split_once(':'))
                    .find(|(key, _)| key.trim().eq_ignore_ascii_case(first))
                    .map(|(_, value)| value.trim())
                    .unwrap_or("");
                Ok(Reply::Text(value.to_string()))
            }
            7 => self.record(Change::Style {
                node,
                property: first.to_string(),
                value: second.to_string(),
            }),
            8 => {
                let node = self
                    .document
                    .nodes
                    .get(node as usize)
                    .ok_or(Error::InvalidInput)?;
                Ok(node
                    .attr(first)
                    .map(|v| Reply::Text(v.to_string()))
                    .unwrap_or(Reply::Null))
            }
            9 | 10 => self.record(Change::Attribute {
                node,
                name: first.to_string(),
                value: if operation == 9 {
                    Some(second.to_string())
                } else {
                    None
                },
            }),
            _ => Err(Error::InvalidInput),
        }
    }
}
/// One fresh native realm per document; bounded inline classic script order and
/// explicit promise checkpoints. Returns only validated, pointer-free effects.
/// External/module scripts, browser events/timers/network/storage remain unsupported.
pub fn run(html: &str) -> Result<Changes, Error> {
    let document = Document::parse(html).map_err(|_| Error::InvalidInput)?;
    let scripts: Vec<_> = document
        .nodes
        .iter()
        .filter(|n| n.tag == "script")
        .map(|n| {
            (
                n.attr("src").is_some(),
                n.attr("type").unwrap_or("").to_ascii_lowercase(),
                n.text.clone(),
            )
        })
        .collect();
    let host = Dom {
        document,
        changes: Changes::default(),
    };
    let mut runtime = Runtime::new(Limits::default())?.with_host(host)?;
    runtime.execute(include_str!("bindings.js"))?;
    let mut source_bytes = 0usize;
    let mut executed = 0usize;
    let mut failures = 0u32;
    for (external, kind, source) in scripts {
        if !["", "text/javascript", "application/javascript", "module"].contains(&kind.as_str()) {
            continue;
        }
        executed += 1;
        source_bytes = source_bytes.saturating_add(source.len());
        if external || kind == "module" || source_bytes > 128 * 1024 || executed > 64 {
            failures = failures.saturating_add(1);
            continue;
        }
        let result = runtime.execute(&source);
        let jobs = runtime.run_jobs(128);
        if result.is_err() || jobs.is_err() {
            failures = failures.saturating_add(1);
        }
        // A non-quiescent or exhausted checkpoint destroys the entire queue with
        // this realm; no deferred native references can outlive the document.
        if matches!(jobs, Err(Error::Interrupted | Error::Memory)) {
            break;
        }
    }
    let changes = Changes {
        failures,
        changes: runtime.host().changes.changes.clone(),
    };
    // Bound/recheck the complete reply, independent of individual setter checks.
    let _ = changes.encode().map_err(|_| Error::OutputLimit)?;
    Ok(changes)
}
