//! In-memory filesystem (`ramfs`).
//!
//! This module implements a tiny volatile filesystem that lives entirely in
//! kernel heap memory. It is intentionally simple: a flat namespace (every
//! file lives at the filesystem root), a fixed maximum number of files, and
//! a fixed-size table of open file descriptions.
//!
//! The filesystem is exposed to user-mode programs through four syscalls
//! (see [`crate::userspace::syscall`]):
//!
//! | Number | Name         | Description                               |
//! |--------|----------------|-------------------------------------------|
//! | `2`    | `sys_open`   | Opens (or creates) a file, returns a fd.  |
//! | `3`    | `sys_close`  | Closes an open file descriptor.           |
//! | `4`    | `sys_read`   | Reads bytes from an open file.            |
//! | `5`    | `sys_fwrite` | Writes bytes to an open file.             |
//!
//! Because the filesystem lives in RAM, all files are lost on reboot. It is
//! meant as the first storage building block of the OS: user programs can
//! already share data through files, and a future persistent block-device
//! driver can reuse the same syscall interface.

use alloc::{collections::BTreeMap, string::String, vec::Vec};
use core::array;

use lazy_static::lazy_static;
use spin::Mutex;

use crate::serial_println;

/// Maximum length of a file path in bytes, including the leading `/`.
pub const MAX_PATH_LEN: usize = 64;

/// Maximum size of a single file in bytes (64 KiB).
pub const MAX_FILE_SIZE: usize = 64 * 1024;

/// Maximum number of files the filesystem can hold at once.
pub const MAX_FILES: usize = 32;

/// Maximum number of simultaneously open file descriptions.
pub const MAX_OPEN_FILES: usize = 32;

// The open file table is indexed by a `u32` file descriptor.
const _: () = assert!(
    MAX_OPEN_FILES <= u32::MAX as usize,
    "the open file table must be indexable by a u32 file descriptor"
);

/// Open flag: open the file for reading.
pub const O_READ: u32 = 0x01;
/// Open flag: open the file for writing.
pub const O_WRITE: u32 = 0x02;
/// Open flag: create the file if it does not exist.
pub const O_CREATE: u32 = 0x04;
/// Open flag: truncate the file to zero length on open (requires [`O_WRITE`]).
pub const O_TRUNC: u32 = 0x08;
/// Open flag: all writes append at the end of the file.
pub const O_APPEND: u32 = 0x10;

/// Bitmask of all valid open flags.
const VALID_FLAGS: u32 = O_READ | O_WRITE | O_CREATE | O_TRUNC | O_APPEND;

/// A file descriptor: an index into the open file table.
pub type Fd = u32;

/// Path of the welcome file seeded at boot by [`init`].
pub const WELCOME_PATH: &str = "/welcome.txt";

/// Content of the welcome file seeded at boot.
const WELCOME_MESSAGE: &str =
    "Welcome to self_rust_os!\nThis file lives in the in-memory filesystem (ramfs).\n";

/// Errors that filesystem operations can return.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsError {
    /// The requested file does not exist.
    NotFound,
    /// The file already exists ([`create`] on an existing path).
    AlreadyExists,
    /// The path is malformed (see [`validate_path`]).
    InvalidPath,
    /// The file descriptor is not currently open.
    InvalidFd,
    /// The file was not opened for reading.
    NotReadable,
    /// The file was not opened for writing.
    NotWritable,
    /// The open file table is full.
    TooManyOpenFiles,
    /// The filesystem already holds [`MAX_FILES`] files.
    TooManyFiles,
    /// The operation would make the file larger than [`MAX_FILE_SIZE`].
    FileTooLarge,
    /// The flags passed to [`open`] are invalid.
    InvalidFlags,
    /// The file is currently open and cannot be removed.
    FileBusy,
}

/// A file stored in the filesystem: its content as a byte vector.
struct FileData {
    content: Vec<u8>,
}

/// An open file description: which file it refers to, the current cursor
/// position, and the access mode it was opened with.
struct OpenFile {
    name: String,
    cursor: usize,
    readable: bool,
    writable: bool,
    append: bool,
}

/// The whole filesystem state, protected by a single mutex.
///
/// A single lock for both the file store and the open file table keeps the
/// locking discipline trivial: there is no lock ordering to get wrong.
struct RamFs {
    files: BTreeMap<String, FileData>,
    open_files: [Option<OpenFile>; MAX_OPEN_FILES],
}

