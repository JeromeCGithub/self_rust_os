//! Owned native document scripting. No sockets, files, process APIs or UI handles.
#![no_std]
extern crate alloc;
use alloc::{
    string::{String, ToString},
    vec::Vec,
};
use rustos_browser::page_context::{PageReply, PageRequest};
use rustos_browser::{
    dom::{Change, Changes},
    Document,
};
use rustos_quickjs::{Error, Host, Limits, Reply, Runtime};
/// Execute with a controller-owned page context and monotonic millisecond clock.
pub fn run_page(request: PageRequest<'_>, clock: fn() -> u64) -> Result<PageReply, Error> {
    request.encode().map_err(|_| Error::InvalidInput)?;
    let mut jar = rustos_browser::cookies::CookieJar::default();
    for entry in request.cookie_snapshot.lines() {
        if !jar.set_document(request.url, entry, request.epoch_seconds) {
            return Err(Error::InvalidInput);
        }
    }
    let context = Context {
        url: request.url.to_string(),
        jar,
        cookies: Vec::new(),
        navigation: None,
        clock,
        start: clock(),
        elapsed: 0,
        epoch: request.epoch_seconds,
    };
    execute(request.html, Some(context))
}
struct Context {
    url: String,
    jar: rustos_browser::cookies::CookieJar,
    cookies: Vec<String>,
    navigation: Option<String>,
    clock: fn() -> u64,
    start: u64,
    elapsed: u64,
    epoch: u64,
}
impl Context {
    fn elapsed(&mut self) -> u64 {
        self.elapsed = self.elapsed.max((self.clock)().saturating_sub(self.start));
        self.elapsed
    }
    fn now(&mut self) -> u64 {
        self.epoch.saturating_add(self.elapsed() / 1000)
    }
}
struct Dom {
    document: Document,
    changes: Changes,
    context: Option<Context>,
}
impl Dom {
    fn reply(&self) -> PageReply {
        PageReply {
            dom: Changes {
                failures: self.changes.failures,
                changes: self.changes.changes.clone(),
            },
            cookies: self
                .context
                .as_ref()
                .map(|c| c.cookies.clone())
                .unwrap_or_default(),
            navigation: self.context.as_ref().and_then(|c| c.navigation.clone()),
        }
    }
    fn record(&mut self, change: Change) -> Result<Reply, Error> {
        self.changes.changes.push(change.clone());
        if self.reply().encode().is_err() || self.document.apply_change(&change).is_err() {
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
            11 => Ok(Reply::Text(
                self.context
                    .as_ref()
                    .map(|c| c.url.clone())
                    .unwrap_or_default(),
            )),
            12 => {
                let value = match self.context.as_mut() {
                    Some(c) => {
                        let now = c.now();
                        c.jar.header(&c.url, now)
                    }
                    None => String::new(),
                };
                Ok(Reply::Text(value))
            }
            13 => {
                let Some(c) = self.context.as_mut() else {
                    return Ok(Reply::Unit);
                };
                let mut next = c.jar.clone();
                let now = c.now();
                if !next.set_document(&c.url, first, now) {
                    return Ok(Reply::Unit);
                }
                c.cookies.push(first.to_string());
                if self.reply().encode().is_err() {
                    self.context
                        .as_mut()
                        .ok_or(Error::InvalidInput)?
                        .cookies
                        .pop();
                    return Err(Error::OutputLimit);
                }
                self.context.as_mut().ok_or(Error::InvalidInput)?.jar = next;
                Ok(Reply::Unit)
            }
            14 => {
                let c = self.context.as_mut().ok_or(Error::InvalidInput)?;
                let next =
                    rustos_browser::resolve(&c.url, first).map_err(|_| Error::InvalidInput)?;
                if c.url.starts_with("https://") && !next.starts_with("https://") {
                    return Err(Error::InvalidInput);
                }
                let previous = c.navigation.replace(next);
                if self.reply().encode().is_err() {
                    self.context.as_mut().ok_or(Error::InvalidInput)?.navigation = previous;
                    return Err(Error::OutputLimit);
                }
                Ok(Reply::Unit)
            }
            15 => Ok(Reply::Text(
                self.context
                    .as_mut()
                    .ok_or(Error::InvalidInput)?
                    .elapsed()
                    .to_string(),
            )),
            16 => Ok(Reply::Text(
                self.context
                    .as_ref()
                    .ok_or(Error::InvalidInput)?
                    .epoch
                    .to_string(),
            )),
            _ => Err(Error::InvalidInput),
        }
    }
}
/// One fresh native realm per document; bounded inline classic script order and
/// explicit promise checkpoints. Returns only validated, pointer-free effects.
/// External/module scripts, browser events/timers/network/storage remain unsupported.
pub fn run(html: &str) -> Result<Changes, Error> {
    execute(html, None).map(|reply| reply.dom)
}
fn execute(html: &str, context: Option<Context>) -> Result<PageReply, Error> {
    let document = Document::parse(html).map_err(|_| Error::InvalidInput)?;
    let scripts: Vec<_> = document
        .nodes
        .iter()
        .filter(|n| {
            matches!(
                n.script_kind(),
                Some(rustos_browser::ScriptKind::Classic | rustos_browser::ScriptKind::Module)
            )
        })
        .map(|n| (n.attr("src").is_some(), n.script_kind(), n.text.clone()))
        .collect();
    let host = Dom {
        document,
        changes: Changes::default(),
        context,
    };
    // Page code can contain nested generated expressions. This remains below
    // the already-supported 128 KiB VM ceiling and the 4 MiB user-thread stack;
    // heap, interrupt and job budgets retain their original limits.
    let mut runtime = Runtime::new(Limits {
        stack_bytes: 128 * 1024,
        ..Limits::default()
    })?;
    runtime.enable_date()?;
    let mut runtime = runtime.with_host(host)?;
    runtime.execute(include_str!("bindings.js"))?;
    let mut source_bytes = 0usize;
    let mut executed = 0usize;
    let mut failures = 0u32;
    for (external, kind, source) in scripts {
        executed += 1;
        source_bytes = source_bytes.saturating_add(source.len());
        if external
            || kind == Some(rustos_browser::ScriptKind::Module)
            || source_bytes > 128 * 1024
            || executed > 64
        {
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
        if runtime
            .host()
            .context
            .as_ref()
            .is_some_and(|c| c.navigation.is_some())
        {
            break;
        }
    }
    let mut changes = runtime.host().reply();
    changes.dom.failures = failures;
    // Bound/recheck the complete reply, independent of individual setter checks.
    let _ = changes.encode().map_err(|_| Error::OutputLimit)?;
    Ok(changes)
}
