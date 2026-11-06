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

/// Directory metadata with its full UTF-8 name.
#[cfg(feature = "alloc")]
pub struct DirectoryEntry {
    /// Preserved Unicode long name, or uppercase short name.
    pub name: alloc::string::String,
    /// FAT directory attribute.
    pub is_directory: bool,
    /// File length in bytes.
    pub size: u64,
}
/// Read a complete directory listing, including VFAT long names. Retries bounded
/// size races; no partially copied record is returned to callers.
#[cfg(feature = "alloc")]
pub fn read_dir(path: &str) -> Result<alloc::vec::Vec<DirectoryEntry>, ()> {
    use alloc::{string::ToString, vec::Vec};
    let path = sys::path_buf(path).ok_or(())?;
    for _ in 0..4 {
        // SAFETY: Query reads the complete live NUL path and writes no output.
        let size = unsafe { sys::syscall(33, path.as_ptr() as u64, 0, 0) };
        if size == sys::SYSCALL_ERROR || size > 16 * 1024 * 1024 {
            return Err(());
        }
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(size as usize).map_err(|_| ())?;
        bytes.resize(size as usize, 0);
        // SAFETY: All advertised output is initialized, uniquely borrowed and
        // writable for the call. The kernel publishes only complete records.
        let count =
            unsafe { sys::syscall(33, path.as_ptr() as u64, bytes.as_mut_ptr() as u64, size) };
        if count == sys::SYSCALL_ERROR {
            continue;
        }
        if count > size {
            return Err(());
        }
        let mut position = 0;
        let mut entries = Vec::new();
        while position < count as usize {
            let header = bytes.get(position..position + 16).ok_or(())?;
            let length = u16::from_le_bytes([header[0], header[1]]) as usize;
            let name_length = u16::from_le_bytes([header[2], header[3]]) as usize;
            if length < 17
                || length % 8 != 0
                || 16 + name_length >= length
                || position + length > count as usize
            {
                return Err(());
            }
            let name = core::str::from_utf8(
                bytes
                    .get(position + 16..position + 16 + name_length)
                    .ok_or(())?,
            )
            .map_err(|_| ())?
            .to_string();
            if bytes[position + 16 + name_length] != 0 {
                return Err(());
            }
            entries.push(DirectoryEntry {
                name,
                is_directory: header[4] & 16 != 0,
                size: u64::from_le_bytes(header[8..16].try_into().map_err(|_| ())?),
            });
            position += length;
        }
        return Ok(entries);
    }
    Err(())
}