impl RamFs {
    /// Creates an empty filesystem.
    fn new() -> Self {
        Self {
            files: BTreeMap::new(),
            open_files: array::from_fn(|_| None),
        }
    }
}

lazy_static! {
    static ref FS: Mutex<RamFs> = Mutex::new(RamFs::new());
}

/// Validates a filesystem path.
///
/// A valid path:
/// - is not empty, is at most [`MAX_PATH_LEN`] bytes long, and starts with `/`,
/// - has at least one component (the root `/` alone is not a file),
/// - has no empty components (`//`), no trailing `/`, and no `.`/`..` components,
/// - only contains ASCII alphanumeric characters plus `.`, `_` and `-` in
///   each component.
///
/// The namespace is flat: `/` separators are allowed syntactically but every
/// file effectively lives at the root.
fn validate_path(path: &str) -> Result<(), FsError> {
    if path.is_empty() || path.len() > MAX_PATH_LEN {
        return Err(FsError::InvalidPath);
    }
    let mut components = path.split('/');
    // A valid absolute path starts with '/', so the first component is empty.
    if components.next() != Some("") {
        return Err(FsError::InvalidPath);
    }
    let mut count = 0;
    for component in components {
        count += 1;
        if component.is_empty() || component == "." || component == ".." {
            return Err(FsError::InvalidPath);
        }
        let valid_chars = component
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
        if !valid_chars {
            return Err(FsError::InvalidPath);
        }
    }
    if count == 0 {
        // The path was just "/": the root is not a file.
        return Err(FsError::InvalidPath);
    }
    Ok(())
}

/// Converts a file descriptor into an index into the open file table.
fn fd_to_index(fd: Fd) -> Result<usize, FsError> {
    let Ok(index) = usize::try_from(fd) else {
        return Err(FsError::InvalidFd);
    };
    Ok(index)
}

/// Allocates a file descriptor for an already-validated path.
///
/// The caller must hold the filesystem lock and guarantee that `path`
/// refers to an existing file.
fn alloc_fd(
    fs: &mut RamFs,
    path: &str,
    readable: bool,
    writable: bool,
    append: bool,
) -> Result<Fd, FsError> {
    let index = fs
        .open_files
        .iter()
        .position(Option::is_none)
        .ok_or(FsError::TooManyOpenFiles)?;
    let Ok(fd) = u32::try_from(index) else {
        // Unreachable: the const assertion above guarantees the table fits in a `u32`.
        return Err(FsError::TooManyOpenFiles);
    };
    let cursor = if append {
        fs.files.get(path).map_or(0, |file| file.content.len())
    } else {
        0
    };
    fs.open_files[index] = Some(OpenFile {
        name: String::from(path),
        cursor,
        readable,
        writable,
        append,
    });
    Ok(fd)
}

/// Initializes the in-memory filesystem and seeds it with a welcome file.
///
/// This must be called once after the heap is initialized, before user
/// programs run. Calling it again is harmless: the welcome file is only
/// created when missing.
pub fn init() {
    // Best-effort seeding; failures are non-fatal (e.g., init called twice).
    if create(WELCOME_PATH).is_ok() {
        if let Ok(fd) = open(WELCOME_PATH, O_WRITE) {
            let wrote = write(fd, WELCOME_MESSAGE.as_bytes());
            let closed = close(fd);
            if wrote.is_err() || closed.is_err() {
                serial_println!("[kernel] warning: failed to seed welcome file");
            }
        }
    }
    serial_println!("[kernel] ramfs initialized");
}

/// Creates a new empty file and returns a file descriptor opened for
/// reading and writing.
///
/// # Errors
///
/// - [`FsError::InvalidPath`] if the path is malformed.
/// - [`FsError::AlreadyExists`] if a file already exists at the path.
/// - [`FsError::TooManyFiles`] if the filesystem is full.
/// - [`FsError::TooManyOpenFiles`] if the open file table is full.
pub fn create(path: &str) -> Result<Fd, FsError> {
    validate_path(path)?;
    let mut fs = FS.lock();
    if fs.files.contains_key(path) {
        return Err(FsError::AlreadyExists);
    }
    if fs.files.len() >= MAX_FILES {
        return Err(FsError::TooManyFiles);
    }
    fs.files
        .insert(String::from(path), FileData { content: Vec::new() });
    // New files are opened readable and writable so the caller can use the
    // returned descriptor immediately.
    alloc_fd(&mut fs, path, true, true, false)
}

