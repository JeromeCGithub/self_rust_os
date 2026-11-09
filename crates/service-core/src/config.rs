//! Bounded native v1 unit definitions. Parsing performs no process operations.
use crate::Policy;
use alloc::{string::String, vec::Vec};
/// Maximum catalog size; runtime still reserves separate process headroom.
pub const MAX_UNITS: usize = 64;
/// Maximum activation/ordering edges across a catalog.
pub const MAX_EDGES: usize = 256;
/// Per-file and whole-catalog input budgets, in UTF-8 bytes.
pub const MAX_FILE_BYTES: usize = 8192;
pub const MAX_CATALOG_BYTES: usize = 128 * 1024;
/// Explicit launch-vector budgets, below the native kernel's ABI limits.
pub const MAX_ARGS: usize = 32;
pub const MAX_ENV: usize = 32;
pub const MAX_LAUNCH_BYTES: usize = 4096;
/// Services own processes; targets only group activation jobs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Service,
    Target,
}
/// Successful spawn or explicit authenticated readiness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Readiness {
    Spawned,
    Notify,
}
/// Immutable parsed definition. Names resolve only after the entire catalog loads.
#[derive(Debug)]
pub struct Unit {
    pub name: String,
    pub kind: Kind,
    pub executable: Option<String>,
    pub arguments: Vec<String>,
    pub environment: Vec<String>,
    pub requires: Vec<String>,
    pub wants: Vec<String>,
    pub after: Vec<String>,
    pub policy: Policy,
    pub readiness: Readiness,
    pub start_timeout: u64,
    pub watchdog: Option<u64>,
    pub control: Option<rustos_service_abi::control::Access>,
}
/// Parse errors retain the source line without allocating while reporting failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Error {
    pub line: usize,
    pub kind: ErrorKind,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    Encoding,
    Limit,
    Syntax,
    UnknownKey,
    Duplicate,
    InvalidValue,
    MissingField,
    Unsupported,
    Allocation,
}
/// Parse one complete unit before catalog admission. No shell expansion is performed.
pub fn parse(bytes: &[u8]) -> Result<Unit, Error> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err(error(1, ErrorKind::Limit));
    }
    let text = core::str::from_utf8(bytes).map_err(|e| {
        error(
            1 + bytes[..e.valid_up_to()]
                .iter()
                .filter(|&&b| b == b'\n')
                .count(),
            ErrorKind::Encoding,
        )
    })?;
    let mut unit = Unit {
        name: String::new(),
        kind: Kind::Service,
        executable: None,
        arguments: Vec::new(),
        environment: Vec::new(),
        requires: Vec::new(),
        wants: Vec::new(),
        after: Vec::new(),
        readiness: Readiness::Spawned,
        start_timeout: 90,
        watchdog: None,
        control: None,
        policy: Policy {
            restart: crate::Restart::Never,
            delay: 18,
            maximum_delay: 180,
            burst: 3,
            window: 1800,
            healthy_reset: 1800,
        },
    };
    let mut seen = 0u32;
    let mut policy_line = 1;
    let mut watchdog_line = 1;
    const KEYS: [&str; 20] = [
        "Version",
        "Kind",
        "Name",
        "Exec",
        "Arg",
        "Env",
        "WorkingDirectory",
        "Ready",
        "Restart",
        "RestartDelayTicks",
        "RestartMaxDelayTicks",
        "RestartBurst",
        "RestartWindowTicks",
        "HealthyResetTicks",
        "Requires",
        "Wants",
        "After",
        "StartTimeoutTicks",
        "WatchdogTicks",
        "ControlAccess",
    ];
    let mut launch_bytes = 0usize;
    for (index, raw) in text.split('\n').enumerate() {
        let line = index + 1;
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        if raw.chars().any(|c| c.is_control() && c != '\t') {
            return Err(error(line, ErrorKind::Encoding));
        }
        let raw = trim(raw);
        if raw.is_empty() || raw.starts_with('#') {
            continue;
        }
        let (key, value) = raw.split_once('=').ok_or(error(line, ErrorKind::Syntax))?;
        let key = trim(key);
        let value = trim(value);
        let id = KEYS
            .iter()
            .position(|&k| k == key)
            .ok_or(error(line, ErrorKind::UnknownKey))?;
        let repeated = matches!(id, 4 | 5 | 14..=16);
        if !repeated && seen & (1 << id) != 0 {
            return Err(error(line, ErrorKind::Duplicate));
        }
        seen |= 1 << id;
        if matches!(id, 3..=5) {
            launch_bytes = launch_bytes
                .checked_add(value.len() + 1)
                .filter(|&n| n <= MAX_LAUNCH_BYTES)
                .ok_or(error(line, ErrorKind::Limit))?;
        }
        match id {
            0 => {
                if value != "1" {
                    return Err(error(line, ErrorKind::Unsupported));
                }
            }
            1 => {
                unit.kind = match value {
                    "service" => Kind::Service,
                    "target" => Kind::Target,
                    _ => return Err(error(line, ErrorKind::InvalidValue)),
                }
            }
            2 => {
                check_name(value, line)?;
                unit.name = owned(value, line)?;
            }
            3 => {
                if !absolute_file(value) {
                    return Err(error(line, ErrorKind::InvalidValue));
                }
                unit.executable = Some(owned(value, line)?);
            }
            4 => append(&mut unit.arguments, value, MAX_ARGS, line)?,
            5 => {
                let (name, _) = value
                    .split_once('=')
                    .ok_or(error(line, ErrorKind::InvalidValue))?;
                if !environment_name(name) {
                    return Err(error(line, ErrorKind::InvalidValue));
                }
                if unit
                    .environment
                    .iter()
                    .any(|e| e.split_once('=').map(|(k, _)| k) == Some(name))
                {
                    return Err(error(line, ErrorKind::Duplicate));
                }
                append(&mut unit.environment, value, MAX_ENV, line)?;
            }
            6 => {
                if value != "/" {
                    return Err(error(line, ErrorKind::Unsupported));
                }
            }
            7 => {
                unit.readiness = match value {
                    "spawned" => Readiness::Spawned,
                    "notify" => Readiness::Notify,
                    _ => return Err(error(line, ErrorKind::Unsupported)),
                };
            }
            8 => {
                unit.policy.restart = match value {
                    "never" => crate::Restart::Never,
                    "on-failure" => crate::Restart::OnFailure,
                    "always" => crate::Restart::Always,
                    _ => return Err(error(line, ErrorKind::InvalidValue)),
                }
            }
            9..=13 => {
                policy_line = line;
                let number = value
                    .parse::<u64>()
                    .ok()
                    .filter(|&n| n != 0 && n <= i64::MAX as u64)
                    .ok_or(error(line, ErrorKind::InvalidValue))?;
                match id {
                    9 => unit.policy.delay = number,
                    10 => unit.policy.maximum_delay = number,
                    11 => {
                        if number > crate::MAX_BURST as u64 {
                            return Err(error(line, ErrorKind::InvalidValue));
                        }
                        unit.policy.burst = number as u16;
                    }
                    12 => unit.policy.window = number,
                    _ => unit.policy.healthy_reset = number,
                }
            }
            14..=16 => {
                check_name(value, line)?;
                if unit.requires.len() + unit.wants.len() + unit.after.len() >= MAX_EDGES {
                    return Err(error(line, ErrorKind::Limit));
                }
                let edges = match id {
                    14 => &mut unit.requires,
                    15 => &mut unit.wants,
                    _ => &mut unit.after,
                };
                if edges.iter().any(|name| name == value) {
                    return Err(error(line, ErrorKind::Duplicate));
                }
                append(edges, value, MAX_EDGES, line)?;
            }
            19 => {
                unit.control = Some(match value {
                    "status" => rustos_service_abi::control::Access::ReadOnly,
                    "manage" => rustos_service_abi::control::Access::Manage,
                    _ => return Err(error(line, ErrorKind::InvalidValue)),
                });
            }
            17 | 18 => {
                let ticks = value
                    .parse::<u64>()
                    .ok()
                    .filter(|&n| n != 0 && n <= i64::MAX as u64)
                    .ok_or(error(line, ErrorKind::InvalidValue))?;
                if id == 17 {
                    unit.start_timeout = ticks;
                } else {
                    unit.watchdog = Some(ticks);
                    watchdog_line = line;
                }
            }
            _ => return Err(error(line, ErrorKind::UnknownKey)),
        }
    }
    if seen & 7 != 7 || unit.kind == Kind::Service && unit.executable.is_none() {
        return Err(error(1, ErrorKind::MissingField));
    }
    // Target definitions have no executable, launch vectors or runtime policy.
    let service_fields = (1 << 19) | (((1u32 << 14) - 1) & !7) | (3u32 << 17);
    if unit.kind == Kind::Target && seen & service_fields != 0 {
        return Err(error(1, ErrorKind::InvalidValue));
    }
    if unit.watchdog.is_some() && unit.readiness != Readiness::Notify {
        return Err(error(watchdog_line, ErrorKind::InvalidValue));
    }
    crate::Supervisor::<1>::new([unit.policy])
        .map_err(|_| error(policy_line, ErrorKind::InvalidValue))?;
    Ok(unit)
}
fn error(line: usize, kind: ErrorKind) -> Error {
    Error { line, kind }
}
fn trim(value: &str) -> &str {
    value.trim_matches(|c: char| c.is_ascii_whitespace())
}
fn owned(value: &str, line: usize) -> Result<String, Error> {
    let mut text = String::new();
    text.try_reserve_exact(value.len())
        .map_err(|_| error(line, ErrorKind::Allocation))?;
    text.push_str(value);
    Ok(text)
}
fn append(values: &mut Vec<String>, value: &str, maximum: usize, line: usize) -> Result<(), Error> {
    if values.len() >= maximum {
        return Err(error(line, ErrorKind::Limit));
    }
    values
        .try_reserve(1)
        .map_err(|_| error(line, ErrorKind::Allocation))?;
    values.push(owned(value, line)?);
    Ok(())
}
fn check_name(name: &str, line: usize) -> Result<(), Error> {
    let first = name.as_bytes().first().copied();
    if name.len() > 48
        || !first.is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_.".contains(&b))
    {
        return Err(error(line, ErrorKind::InvalidValue));
    }
    Ok(())
}
fn environment_name(name: &str) -> bool {
    name.as_bytes()
        .first()
        .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_')
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
fn absolute_file(path: &str) -> bool {
    path.starts_with('/')
        && path.len() > 1
        && path[1..]
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}
impl Error {
    /// Borrow the offending key for a file/line/key diagnostic without allocating.
    pub fn key<'a>(&self, source: &'a str) -> Option<&'a str> {
        source
            .split('\n')
            .nth(self.line.checked_sub(1)?)?
            .split_once('=')
            .map(|(k, _)| trim(k))
    }
}
