//! Process-owned working directories and bounded regular-file descriptors.
//! Lock order: filesystem service -> open file. Never hold the scheduler lock
//! during disk I/O or while taking either service lock.
use alloc::{string::String, sync::Arc};
use rustos_scope_core::Usage;
use spin::Mutex;

pub(crate) const OPEN: u64 = 23;
pub(crate) const CLOSE: u64 = 24;
pub(crate) const WRITE: u64 = 25;
pub(crate) const LSEEK: u64 = 26;
pub(crate) const CHDIR: u64 = 27;
pub(crate) const GETCWD: u64 = 28;
const ERROR: u64 = u64::MAX - 1;
const MAX_IO: usize = 64 * 1024;
const MAX_DESCRIPTORS: usize = 1024;
const CREAT: u64 = 0o100;
const TRUNC: u64 = 0o1000;

pub(crate) struct OpenFile {
    path: String,
    offset: usize,
    access: u64,
    ready: bool,
    // Field order releases the path allocation before publishing quota release.
    // Shared Arc owners retain this lease through concurrent/blocked file I/O.
    _lease: crate::scheduler::ResourceLease,
}
pub(crate) struct Files {
    pub(crate) cwd: String,
    slots: alloc::vec::Vec<Option<Arc<Mutex<OpenFile>>>>,
}
impl Files {
    pub(crate) fn new(cwd: String) -> Self {
        Self {
            cwd,
            slots: alloc::vec::Vec::new(),
        }
    }
    pub(crate) fn clear(&mut self) {
        self.slots.fill_with(|| None);
    }
    fn descriptor(&self, fd: u64) -> Option<Arc<Mutex<OpenFile>>> {
        self.slots
            .get(usize::try_from(fd.checked_sub(3)?).ok()?)?
            .clone()
    }
}

pub(crate) enum PathKind {
    Existing,
    RecursiveCreate,
    Listing,
}

/// Copy and resolve a user path against a snapshot of the calling process cwd.
/// Each traversed component must be a directory, even before `..` or `.`.
/// The legacy recursive mkdir alone permits missing intermediate directories.
pub(crate) fn path(pointer: u64, kind: PathKind) -> Option<String> {
    let bytes = crate::userspace::usercopy::cstring(pointer, 4096)?;
    if bytes.is_empty() && !matches!(kind, PathKind::Listing) {
        return None;
    }
    let input = core::str::from_utf8(&bytes).ok()?;
    let cwd = crate::scheduler::with_files(|files| files.cwd.clone())?;
    super::with_fs(|fs| {
        let mut out = if input.starts_with('/') {
            String::from("/")
        } else {
            cwd
        };
        let mut components = input.split('/').filter(|p| !p.is_empty()).peekable();
        while let Some(component) = components.next() {
            match component {
                "." | ".." => {
                    fs.check_directory(&out).ok()?;
                    if component == ".." {
                        let index = out.rfind('/').unwrap_or(0);
                        out.truncate(index.max(1));
                    }
                }
                _ => {
                    if out.len() > 1 {
                        out.push('/');
                    }
                    out.push_str(component);
                    if out.len() >= 4096 {
                        return None;
                    }
                    // Keep conventional short-name spelling while preserving
                    // Unicode/case in long components. FAT lookup is case-insensitive.
                    if crate::fat32::short_component(component) {
                        let start = out.len() - component.len();
                        out[start..].make_ascii_uppercase();
                    }
                    if !matches!(kind, PathKind::RecursiveCreate)
                        && (components.peek().is_some() || input.ends_with('/'))
                    {
                        fs.check_directory(&out).ok()?;
                    }
                }
            }
        }
        Some(out)
    })?
}

/// Kernel facade for descriptor/cwd operations in the current process's address
/// space. The syscall dispatcher applies capability filtering before this helper;
/// each operation independently validates buffers, ownership and native quotas.
pub fn syscall(number: u64, first: u64, second: u64, third: u64) -> u64 {
    match number {
        OPEN if third == 0 => open(first, second),
        CLOSE if second == 0 && third == 0 => crate::scheduler::with_files(|files| {
            let index = first.checked_sub(3).and_then(|n| usize::try_from(n).ok());
            index
                .and_then(|i| files.slots.get_mut(i))
                .and_then(Option::take)
                .map_or(ERROR, |_| 0)
        })
        .unwrap_or(ERROR),
        WRITE => write(first, second, third),
        LSEEK => seek(first, second as i64, third),
        CHDIR if second == 0 && third == 0 => {
            let Some(path) = path(first, PathKind::Existing) else {
                return ERROR;
            };
            if !matches!(super::with_fs(|fs| fs.check_directory(&path)), Some(Ok(()))) {
                return ERROR;
            }
            crate::scheduler::with_files(|files| {
                files.cwd = path;
                0
            })
            .unwrap_or(ERROR)
        }
        GETCWD if third == 0 => {
            let Some(cwd) = crate::scheduler::with_files(|files| files.cwd.clone()) else {
                return ERROR;
            };
            let mut bytes = cwd.into_bytes();
            bytes.push(0);
            if second < bytes.len() as u64
                || !crate::userspace::usercopy::validate(first, second, true)
                || !crate::userspace::usercopy::to_user(first, &bytes)
            {
                return ERROR;
            }
            bytes.len() as u64
        }
        _ => ERROR,
    }
}

