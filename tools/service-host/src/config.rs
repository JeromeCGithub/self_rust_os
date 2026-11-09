//! Bounded host filesystem adapter; the shared native parser remains authoritative.
use rustos_service_core::{
    config::{MAX_CATALOG_BYTES, MAX_FILE_BYTES, MAX_UNITS},
    graph::Catalog,
};
use std::{
    fs,
    io::{self, Read},
    path::Path,
};
pub fn load(directory: &Path) -> io::Result<Catalog> {
    let mut sources = Vec::new();
    let mut total = 0usize;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if sources.len() >= MAX_UNITS || !entry.file_type()?.is_file() {
            return Err(io::Error::other("configuration entry/count limit"));
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| io::Error::other("configuration filename encoding"))?;
        let size = usize::try_from(entry.metadata()?.len())
            .map_err(|_| io::Error::other("configuration size"))?;
        total = total
            .checked_add(size)
            .filter(|&n| n <= MAX_CATALOG_BYTES)
            .ok_or_else(|| io::Error::other("configuration aggregate limit"))?;
        if size > MAX_FILE_BYTES {
            return Err(io::Error::other("configuration file limit"));
        }
        let mut bytes = Vec::with_capacity(MAX_FILE_BYTES + 1);
        fs::File::open(entry.path())?
            .take((MAX_FILE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() != size {
            return Err(io::Error::other("configuration changed during read"));
        }
        sources.push((name, bytes));
    }
    let views: Vec<_> = sources
        .iter()
        .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
        .collect();
    Catalog::load(&views).map_err(|error| io::Error::other(format!("configuration: {error:?}")))
}