/// Opens a file and returns a file descriptor.
///
/// `flags` is a combination of [`O_READ`], [`O_WRITE`], [`O_CREATE`],
/// [`O_TRUNC`] and [`O_APPEND`]. At least one of [`O_READ`]/[`O_WRITE`]
/// must be set. With [`O_CREATE`], a missing file is created; without it,
/// opening a missing file fails with [`FsError::NotFound`]. [`O_TRUNC`]
/// requires [`O_WRITE`] and empties an existing file.
///
/// # Errors
///
/// - [`FsError::InvalidPath`] if the path is malformed.
/// - [`FsError::InvalidFlags`] if the flags are malformed or contradictory.
/// - [`FsError::NotFound`] if the file does not exist and [`O_CREATE`] is
///   not set.
/// - [`FsError::TooManyFiles`] if creation is requested but the filesystem
///   is full.
/// - [`FsError::TooManyOpenFiles`] if the open file table is full.
pub fn open(path: &str, flags: u32) -> Result<Fd, FsError> {
    validate_path(path)?;
    if flags & !VALID_FLAGS != 0 {
        return Err(FsError::InvalidFlags);
    }
    let readable = flags & O_READ != 0;
    let writable = flags & O_WRITE != 0;
    if !readable && !writable {
        return Err(FsError::InvalidFlags);
    }
    let mut fs = FS.lock();
    if fs.files.contains_key(path) {
        if flags & O_TRUNC != 0 {
            if !writable {
                return Err(FsError::InvalidFlags);
            }
            if let Some(file) = fs.files.get_mut(path) {
                file.content.clear();
            }
        }
    } else {
        if flags & O_CREATE == 0 {
            return Err(FsError::NotFound);
        }
        if fs.files.len() >= MAX_FILES {
            return Err(FsError::TooManyFiles);
        }
        fs.files
            .insert(String::from(path), FileData { content: Vec::new() });
    }
    alloc_fd(&mut fs, path, readable, writable, flags & O_APPEND != 0)
}

/// Reads up to `buf.len()` bytes from the file at the current cursor
/// position, advances the cursor, and returns the number of bytes read.
///
/// Returns `Ok(0)` at end of file. The cursor never moves past the end of
/// the file.
///
/// # Errors
///
/// - [`FsError::InvalidFd`] if the descriptor is not open.
/// - [`FsError::NotReadable`] if the file was not opened for reading.
/// - [`FsError::NotFound`] if the underlying file was removed.
pub fn read(fd: Fd, buf: &mut [u8]) -> Result<usize, FsError> {
    let mut fs = FS.lock();
    let index = fd_to_index(fd)?;
    // Copy out the fields we need so the borrow of the open file table ends
    // before we touch the file store.
    let (name, cursor, readable) = {
        let open = fs
            .open_files
            .get(index)
            .and_then(Option::as_ref)
            .ok_or(FsError::InvalidFd)?;
        (open.name.clone(), open.cursor, open.readable)
    };
    if !readable {
        return Err(FsError::NotReadable);
    }
    let file = fs.files.get(&name).ok_or(FsError::NotFound)?;
    let start = core::cmp::min(cursor, file.content.len());
    let len = core::cmp::min(file.content.len() - start, buf.len());
    buf[..len].copy_from_slice(&file.content[start..start + len]);
    if let Some(open) = fs
        .open_files
        .get_mut(index)
        .and_then(Option::as_mut)
    {
        open.cursor = start + len;
    }
    Ok(len)
}