fn open(pointer: u64, flags: u64) -> u64 {
    if flags & !(3 | CREAT | TRUNC) != 0 || flags & 3 == 3 || (flags & TRUNC != 0 && flags & 3 == 0)
    {
        return ERROR;
    }
    let Some(path) = path(pointer, PathKind::Existing) else {
        return ERROR;
    };
    // Reserve the slot before any create/truncate side effects. This also makes
    // concurrent opens in sibling threads observe the process descriptor quota.
    let Ok(lease) = crate::scheduler::reserve_resource(Usage {
        descriptors: 1,
        kernel_bytes: path.capacity() as u64,
        ..Usage::ZERO
    }) else {
        return ERROR;
    };
    let Ok(file) = Arc::try_new(Mutex::new(OpenFile {
        path,
        offset: 0,
        access: flags & 3,
        ready: false,
        _lease: lease,
    })) else {
        return ERROR;
    };
    let Some(Some(fd)) = crate::scheduler::with_files(|files| {
        let index = if let Some(index) = files.slots.iter().position(Option::is_none) {
            index
        } else {
            if files.slots.len() == MAX_DESCRIPTORS || files.slots.try_reserve(1).is_err() {
                return None;
            }
            files.slots.push(None);
            files.slots.len() - 1
        };
        files.slots[index] = Some(file.clone());
        Some(index as u64 + 3)
    }) else {
        return ERROR;
    };
    let system = crate::scheduler::system_write_authority();
    let result = super::with_fs(|fs| {
        // The service lock precedes the open-file lock. No scheduler acquisition
        // occurs here; path and reservation remain owned through the whole I/O.
        let mut open_file = file.lock();
        if flags & 3 != 0 || flags & (CREAT | TRUNC) != 0 {
            super::authorize_write(fs, &open_file.path, system)?;
        }
        match fs.file_size(&open_file.path) {
            Ok(_) => (),
            Err(crate::fat32::FatError::FileNotFound) if flags & CREAT != 0 => {
                fs.create_file(&open_file.path)?
            }
            Err(error) => return Err(error),
        }
        if flags & TRUNC != 0 {
            fs.write_file(&open_file.path, &[])?;
        }
        open_file.ready = true;
        Ok(())
    });
    if matches!(result, Some(Ok(()))) {
        fd
    } else {
        crate::scheduler::with_files(|files| {
            // A sibling may have closed/reused this number while disk I/O ran.
            let slot = &mut files.slots[(fd - 3) as usize];
            if slot.as_ref().is_some_and(|entry| Arc::ptr_eq(entry, &file)) {
                *slot = None;
            }
        });
        ERROR
    }
}
fn descriptor(fd: u64) -> Option<Arc<Mutex<OpenFile>>> {
    crate::scheduler::with_files(|files| files.descriptor(fd))?
}

