//! Immutable catalog validation and activation/order closure, with no runtime I/O.
use crate::config::Unit;
use alloc::{string::String, vec::Vec};
/// Resolved unit membership uses catalog indices, independent of source file order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Edges {
    pub requires: u64,
    pub wants: u64,
    pub after: u64,
}
#[derive(Debug)]
pub struct Catalog {
    units: Vec<Unit>,
    edges: Vec<Edges>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub source: usize,
    pub kind: ErrorKind,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    Parse(crate::config::Error),
    Limit,
    Allocation,
    InvalidSource,
    DuplicateSource,
    DuplicateName,
    Missing(String),
    Cycle(Vec<String>),
    UnknownTarget,
    Invariant,
}
/// Full pull-in closure and deterministic prerequisite-first order.
#[derive(Debug)]
pub struct Plan {
    pub selected: u64,
    pub order: Vec<usize>,
}
impl Catalog {
    /// Validate all files and references before publishing any catalog.
    pub fn load(sources: &[(&str, &[u8])]) -> Result<Self, Error> {
        use crate::config::{self, MAX_CATALOG_BYTES, MAX_EDGES, MAX_UNITS};
        if sources.is_empty() || sources.len() > MAX_UNITS {
            return Err(error(0, ErrorKind::Limit));
        }
        let mut bytes = 0usize;
        for (source, (_, data)) in sources.iter().enumerate() {
            bytes = bytes
                .checked_add(data.len())
                .filter(|&n| n <= MAX_CATALOG_BYTES)
                .ok_or(error(source, ErrorKind::Limit))?;
        }
        let mut parsed = Vec::new();
        reserve(&mut parsed, sources.len(), 0)?;
        for (source, (name, data)) in sources.iter().enumerate() {
            if name.is_empty()
                || name.len() > 255
                || name.chars().any(|c| c.is_control())
                || name.contains('/')
                || name.contains('\\')
                || !name
                    .rsplit_once('.')
                    .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("unit"))
            {
                return Err(error(source, ErrorKind::InvalidSource));
            }
            if sources[..source]
                .iter()
                .any(|(other, _)| name.eq_ignore_ascii_case(other))
            {
                return Err(error(source, ErrorKind::DuplicateSource));
            }
            let unit = config::parse(data).map_err(|e| error(source, ErrorKind::Parse(e)))?;
            if parsed
                .iter()
                .any(|(other, _): &(Unit, usize)| other.name == unit.name)
            {
                return Err(error(source, ErrorKind::DuplicateName));
            }
            parsed.push((unit, source));
        }
        parsed.sort_unstable_by(|(a, _), (b, _)| a.name.cmp(&b.name));
        let mut source_ids = Vec::new();
        reserve(&mut source_ids, parsed.len(), 0)?;
        let mut units = Vec::new();
        reserve(&mut units, parsed.len(), 0)?;
        for (unit, source) in parsed {
            units.push(unit);
            source_ids.push(source);
        }
        let total_edges = units
            .iter()
            .map(|u| u.requires.len() + u.wants.len() + u.after.len())
            .sum::<usize>();
        if total_edges > MAX_EDGES {
            return Err(error(0, ErrorKind::Limit));
        }
        let mut edges = Vec::new();
        reserve(&mut edges, units.len(), 0)?;
        for (id, unit) in units.iter().enumerate() {
            let source = source_ids[id];
            let mut row = Edges::default();
            for (names, mask) in [
                (&unit.requires, &mut row.requires),
                (&unit.wants, &mut row.wants),
                (&unit.after, &mut row.after),
            ] {
                for name in names {
                    let Some(target) = units.iter().position(|u| &u.name == name) else {
                        return Err(error(source, ErrorKind::Missing(owned(name, source)?)));
                    };
                    if target == id {
                        let mut path = Vec::new();
                        reserve(&mut path, 2, source)?;
                        path.push(owned(name, source)?);
                        path.push(owned(name, source)?);
                        return Err(error(source, ErrorKind::Cycle(path)));
                    }
                    *mask |= 1u64 << target;
                }
            }
            edges.push(row);
        }
        let catalog = Self { units, edges };
        catalog.validate_cycles(&source_ids)?;
        Ok(catalog)
    }
    pub fn len(&self) -> usize {
        self.units.len()
    }
    pub fn is_empty(&self) -> bool {
        self.units.is_empty()
    }
    pub fn unit(&self, id: usize) -> Option<&Unit> {
        self.units.get(id)
    }
    pub fn lookup(&self, name: &str) -> Option<usize> {
        self.units.iter().position(|u| u.name == name)
    }
    pub fn edges(&self, id: usize) -> Option<Edges> {
        self.edges.get(id).copied()
    }
    /// Wants/Requires pull units in; After only orders units already selected.
    pub fn plan_start(&self, root: &str) -> Result<Plan, Error> {
        let id = self
            .lookup(root)
            .ok_or(error(0, ErrorKind::UnknownTarget))?;
        let mut selected = 1u64 << id;
        loop {
            let mut expanded = selected;
            for (id, row) in self.edges.iter().enumerate() {
                if selected & (1u64 << id) != 0 {
                    expanded |= row.requires | row.wants;
                }
            }
            if expanded == selected {
                break;
            }
            selected = expanded;
        }
        let mut order = Vec::new();
        reserve(&mut order, selected.count_ones() as usize, 0)?;
        let mut pending = selected;
        while pending != 0 {
            let id = (0..self.len())
                .find(|&id| {
                    pending & (1u64 << id) != 0
                        && (self.edges[id].requires | self.edges[id].after) & pending == 0
                })
                .ok_or(error(0, ErrorKind::Invariant))?;
            pending &= !(1u64 << id);
            order.push(id);
        }
        Ok(Plan { selected, order })
    }
    fn validate_cycles(&self, sources: &[usize]) -> Result<(), Error> {
        // Iterative DFS: bounded stack/cursors; gray vertices are on this stack.
        let mut marks = [0u8; crate::config::MAX_UNITS];
        let mut cursors = [0usize; crate::config::MAX_UNITS];
        let mut stack = [0usize; crate::config::MAX_UNITS];
        for start in 0..self.len() {
            if marks[start] != 0 {
                continue;
            }
            let mut depth = 1;
            stack[0] = start;
            marks[start] = 1;
            while depth != 0 {
                let id = stack[depth - 1];
                let barriers = self.edges[id].requires | self.edges[id].after;
                let next = (cursors[id]..self.len()).find(|&n| barriers & (1u64 << n) != 0);
                let Some(next) = next else {
                    marks[id] = 2;
                    depth -= 1;
                    continue;
                };
                cursors[id] = next + 1;
                if marks[next] == 1 {
                    let begin = stack[..depth]
                        .iter()
                        .position(|&n| n == next)
                        .ok_or(error(sources[id], ErrorKind::Invariant))?;
                    let mut path = Vec::new();
                    reserve(&mut path, depth - begin + 1, sources[id])?;
                    for &n in &stack[begin..depth] {
                        path.push(owned(&self.units[n].name, sources[id])?);
                    }
                    path.push(owned(&self.units[next].name, sources[id])?);
                    return Err(error(sources[id], ErrorKind::Cycle(path)));
                }
                if marks[next] == 0 {
                    stack[depth] = next;
                    depth += 1;
                    marks[next] = 1;
                }
            }
        }
        Ok(())
    }
}
fn error(source: usize, kind: ErrorKind) -> Error {
    Error { source, kind }
}
fn reserve<T>(values: &mut Vec<T>, count: usize, source: usize) -> Result<(), Error> {
    values
        .try_reserve_exact(count)
        .map_err(|_| error(source, ErrorKind::Allocation))
}
fn owned(value: &str, source: usize) -> Result<String, Error> {
    let mut text = String::new();
    text.try_reserve_exact(value.len())
        .map_err(|_| error(source, ErrorKind::Allocation))?;
    text.push_str(value);
    Ok(text)
}
