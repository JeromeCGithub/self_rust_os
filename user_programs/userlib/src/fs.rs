//! Owned regular files over the descriptor ABI. Drop closes the descriptor.
//! Open flags and seek origins use Linux values; syscall numbers remain native.
use crate::sys;
/// Read-only access.
pub const O_RDONLY: u64 = 0;
/// Write-only access.
pub const O_WRONLY: u64 = 1;
/// Read/write access.
pub const O_RDWR: u64 = 2;
/// Create a missing file; parent directories must already exist.
pub const O_CREAT: u64 = 0o100;
/// Truncate an existing file (requires write access).
pub const O_TRUNC: u64 = 0o1000;
/// Seek relative to the beginning.
pub const SEEK_SET: u64 = 0;
/// Seek relative to the current position.
pub const SEEK_CUR: u64 = 1;
/// Seek relative to the current file size.
pub const SEEK_END: u64 = 2;

/// A descriptor owned by this process. No implicit copying or inheritance.
pub struct File {
    descriptor: u64,
}
impl File {
    /// Open a regular file with the documented O_* flag subset.
    pub fn open(path: &str, flags: u64) -> Result<Self, ()> {
        sys::open(path, flags).map(|descriptor| Self { descriptor })
    }
    /// Read up to the buffer length, returning zero at EOF.
    pub fn read(&mut self, bytes: &mut [u8]) -> Result<usize, ()> {
        let n = sys::read(self.descriptor, bytes);
        if n as u64 == sys::SYSCALL_ERROR {
            Err(())
        } else {
            Ok(n)
        }
    }
    /// Write up to the buffer length at the current offset; may be short.
    pub fn write(&mut self, bytes: &[u8]) -> Result<usize, ()> {
        sys::write(self.descriptor, bytes)
    }
    /// Retry short writes until all bytes are written; zero progress is an error.
    pub fn write_all(&mut self, mut bytes: &[u8]) -> Result<(), ()> {
        while !bytes.is_empty() {
            let n = self.write(bytes)?;
            if n == 0 {
                return Err(());
            }
            bytes = &bytes[n..];
        }
        Ok(())
    }
    /// Reposition the byte offset. A seek beyond EOF does not extend the file.
    pub fn seek(&mut self, offset: i64, origin: u64) -> Result<u64, ()> {
        sys::lseek(self.descriptor, offset, origin)
    }
}
impl Drop for File {
    fn drop(&mut self) {
        let _ = sys::close(self.descriptor);
    }
}

#[cfg(feature = "alloc")]
#[path = "fs/directory.rs"]
mod directory;
#[cfg(feature = "alloc")]
pub use directory::DirectoryEntry;

/// Read a complete directory listing, including VFAT long names. Retries bounded
/// size races; no partially copied record is returned to callers.
#[cfg(feature = "alloc")]
pub fn read_dir(path: &str) -> Result<alloc::vec::Vec<DirectoryEntry>, ()> {
    read_dir_bounded(path, 16 * 1024 * 1024, 16 * 1024 * 1024 / 24)
}
/// Read a complete listing within explicit wire-byte and record budgets.
/// The byte budget is checked before allocation and copy; an oversized listing
/// fails instead of returning a prefix. Names and result vectors use fallible allocation.
#[cfg(feature = "alloc")]
pub fn read_dir_bounded(
    path: &str,
    max_bytes: usize,
    max_entries: usize,
) -> Result<alloc::vec::Vec<DirectoryEntry>, ()> {
    let path = sys::path_buf(path).ok_or(())?;
    directory::read_with(max_bytes, max_entries, |output| {
        let (address, length) = output
            .map(|bytes| (bytes.as_mut_ptr() as u64, bytes.len() as u64))
            .unwrap_or((0, 0));
        // SAFETY: The complete live NUL path remains readable during the call.
        // Query writes nothing. Copy advertises only the uniquely borrowed,
        // initialized writable output and no reference escapes this syscall.
        let count = unsafe { sys::syscall(33, path.as_ptr() as u64, address, length) };
        if count == sys::SYSCALL_ERROR {
            Err(())
        } else {
            usize::try_from(count).map_err(|_| ())
        }
    })
}