/// Descriptor read used by the existing read syscall for fd >= 3.
pub fn read(fd: u64, pointer: u64, length: u64) -> u64 {
    let Some(file) = descriptor(fd) else {
        return ERROR;
    };
    let count = length.min(MAX_IO as u64) as usize;
    if !crate::userspace::usercopy::validate(pointer, length, true) {
        return ERROR;
    }
    // Declare the lease before the buffer: every return drops buffer storage
    // before the lock-free release is published, including I/O/copy failure.
    let Ok(lease) = crate::scheduler::reserve_resource(Usage {
        kernel_bytes: count as u64,
        ..Usage::ZERO
    }) else {
        return ERROR;
    };
    let mut bytes = alloc::vec::Vec::new();
    if bytes.try_reserve_exact(count).is_err() {
        return ERROR;
    }
    if crate::scheduler::grow_resource(
        &lease,
        Usage {
            kernel_bytes: (bytes.capacity() - count) as u64,
            ..Usage::ZERO
        },
    )
    .is_err()
    {
        return ERROR;
    }
    bytes.resize(count, 0);
    // Copying to user memory requires the entry gate. Reserve an offset only
    // after disk I/O succeeded; the full destination was validated above.
    let result = super::with_fs(|fs| {
        let mut file = file.lock();
        if !file.ready || file.access == 1 {
            return Err(crate::fat32::FatError::InvalidName);
        }
        let n = fs.read_at(&file.path, file.offset, &mut bytes)?;
        file.offset += n;
        Ok(n)
    });
    match result {
        Some(Ok(n)) if crate::userspace::usercopy::to_user(pointer, &bytes[..n]) => n as u64,
        _ => ERROR,
    }
}
fn write(fd: u64, pointer: u64, length: u64) -> u64 {
    let Some(file) = descriptor(fd) else {
        return ERROR;
    };
    if !crate::userspace::usercopy::validate(pointer, length, false) {
        return ERROR;
    }
    let count = length.min(MAX_IO as u64) as usize;
    // Declare the lease before the buffer: every return drops buffer storage
    // before the lock-free release is published, including I/O/copy failure.
    let Ok(lease) = crate::scheduler::reserve_resource(Usage {
        kernel_bytes: count as u64,
        ..Usage::ZERO
    }) else {
        return ERROR;
    };
    let mut bytes = alloc::vec::Vec::new();
    if bytes.try_reserve_exact(count).is_err() {
        return ERROR;
    }
    if crate::scheduler::grow_resource(
        &lease,
        Usage {
            kernel_bytes: (bytes.capacity() - count) as u64,
            ..Usage::ZERO
        },
    )
    .is_err()
    {
        return ERROR;
    }
    bytes.resize(count, 0);
    if !crate::userspace::usercopy::from_user(pointer, &mut bytes) {
        return ERROR;
    }
    let system = crate::scheduler::system_write_authority();
    super::with_fs(|fs| {
        let mut file = file.lock();
        if super::authorize_write(fs, &file.path, system).is_err() {
            return ERROR;
        }
        if !file.ready || file.access == 0 {
            return ERROR;
        }
        if count == 0 {
            return 0;
        }
        let Ok(written) = fs.write_at(&file.path, file.offset, &bytes) else {
            return ERROR;
        };
        file.offset += written;
        count as u64
    })
    .unwrap_or(ERROR)
}
fn seek(fd: u64, offset: i64, whence: u64) -> u64 {
    let Some(file) = descriptor(fd) else {
        return ERROR;
    };
    super::with_fs(|fs| {
        let mut file = file.lock();
        if !file.ready {
            return ERROR;
        }
        let base = match whence {
            0 => 0,
            1 => file.offset as u64,
            2 => match fs.file_size(&file.path) {
                Ok(n) => n as u64,
                _ => return ERROR,
            },
            _ => return ERROR,
        };
        let Some(position) = base
            .checked_add_signed(offset)
            .filter(|&n| n <= u32::MAX as u64)
        else {
            return ERROR;
        };
        file.offset = position as usize;
        position
    })
    .unwrap_or(ERROR)
}

/// Versioned long-name directory records. A zero capacity queries required bytes;
/// nonzero calls copy a complete listing or return an error without partial output.
pub(crate) fn readdir(path_pointer: u64, output: u64, capacity: u64) -> u64 {
    let Some(path) = path(path_pointer, PathKind::Listing) else {
        return ERROR;
    };
    let Some(Ok(entries)) = super::with_fs(|fs| fs.list_dir(Some(&path))) else {
        return ERROR;
    };
    let mut size = 0usize;
    for entry in &entries {
        let name = entry.file_name();
        let Some(next) = size.checked_add((16 + name.len() + 1 + 7) & !7) else {
            return ERROR;
        };
        size = next;
    }
    if capacity == 0 {
        return size as u64;
    }
    if capacity < size as u64 || !crate::userspace::usercopy::validate(output, capacity, true) {
        return ERROR;
    }
    let mut bytes = alloc::vec::Vec::new();
    if bytes.try_reserve_exact(size).is_err() {
        return ERROR;
    }
    bytes.resize(size, 0);
    let mut position = 0;
    for entry in entries {
        let name = entry.file_name();
        let length = (16 + name.len() + 1 + 7) & !7;
        bytes[position..position + 2].copy_from_slice(&(length as u16).to_le_bytes());
        bytes[position + 2..position + 4].copy_from_slice(&(name.len() as u16).to_le_bytes());
        bytes[position + 4] = entry.attr;
        bytes[position + 8..position + 16].copy_from_slice(&(entry.size as u64).to_le_bytes());
        bytes[position + 16..position + 16 + name.len()].copy_from_slice(name.as_bytes());
        position += length;
    }
    if crate::userspace::usercopy::to_user(output, &bytes) {
        size as u64
    } else {
        ERROR
    }
}
