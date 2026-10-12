//! Filesystem management for syscalls.
//!
//! Mounts the FAT32 filesystem on the secondary ATA master at boot and
//! provides access to it for filesystem syscalls.

use conquer_once::spin::OnceCell;
use spin::Mutex;

use crate::{
    ata::{AtaDisk, AtaDrive},
    fat32::Fat32,
};

/// The mounted root filesystem, initialized by [`init`].
static ROOT_FS: OnceCell<Mutex<Fat32<AtaDisk>>> = OnceCell::uninit();

/// Mounts the FAT32 filesystem on the secondary master ATA disk.
///
/// # Panics
/// Panics if the filesystem cannot be opened or if called more than once.
pub fn init() {
    let disk = AtaDisk::secondary(AtaDrive::Master);
    let fs = Fat32::open(disk).expect("failed to open root filesystem");
    ROOT_FS
        .try_init_once(|| Mutex::new(fs))
        .expect("fs::init must be called exactly once");
}

/// Runs a closure with the root filesystem locked.
pub fn with_fs<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut Fat32<AtaDisk>) -> R,
{
    ROOT_FS.try_get().ok().map(|fs| f(&mut fs.lock()))
}
