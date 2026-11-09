//! Linux poll(2), kept to one audited FFI boundary; see docs/initator-host.md.
use std::{
    ffi::{c_int, c_short, c_ulong},
    io,
};
pub const READ: c_short = 0x001;
pub const WRITE: c_short = 0x004;
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Fd {
    pub fd: c_int,
    pub events: c_short,
    pub returned: c_short,
}
unsafe extern "C" {
    fn poll(fds: *mut Fd, count: c_ulong, timeout: c_int) -> c_int;
}
pub fn wait(fds: &mut [Fd], timeout: i32) -> io::Result<()> {
    let count =
        c_ulong::try_from(fds.len()).map_err(|_| io::Error::other("poll descriptor limit"))?;
    // SAFETY: Fd matches Linux's repr(C) pollfd (int, short, short). The complete
    // initialized slice is uniquely borrowed and writable for count elements; it
    // remains live and unmoved for this synchronous call. Its owned descriptors
    // cannot be closed by another thread during wait. poll retains no pointers.
    let result = unsafe { poll(fds.as_mut_ptr(), count, timeout) };
    if result < 0 {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
    Ok(())
}