/// Writes `buf` at the current cursor position (or at the end of the file
/// with [`O_APPEND`]), advances the cursor past the written bytes, and
/// returns the number of bytes written.
///
/// Writing past the current end of the file zero-fills the gap. The file
/// can never grow beyond [`MAX_FILE_SIZE`].
///
/// # Errors
///
/// - [`FsError::InvalidFd`] if the descriptor is not open.
/// - [`FsError::NotWritable`] if the file was not opened for writing.
/// - [`FsError::NotFound`] if the underlying file was removed.
/// - [`FsError::FileTooLarge`] if the write would exceed [`MAX_FILE_SIZE`].
pub fn write(fd: Fd, buf: &[u8]) -> Result<usize, FsError> {
    let mut fs = FS.lock();
    let index = fd_to_index(fd)?;
    // Copy out the fields we need so the borrow of the open file table ends
    // before we touch the file store.
    let (name, cursor, writable, append) = {
        let open = fs
            .open_files
            .get(index)
            .and_then(Option::as_ref)
            .ok_or(FsError::InvalidFd)?;
        (
            open.name.clone(),
            open.cursor,
            open.writable,
            open.append,
        )
    };
    if !writable {
        return Err(FsError::NotWritable);
    }
    let end = {
        let file = fs.files.get_mut(&name).ok_or(FsError::NotFound)?;
        let start = if append { file.content.len() } else { cursor };
        let end = start.saturating_add(buf.len());
        if end > MAX_FILE_SIZE {
            return Err(FsError::FileTooLarge);
        }
        if end > file.content.len() {
            file.content.resize(end, 0);
        }
        file.content[start..end].copy_from_slice(buf);
        end
    };
    if let Some(open) = fs
        .open_files
        .get_mut(index)
        .and_then(Option::as_mut)
    {
        open.cursor = end;
    }
    Ok(buf.len())
}

/// Closes an open file descriptor, making it available for reuse.
///
/// # Errors
///
/// [`FsError::InvalidFd`] if the descriptor is not currently open.
pub fn close(fd: Fd) -> Result<(), FsError> {
    let mut fs = FS.lock();
    let index = fd_to_index(fd)?;
    let entry = fs.open_files.get_mut(index).ok_or(FsError::InvalidFd)?;
    if entry.take().is_some() {
        Ok(())
    } else {
        Err(FsError::InvalidFd)
    }
}

