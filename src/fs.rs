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

pub(crate) mod devfs;

use conquer_once::spin::OnceCell;
use spin::Mutex;

use crate::{
    drivers::block::ata::{AtaDisk, AtaDrive},
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
        crate::devices::register(crate::devices::Device {
            name: "disk",
            instance: 0,
            bus: crate::devices::Bus::Legacy,
            driver: "ata",
            details: crate::devices::Details::Block {
                partition_lba: part.lba_start,
                sectors: part.sector_count,
            },
        });
        OffsetDisk::new(raw, part.lba_start)
    }
    #[cfg(not(feature = "grub"))]
    {
        let mut disk = AtaDisk::secondary(AtaDrive::Master);
        let info = disk
            .identify()
            .expect("root disk identify failed")
            .expect("root disk absent");
        crate::devices::register(crate::devices::Device {
            name: "disk",
            instance: 0,
            bus: crate::devices::Bus::Legacy,
            driver: "ata",
            details: crate::devices::Details::Block {
                partition_lba: 0,
                sectors: info.sector_count,
            },
        });
        OffsetDisk::new(disk, 0)
    }
}

/// Runs a closure with the root filesystem locked.
pub fn with_fs<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut Fat32<OffsetDisk<AtaDisk>>) -> R,
{
    let fs = ROOT_FS.try_get().ok()?;
    // Syscalls copy paths/data into kernel-owned storage before taking this
    // service lock. PIO must not prevent other CPUs scheduling or acknowledging
    // user frames. Release the filesystem guard before reacquiring CPU entry.
    Some(crate::arch::x86_64::entry::without_gate(|| {
        let mut filesystem = fs.lock();
        f(&mut filesystem)
    }))
}

/// Namespace errors retain the underlying filesystem/device failure for diagnostics.
#[derive(Debug)]
pub(crate) enum Error {
    Disk(crate::fat32::FatError),
    Device(devfs::Error),
    Unmounted,
}
impl core::fmt::Display for Error {
    fn fmt(&self, out: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Disk(error) => write!(out, "FAT32: {:?}", error),
            Self::Device(error) => write!(out, "devfs: {:?}", error),
            Self::Unmounted => out.write_str("root filesystem not mounted"),
        }
    }
}
impl From<devfs::Error> for Error {
    fn from(error: devfs::Error) -> Self {
        Self::Device(error)
    }
}

/// Reserve /dev for kernel devices, including aliases accepted by path normalization.
pub(crate) fn is_device_path(path: &str) -> Result<bool, Error> {
    Ok(devfs::Path::new(path)?.device().is_some())
}

/// List the virtual mount or merge its single mount point into the FAT root.
pub(crate) fn list_dir(path: &str) -> Result<alloc::vec::Vec<crate::fat32::DirEntry>, Error> {
    use crate::fat32::DirEntry;
    let path = devfs::Path::new(path)?;
    if let Some(device) = path.device() {
        if devfs::lookup(device)? != devfs::Node::Directory {
            return Err(devfs::Error::NotDirectory.into());
        }
        return Ok([
            *b"NULL       ",
            *b"ZERO       ",
            *b"RANDOM     ",
            *b"DEVICES    ",
        ]
        .into_iter()
        .map(|name| DirEntry {
            name,
            attr: 0x20,
            size: 0,
        })
        .collect());
    }
    let mut entries = with_fs(|fs| fs.list_dir(Some(path.as_str())))
        .ok_or(Error::Unmounted)?
        .map_err(Error::Disk)?;
    if path.as_str() == "/" {
        entries.retain(|entry| entry.name != *b"DEV        ");
        entries.push(DirEntry {
            name: *b"DEV        ",
            attr: 0x10,
            size: 0,
        });
    }
    Ok(entries)
}

/// Device files do not allocate disk entries; the virtual mount is immutable.
pub(crate) fn create_dir(path: &str) -> Result<(), Error> {
    let path = devfs::Path::new(path)?;
    if path.device().is_some() {
        return Err(devfs::Error::ReadOnly.into());
    }
    with_fs(|fs| fs.create_dir(path.as_str()))
        .ok_or(Error::Unmounted)?
        .map_err(Error::Disk)
}

/// Read an owned kernel buffer from a device endpoint or ordinary FAT file.
pub(crate) fn read_file(path: &str, out: &mut [u8]) -> Result<usize, Error> {
    let path = devfs::Path::new(path)?;
    if let Some(device) = path.device() {
        return devfs::read(
            devfs::lookup(device)?,
            out,
            |out| crate::devices::render(out),
            |bytes| crate::drivers::char::random::fill(bytes).is_ok(),
        )
        .map_err(Error::Device);
    }
    with_fs(|fs| fs.read_file(path.as_str(), out))
        .ok_or(Error::Unmounted)?
        .map_err(Error::Disk)
}

/// Writes to unknown device paths fail without falling through to FAT creation.
pub(crate) fn write_file(path: &str, bytes: &[u8]) -> Result<usize, Error> {
    let path = devfs::Path::new(path)?;
    if let Some(device) = path.device() {
        return devfs::write(devfs::lookup(device)?, bytes).map_err(Error::Device);
    }
    with_fs(|fs| {
        let _ = fs.create_file(path.as_str());
        fs.write_file(path.as_str(), bytes)
    })
    .ok_or(Error::Unmounted)?
    .map_err(Error::Disk)
}
