//! Filesystem management for syscalls.
//!
//! Mounts the FAT32 filesystem at boot and provides access to it for
//! filesystem syscalls.
//!
//! - Legacy boot: the filesystem lives on the secondary ATA master (a
//!   separate data disk attached for tests).
//! - GRUB boot (`grub` feature): the kernel itself was loaded from a FAT32
//!   partition on the boot disk (primary master), so the root filesystem is
//!   that partition, located via the MBR partition table.

use conquer_once::spin::OnceCell;
use spin::Mutex;

use crate::{
    ata::{AtaDisk, AtaDrive},
    fat32::Fat32,
    mbr::OffsetDisk,
};

/// The mounted root filesystem, initialized by [`init`].
static ROOT_FS: OnceCell<Mutex<Fat32<OffsetDisk<AtaDisk>>>> = OnceCell::uninit();

/// Mounts the root FAT32 filesystem.
///
/// # Panics
///
/// Panics if the filesystem cannot be opened or if called more than once.
pub fn init() {
    let disk = open_root_disk();
    let fs = Fat32::open(disk).expect("failed to open root filesystem");
    ROOT_FS
        .try_init_once(|| Mutex::new(fs))
        .expect("fs::init must be called exactly once");
}

/// Opens the disk (or disk partition) holding the root filesystem.
fn open_root_disk() -> OffsetDisk<AtaDisk> {
    #[cfg(feature = "grub")]
    {
        // GRUB loaded the kernel from the first FAT32 partition of the boot
        // disk (primary master): mount that same partition.
        let mut raw = AtaDisk::primary(AtaDrive::Master);
        let partitions =
            crate::mbr::read_partition_table(&mut raw).expect("no MBR partition table");
        let part = crate::mbr::find_first_fat32(&partitions).expect("no FAT32 partition");
        crate::serial_println!(
            "[fs] mounting FAT32 partition: lba_start={} sectors={}",
            part.lba_start,
            part.sector_count
        );
        OffsetDisk::new(raw, part.lba_start)
    }
    #[cfg(not(feature = "grub"))]
    {
        OffsetDisk::new(AtaDisk::secondary(AtaDrive::Master), 0)
    }
}

/// Runs a closure with the root filesystem locked.
pub fn with_fs<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut Fat32<OffsetDisk<AtaDisk>>) -> R,
{
    ROOT_FS.try_get().ok().map(|fs| f(&mut fs.lock()))
}