/// Removes a file from the filesystem.
///
/// A file that is currently open cannot be removed.
///
/// # Errors
///
/// - [`FsError::InvalidPath`] if the path is malformed.
/// - [`FsError::FileBusy`] if the file is currently open.
/// - [`FsError::NotFound`] if no file exists at the path.
pub fn remove(path: &str) -> Result<(), FsError> {
    validate_path(path)?;
    let mut fs = FS.lock();
    if fs.open_files.iter().flatten().any(|open| open.name == path) {
        return Err(FsError::FileBusy);
    }
    fs.files.remove(path).map(|_| ()).ok_or(FsError::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    #[expect(clippy::expect_used)]
    fn test_create_write_read_close_roundtrip() {
        let fd = create("/test_roundtrip.txt").expect("create should succeed");
        let written = write(fd, b"hello ramfs").expect("write should succeed");
        assert_eq!(written, 11, "write should report all bytes written.");
        close(fd).expect("close should succeed");

        let fd = open("/test_roundtrip.txt", O_READ).expect("open should succeed");
        let mut buf = [0_u8; 32];
        let n = read(fd, &mut buf).expect("read should succeed");
        assert_eq!(n, 11, "read should return the written bytes.");
        assert_eq!(&buf[..n], b"hello ramfs", "content should round-trip.");
        // Reading again hits end of file.
        let n = read(fd, &mut buf).expect("second read should succeed");
        assert_eq!(n, 0, "read at EOF should return 0.");
        close(fd).expect("close should succeed");
    }

    #[test_case]
    fn test_open_nonexistent_without_create_fails() {
        assert_eq!(
            open("/test_missing.txt", O_READ),
            Err(FsError::NotFound),
            "opening a missing file without O_CREATE should fail."
        );
    }

    #[test_case]
    #[expect(clippy::expect_used)]
    fn test_double_create_fails() {
        let fd = create("/test_double.txt").expect("first create should succeed");
        close(fd).expect("close should succeed");
        assert_eq!(
            create("/test_double.txt"),
            Err(FsError::AlreadyExists),
            "creating an existing file should fail."
        );
    }

    #[test_case]
    #[expect(clippy::expect_used)]
    fn test_open_create_trunc() {
        let fd = open("/test_trunc.txt", O_CREATE | O_WRITE).expect("create should succeed");
        write(fd, b"old content").expect("write should succeed");
        close(fd).expect("close should succeed");

        // Reopen with O_TRUNC: previous content must be gone.
        let fd = open("/test_trunc.txt", O_READ | O_WRITE | O_TRUNC)
            .expect("open with O_TRUNC should succeed");
        let mut buf = [0_u8; 32];
        let n = read(fd, &mut buf).expect("read should succeed");
        assert_eq!(n, 0, "truncated file should be empty.");
        close(fd).expect("close should succeed");
    }

    #[test_case]
    #[expect(clippy::expect_used)]
    fn test_write_requires_write_flag() {
        let fd = create("/test_ro.txt").expect("create should succeed");
        close(fd).expect("close should succeed");
        let fd = open("/test_ro.txt", O_READ).expect("open read-only should succeed");
        assert_eq!(
            write(fd, b"x"),
            Err(FsError::NotWritable),
            "writing to a read-only descriptor should fail."
        );
        close(fd).expect("close should succeed");
    }

    #[test_case]
    #[expect(clippy::expect_used)]
    fn test_read_requires_read_flag() {
        let fd = create("/test_wo.txt").expect("create should succeed");
        // create() opens read+write; close and reopen write-only.
        close(fd).expect("close should succeed");
        let fd = open("/test_wo.txt", O_WRITE).expect("open write-only should succeed");
        let mut buf = [0_u8; 8];
        assert_eq!(
            read(fd, &mut buf),
            Err(FsError::NotReadable),
            "reading from a write-only descriptor should fail."
        );
        close(fd).expect("close should succeed");
    }

    #[test_case]
    fn test_invalid_paths_rejected() {
        for path in ["", "/", "/a//b", "/a/", "/.", "/..", "/a/./b", "/a/../b"] {
            assert_eq!(
                create(path),
                Err(FsError::InvalidPath),
                "path should be rejected."
            );
        }
    }

    #[test_case]
    #[expect(clippy::expect_used)]
    fn test_write_beyond_max_file_size_fails() {
        let fd = create("/test_big.txt").expect("create should succeed");
        let chunk = [0xAB_u8; 4096];
        // Fill the file up to exactly MAX_FILE_SIZE (64 KiB / 4 KiB chunks).
        for _ in 0..16 {
            write(fd, &chunk).expect("chunked write should succeed");
        }
        assert_eq!(
            write(fd, &[0]),
            Err(FsError::FileTooLarge),
            "writing past MAX_FILE_SIZE should fail."
        );
        close(fd).expect("close should succeed");
    }

    #[test_case]
    #[expect(clippy::expect_used)]
    fn test_remove_open_file_fails() {
        let fd = create("/test_busy.txt").expect("create should succeed");
        assert_eq!(
            remove("/test_busy.txt"),
            Err(FsError::FileBusy),
            "removing an open file should fail."
        );
        close(fd).expect("close should succeed");
        remove("/test_busy.txt").expect("remove after close should succeed");
        assert_eq!(
            open("/test_busy.txt", O_READ),
            Err(FsError::NotFound),
            "removed file should be gone."
        );
    }

    #[test_case]
    #[expect(clippy::expect_used)]
    fn test_append_mode_writes_at_end() {
        let fd = create("/test_append.txt").expect("create should succeed");
        write(fd, b"first").expect("write should succeed");
        close(fd).expect("close should succeed");

        let fd = open("/test_append.txt", O_WRITE | O_APPEND).expect("open should succeed");
        write(fd, b"second").expect("append should succeed");
        close(fd).expect("close should succeed");

        let fd = open("/test_append.txt", O_READ).expect("open should succeed");
        let mut buf = [0_u8; 32];
        let n = read(fd, &mut buf).expect("read should succeed");
        assert_eq!(&buf[..n], b"firstsecond", "append should add at the end.");
        close(fd).expect("close should succeed");
    }

    #[test_case]
    fn test_invalid_flags_rejected() {
        assert_eq!(
            open("/test_flags.txt", 0),
            Err(FsError::InvalidFlags),
            "open with no access mode should fail."
        );
        assert_eq!(
            open("/test_flags.txt", 0x100),
            Err(FsError::InvalidFlags),
            "open with unknown flag bits should fail."
        );
        assert_eq!(
            open("/test_flags.txt", O_TRUNC),
            Err(FsError::InvalidFlags),
            "O_TRUNC without O_WRITE should fail."
        );
    }

    #[test_case]
    fn test_invalid_fd_rejected() {
        let mut buf = [0_u8; 8];
        assert_eq!(
            read(9999, &mut buf),
            Err(FsError::InvalidFd),
            "read on a bad fd should fail."
        );
        assert_eq!(
            write(9999, b"x"),
            Err(FsError::InvalidFd),
            "write on a bad fd should fail."
        );
        assert_eq!(
            close(9999),
            Err(FsError::InvalidFd),
            "close on a bad fd should fail."
        );
    }
}
