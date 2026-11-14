//! FAT32 filesystem.
//!
//! A small, self-contained FAT32 implementation used as the on-disk filesystem.
//! It is generic over a [`BlockDevice`] so the logic can be unit-tested
//! against a RAM-backed mock and used with the real [`AtaDisk`](crate::drivers::block::ata::AtaDisk)
//! in production.
//!
//! # Scope (v1)
//!
//! - Formats disks, creates/reads/writes/deletes files, creates directories,
//!   and resolves `/`-separated paths (`/home/Desktop/note.txt`).
//! - 8.3 aliases and validated VFAT Unicode names for files and directories.
//! - Single-threaded; the caller must serialize access.
//!
//! # Example
//!
//! ```no_run
//! use self_rust_os::drivers::block::ata::{AtaDisk, AtaDrive};
//! use self_rust_os::fat32::Fat32;
//!
//! let disk = AtaDisk::secondary(AtaDrive::Master);
//! let mut fs = Fat32::format(disk, 32768, b"RUSTOS     ").expect("format failed");
//! fs.create_dir("/home/Desktop").expect("mkdir failed");
//! fs.create_file("/home/Desktop/HELLO.TXT").expect("create failed");
//! fs.write_file("/home/Desktop/HELLO.TXT", b"Hello, disk!").expect("write failed");
//!
//! let mut buf = [0u8; 64];
//! let n = fs.read_file("/home/Desktop/HELLO.TXT", &mut buf).expect("read failed");
//! assert_eq!(&buf[..n], b"Hello, disk!");
//! ```
//!
//! # Implementation notes
//!
//! Sector/cluster arithmetic intentionally uses truncating integer division
//! and `as` casts: all values are sector or cluster indices bounded by the
//! 32-bit LBA space, so truncation cannot occur in practice.

#![allow(
    clippy::integer_division,
    reason = "Sector/cluster math uses truncating division by design"
)]
#![allow(
    clippy::cast_possible_truncation,
    reason = "LBA/cluster indices fit in u32 by construction"
)]

use alloc::{string::String, vec::Vec};
use core::{cmp, fmt::Debug};
mod lfn;

use crate::drivers::block::ata::{AtaDisk, AtaError, SECTOR_SIZE};

/// A 512-byte sector addressable block device.
pub trait BlockDevice {
    /// The error type returned by I/O operations.
    type Error: Debug;

    /// Reads the sector at `lba` into `buf`.
    ///
    /// # Errors
    ///
    /// Returns the device's error type on I/O failure.
    fn read_sector(&mut self, lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), Self::Error>;

    /// Read consecutive complete sectors. Return the number transferred; a
    /// short count reports an unrepresentable address without wrapping it.
    /// Devices may override this to submit one bounded multi-sector command.
    fn read_sectors(
        &mut self,
        lba: u32,
        buf: &mut [[u8; SECTOR_SIZE]],
    ) -> Result<usize, Self::Error> {
        for (index, sector) in buf.iter_mut().enumerate() {
            let Some(current) = u32::try_from(index).ok().and_then(|i| lba.checked_add(i)) else {
                return Ok(index);
            };
            self.read_sector(current, sector)?;
        }
        Ok(buf.len())
    }

    /// Writes `buf` to the sector at `lba`.
    ///
    /// # Errors
    ///
    /// Returns the device's error type on I/O failure.
    fn write_sector(&mut self, lba: u32, buf: &[u8; 512]) -> Result<(), Self::Error>;
}

impl BlockDevice for AtaDisk {
    type Error = AtaError;

    fn read_sector(&mut self, lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), Self::Error> {
        Self::read_sector(self, lba, buf)
    }

    fn read_sectors(
        &mut self,
        lba: u32,
        buf: &mut [[u8; SECTOR_SIZE]],
    ) -> Result<usize, Self::Error> {
        Self::read_sectors(self, lba, buf)
    }

    fn write_sector(&mut self, lba: u32, buf: &[u8; 512]) -> Result<(), Self::Error> {
        Self::write_sector(self, lba, buf)
    }
}

/// Errors that can occur during filesystem operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FatError {
    /// The underlying block device reported an I/O error.
    DeviceError,
    /// Sector 0 does not have the `0xAA55` boot signature.
    InvalidSignature,
    /// The volume is not FAT32 (bad filesystem type string or zero FAT size).
    NotFat32,
    /// The filename is not valid 8.3 format.
    InvalidName,
    /// The file was not found.
    FileNotFound,
    /// A path component that should be a directory is not one.
    NotADirectory,
    /// A file operation targeted a directory.
    IsADirectory,
    /// A file with this name already exists.
    FileExists,
    /// No free clusters left on the disk.
    DiskFull,
    /// No free directory entries left.
    DirFull,
    /// The read buffer is smaller than the file.
    BufferTooSmall,
    /// A syscall caller lacks authority for this resolved filesystem object.
    AccessDenied,
}

/// Opaque directory identity for the single boot-mounted volume. No userspace
/// rename, directory deletion or remount API can recycle these identities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DirectoryKey(u32);

/// A directory listing entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    /// 8.3 name (`b"DESKTOP     "`).
    pub name: [u8; 11],
    /// Validated VFAT Unicode name, absent for legacy short-only entries.
    pub long_name: Option<String>,
    /// FAT attribute byte (`0x10` = directory, `0x20` = archive/file).
    pub attr: u8,
    /// File size in bytes (0 for directories).
    pub size: u32,
}

impl DirEntry {
    /// Preserved Unicode long name, or the conventional short-name spelling.
    pub fn file_name(&self) -> String {
        self.long_name
            .clone()
            .unwrap_or_else(|| short_name(&self.name))
    }
    /// Returns `true` if this entry is a directory.
    #[must_use]
    pub fn is_dir(&self) -> bool {
        self.attr & ATTR_DIRECTORY != 0
    }
}

// Layout constants for the volumes created by `format`.
const SECTORS_PER_CLUSTER: u8 = 4;
const RESERVED_SECTORS: u16 = 32;
const NUM_FATS: u8 = 2;
const ROOT_CLUSTER: u32 = 2;
const FSINFO_SECTOR: u32 = 1;
const BACKUP_BOOT_SECTOR: u32 = 6;
const DIR_ENTRY_SIZE: usize = 32;

// FAT32 cluster chain markers.
const FAT_FREE: u32 = 0x0000_0000;
const FAT_EOC: u32 = 0x0FFF_FFFF;
const FAT_EOC_MIN: u32 = 0x0FFF_FFF8;
const FAT_MASK: u32 = 0x0FFF_FFFF;

// Directory entry attribute bits.
const ATTR_ARCHIVE: u8 = 0x20;
const ATTR_DIRECTORY: u8 = 0x10;
/// Directory entry attribute: volume label (not a file).
const ATTR_VOLUME_LABEL: u8 = 0x08;

// Directory entry slot markers.
const ENTRY_FREE: u8 = 0xE5;
const ENTRY_END: u8 = 0x00;

/// A FAT32 filesystem on a block device.
#[derive(Debug)]
pub struct Fat32<B> {
    device: B,
    sectors_per_cluster: u8,
    reserved_sectors: u16,
    num_fats: u8,
    fat_size_sectors: u32,
    root_cluster: u32,
    first_data_sector: u32,
    total_sectors: u32,
    // The FAT sector holds 128 adjacent chain links. Keep it across data reads,
    // under the filesystem service lock; writes invalidate before device I/O.
    fat_cache: Option<(u32, [u8; SECTOR_SIZE])>,
    next_free_cluster: u32,
}

impl<B: BlockDevice> Fat32<B> {
    /// Formats the device as FAT32 and returns the mounted filesystem.
    ///
    /// `total_sectors` is the size of the device in 512-byte sectors and
    /// `volume_label` is an 11-byte, space-padded label (e.g. `b"RUSTOS     "`).
    ///
    /// # Errors
    ///
    /// Returns [`FatError::DeviceError`] if the underlying I/O fails.
    pub fn format(
        device: B,
        total_sectors: u32,
        volume_label: &[u8; 11],
    ) -> Result<Self, FatError> {
        let sectors_per_cluster = SECTORS_PER_CLUSTER;
        let reserved_sectors = RESERVED_SECTORS;
        let num_fats = NUM_FATS;

        let data_sectors = total_sectors - u32::from(reserved_sectors);
        let cluster_count = data_sectors / u32::from(sectors_per_cluster);
        // One FAT entry per cluster plus the two reserved entries, 4 bytes each.
        let fat_size_sectors = ((cluster_count + 2) * 4).div_ceil(SECTOR_SIZE as u32);
        let first_data_sector =
            u32::from(reserved_sectors) + u32::from(num_fats) * fat_size_sectors;

        let mut fs = Self {
            device,
            sectors_per_cluster,
            reserved_sectors,
            num_fats,
            fat_size_sectors,
            root_cluster: ROOT_CLUSTER,
            first_data_sector,
            total_sectors,
            fat_cache: None,
            next_free_cluster: 2,
        };

        // --- Boot sector (sector 0) ---
        let mut boot = [0_u8; SECTOR_SIZE];
        boot[0] = 0xEB;
        boot[1] = 0x58;
        boot[2] = 0x90;
        boot[3..11].copy_from_slice(b"RUSTOS  ");
        boot[11..13].copy_from_slice(&(SECTOR_SIZE as u16).to_le_bytes());
        boot[13] = sectors_per_cluster;
        boot[14..16].copy_from_slice(&reserved_sectors.to_le_bytes());
        boot[16] = num_fats;
        // root_entry_count (17..19) stays 0 for FAT32.
        // total_sectors_16 (19..21) stays 0.
        boot[21] = 0xF8; // media descriptor: hard disk
                         // fat_size_16 (22..24) stays 0 for FAT32.
        boot[24..26].copy_from_slice(&32_u16.to_le_bytes()); // sectors per track
        boot[26..28].copy_from_slice(&64_u16.to_le_bytes()); // number of heads
                                                             // hidden_sectors (28..32) stays 0.
        boot[32..36].copy_from_slice(&total_sectors.to_le_bytes());
        boot[36..40].copy_from_slice(&fat_size_sectors.to_le_bytes());
        // ext_flags (40..42), fs_version (42..44) stay 0.
        boot[44..48].copy_from_slice(&ROOT_CLUSTER.to_le_bytes());
        boot[48..50].copy_from_slice(&(FSINFO_SECTOR as u16).to_le_bytes());
        boot[50..52].copy_from_slice(&(BACKUP_BOOT_SECTOR as u16).to_le_bytes());
        boot[64] = 0x80; // drive number
        boot[66] = 0x29; // boot signature
        boot[67..71].copy_from_slice(&0x1234_5678_u32.to_le_bytes()); // volume id
        boot[71..82].copy_from_slice(volume_label);
        boot[82..90].copy_from_slice(b"FAT32   ");
        boot[510] = 0x55;
        boot[511] = 0xAA;
        fs.write_sector_raw(0, &boot)?;

        // --- FSInfo sector ---
        let mut fsinfo = [0_u8; SECTOR_SIZE];
        fsinfo[0..4].copy_from_slice(&0x5252_6141_u32.to_le_bytes());
        fsinfo[484..488].copy_from_slice(&0x7272_4161_u32.to_le_bytes());
        fsinfo[488..492].copy_from_slice(&0xFFFF_FFFF_u32.to_le_bytes()); // free count unknown
        fsinfo[492..496].copy_from_slice(&0xFFFF_FFFF_u32.to_le_bytes()); // next free unknown
        fsinfo[508..512].copy_from_slice(&0xAA55_0000_u32.to_le_bytes());
        fs.write_sector_raw(FSINFO_SECTOR, &fsinfo)?;

        // --- Backup boot sector ---
        fs.write_sector_raw(BACKUP_BOOT_SECTOR, &boot)?;

        // --- FATs: zero them, then set the reserved entries ---
        let zero = [0_u8; SECTOR_SIZE];
        for i in 0..fs.fat_size_sectors {
            for f in 0..fs.num_fats {
                let lba = u32::from(fs.reserved_sectors) + u32::from(f) * fs.fat_size_sectors + i;
                fs.write_sector_raw(lba, &zero)?;
            }
        }
        // Entry 0: media descriptor; entry 1: EOC; entry 2 (root dir): EOC.
        fs.write_fat_entry_raw(0, 0x0FFF_FFF8)?;
        fs.write_fat_entry_raw(1, FAT_EOC)?;
        fs.write_fat_entry_raw(ROOT_CLUSTER, FAT_EOC)?;

        // --- Zero the root directory cluster ---
        fs.zero_cluster(ROOT_CLUSTER)?;

        Ok(fs)
    }

    /// Opens an existing FAT32 volume.
    ///
    /// # Errors
    ///
    /// Returns [`FatError::InvalidSignature`] when sector 0 lacks the boot
    /// signature, [`FatError::NotFat32`] when the BPB does not describe FAT32,
    /// or [`FatError::DeviceError`] on I/O failure.
    pub fn open(mut device: B) -> Result<Self, FatError> {
        let mut boot = [0_u8; SECTOR_SIZE];
        device
            .read_sector(0, &mut boot)
            .map_err(|_| FatError::DeviceError)?;

        if boot[510] != 0x55 || boot[511] != 0xAA {
            return Err(FatError::InvalidSignature);
        }
        if &boot[82..90] != b"FAT32   " {
            return Err(FatError::NotFat32);
        }

        let bytes_per_sector = u16::from_le_bytes([boot[11], boot[12]]);
        if bytes_per_sector != SECTOR_SIZE as u16 {
            return Err(FatError::NotFat32);
        }
        let fat_size_sectors = u32::from_le_bytes([boot[36], boot[37], boot[38], boot[39]]);
        if fat_size_sectors == 0 {
            return Err(FatError::NotFat32);
        }

        let sectors_per_cluster = boot[13];
        let reserved_sectors = u16::from_le_bytes([boot[14], boot[15]]);
        let num_fats = boot[16];
        let total_sectors_16 = u16::from_le_bytes([boot[19], boot[20]]);
        let total_sectors_32 = u32::from_le_bytes([boot[32], boot[33], boot[34], boot[35]]);
        // Per the FAT spec, when the 32-bit total sector count is zero the
        // 16-bit field holds the volume size. Small volumes formatted by
        // external tools (e.g. `mformat` on a 16 MiB image) put the count
        // there and leave the 32-bit field at 0; without this fallback
        // `total_sectors` is 0 and `allocate_cluster` underflows.
        let total_sectors = if total_sectors_32 != 0 {
            total_sectors_32
        } else {
            u32::from(total_sectors_16)
        };
        let root_cluster = u32::from_le_bytes([boot[44], boot[45], boot[46], boot[47]]);
        let first_data_sector =
            u32::from(reserved_sectors) + u32::from(num_fats) * fat_size_sectors;

        Ok(Self {
            device,
            sectors_per_cluster,
            reserved_sectors,
            num_fats,
            fat_size_sectors,
            root_cluster,
            first_data_sector,
            total_sectors,
            fat_cache: None,
            next_free_cluster: 2,
        })
    }

    /// Returns the underlying block device, consuming the filesystem handle.
    #[must_use]
    pub fn into_device(self) -> B {
        self.device
    }

    /// Returns the total size of the volume in bytes.
    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        u64::from(self.total_sectors) * SECTOR_SIZE as u64
    }

    /// Creates an empty file at `path` (e.g. `"/home/Desktop/HELLO.TXT"`).
    ///
    /// Parent directories must already exist (see [`create_dir`](Self::create_dir)).
    /// A plain name like `"HELLO.TXT"` refers to the root directory.
    ///
    /// # Errors
    ///
    /// Returns [`FatError::InvalidName`] for bad names, [`FatError::FileExists`]
    /// if the file exists, [`FatError::FileNotFound`] if a parent directory
    /// is missing, [`FatError::DirFull`] if the directory is full.
    pub fn create_file(&mut self, path: &str) -> Result<(), FatError> {
        let (dir_cluster, name_83) = self.resolve_parent(path)?;
        if self.find_entry_in(dir_cluster, &name_83)?.is_some() {
            return Err(FatError::FileExists);
        }
        let mut entry = [0; DIR_ENTRY_SIZE];
        entry[11] = ATTR_ARCHIVE;
        self.install_name(dir_cluster, &name_83, entry)?;
        Ok(())
    }

    /// Resolve a normalized absolute path's first component against a reserved
    /// root entry. Long names and 8.3 aliases compare by actual entry location;
    /// the reserved spelling also matches if the entry has not been created.
    /// Callers must serialize this check with the protected mutation.
    pub(crate) fn in_root_entry(&mut self, path: &str, root: &str) -> Result<bool, FatError> {
        if !path.starts_with('/') || !lfn::valid(root) {
            return Err(FatError::InvalidName);
        }
        let mut components = path.split('/').filter(|part| !part.is_empty());
        let Some(first) = components.next() else {
            return Ok(false);
        };
        if !lfn::valid(first) || !components.all(lfn::valid) {
            return Err(FatError::InvalidName);
        }
        if name_equal(first, root) {
            return Ok(true);
        }
        let Some(reserved) = self.find_entry_in(self.root_cluster, root)? else {
            return Ok(false);
        };
        Ok(self.find_entry_in(self.root_cluster, first)? == Some(reserved))
    }

    /// Creates a directory at `path`, creating missing parents (`mkdir -p`).
    ///
    /// Each component is a validated short or VFAT name (e.g. `"/home/Desktop"`).
    /// Creating a directory that already exists is a no-op returning `Ok`.
    ///
    /// # Errors
    ///
    /// Returns [`FatError::InvalidName`] for bad names,
    /// [`FatError::NotADirectory`] if an existing path component is a file,
    /// [`FatError::DiskFull`] / [`FatError::DirFull`] when out of space.
    pub fn create_dir(&mut self, path: &str) -> Result<(), FatError> {
        let components = path.split('/').filter(|component| !component.is_empty());
        if components.clone().next().is_none() || !components.clone().all(lfn::valid) {
            return Err(FatError::InvalidName);
        }
        let mut dir_cluster = self.root_cluster;
        for name_83 in components {
            match self.find_entry_in(dir_cluster, name_83)? {
                Some((c, idx)) => {
                    let mut entry = [0_u8; DIR_ENTRY_SIZE];
                    self.read_dir_entry(c, idx, &mut entry)?;
                    if entry[11] & ATTR_DIRECTORY == 0 {
                        return Err(FatError::NotADirectory);
                    }
                    dir_cluster = self.dir_entry_cluster(c, idx)?;
                }
                None => {
                    dir_cluster = self.make_dir(dir_cluster, name_83)?;
                }
            }
        }
        Ok(())
    }

    /// Creates a single directory `name_83` inside `parent_cluster`.
    ///
    /// Allocates a cluster, writes the `.` / `..` entries, and links the new
    /// directory into its parent. Returns the new directory's cluster.
    fn make_dir(&mut self, parent_cluster: u32, name_83: &str) -> Result<u32, FatError> {
        // allocate_cluster zeroes the new cluster: entries 2.. are ENTRY_END.
        let cluster = self.allocate_cluster()?;

        let result = (|| {
            // `.` points at the new directory itself.
            let mut dot = [0_u8; DIR_ENTRY_SIZE];
            dot[0..11].copy_from_slice(b".          ");
            dot[11] = ATTR_DIRECTORY;
            set_entry_cluster(&mut dot, cluster);
            self.write_dir_entry(cluster, 0, &dot)?;

            // `..` points at the parent.
            let mut dotdot = [0_u8; DIR_ENTRY_SIZE];
            dotdot[0..11].copy_from_slice(b"..         ");
            dotdot[11] = ATTR_DIRECTORY;
            set_entry_cluster(&mut dotdot, parent_cluster);
            self.write_dir_entry(cluster, 1, &dotdot)?;

            let mut entry = [0; DIR_ENTRY_SIZE];
            entry[11] = ATTR_DIRECTORY;
            set_entry_cluster(&mut entry, cluster);
            self.install_name(parent_cluster, name_83, entry)?;
            Ok(cluster)
        })();
        if result.is_err() {
            // Every post-allocation initialization/publication error owns this
            // unpublished cluster. Metadata rollback requires no new storage.
            let _ = self.free_chain(cluster);
        }
        result
    }

    /// Writes `data` to the file at `path`, replacing its previous contents.
    ///
    /// The file must exist (see [`create_file`](Self::create_file)). Returns
    /// the number of bytes written.
    ///
    /// # Errors
    ///
    /// Returns [`FatError::FileNotFound`], [`FatError::IsADirectory`],
    /// [`FatError::DiskFull`], or [`FatError::DeviceError`].
    pub fn write_file(&mut self, path: &str, data: &[u8]) -> Result<usize, FatError> {
        let (dir_cluster, name_83) = self.resolve_parent(path)?;
        let (entry_cluster, dir_index) = self
            .find_entry_in(dir_cluster, &name_83)?
            .ok_or(FatError::FileNotFound)?;
        self.ensure_not_dir(entry_cluster, dir_index)?;

        // Build a separate replacement chain. Publish only after every byte is
        // stored, so allocation failure leaves the old file and size intact.
        let old_start = self.dir_entry_cluster(entry_cluster, dir_index)?;
        let size = u32::try_from(data.len()).map_err(|_| FatError::DiskFull)?;
        let bytes_per_cluster = usize::from(self.sectors_per_cluster) * SECTOR_SIZE;
        let mut first_cluster = 0_u32;
        let mut prev_cluster = 0_u32;
        let replacement = (|| {
            for chunk in data.chunks(bytes_per_cluster) {
                let cluster = self.allocate_cluster()?;
                if first_cluster == 0 {
                    first_cluster = cluster;
                } else if let Err(error) = self.write_fat_entry(prev_cluster, cluster) {
                    let _ = self.free_chain(cluster);
                    return Err(error);
                }
                prev_cluster = cluster;
                // Allocation already zeroed the full cluster. write_cluster
                // pads a partial last sector; untouched sectors remain zero.
                self.write_cluster(cluster, chunk)?;
            }
            let mut entry = [0_u8; DIR_ENTRY_SIZE];
            self.read_dir_entry(entry_cluster, dir_index, &mut entry)?;
            set_entry_cluster(&mut entry, first_cluster);
            entry[28..32].copy_from_slice(&size.to_le_bytes());
            self.write_dir_entry(entry_cluster, dir_index, &entry)
        })();
        if let Err(error) = replacement {
            if first_cluster != 0 {
                // Device failure can also prevent rollback; propagate the
                // original error. Disk-full rollback requires no new storage.
                let _ = self.free_chain(first_cluster);
            }
            return Err(error);
        }
        if old_start != 0 {
            self.free_chain(old_start)?;
        }
        let written = data.len();

        Ok(written)
    }

    /// Query file size without allocating or reading its contents.
    /// Returns errors for directories, absent files, invalid paths and I/O failure.
    pub fn file_size(&mut self, path: &str) -> Result<usize, FatError> {
        let (directory, name) = self.resolve_parent(path)?;
        let (cluster, index) = self
            .find_entry_in(directory, &name)?
            .ok_or(FatError::FileNotFound)?;
        self.ensure_not_dir(cluster, index)?;
        let mut entry = [0; DIR_ENTRY_SIZE];
        self.read_dir_entry(cluster, index, &mut entry)?;
        Ok(u32::from_le_bytes([entry[28], entry[29], entry[30], entry[31]]) as usize)
    }

    /// Reads the file at `path` into `buf`, returning bytes read.
    ///
    /// # Errors
    /// Returns FileNotFound, IsADirectory, BufferTooSmall, or an I/O error.
    pub fn read_file(&mut self, path: &str, buf: &mut [u8]) -> Result<usize, FatError> {
        let (dir_cluster, name_83) = self.resolve_parent(path)?;
        let (entry_cluster, dir_index) = self
            .find_entry_in(dir_cluster, &name_83)?
            .ok_or(FatError::FileNotFound)?;
        self.ensure_not_dir(entry_cluster, dir_index)?;

        let mut entry = [0_u8; DIR_ENTRY_SIZE];
        self.read_dir_entry(entry_cluster, dir_index, &mut entry)?;
        let size = u32::from_le_bytes([entry[28], entry[29], entry[30], entry[31]]) as usize;
        if buf.len() < size {
            return Err(FatError::BufferTooSmall);
        }

        let first_cluster = self.dir_entry_cluster(entry_cluster, dir_index)?;
        if size == 0 || first_cluster == 0 {
            return Ok(0);
        }

        let bytes_per_cluster = usize::from(self.sectors_per_cluster) * SECTOR_SIZE;
        let mut cluster = first_cluster;
        let mut read = 0_usize;
        while read < size {
            let take = cmp::min(size - read, bytes_per_cluster);
            self.read_cluster(cluster, &mut buf[read..read + take])?;
            read += take;
            if read < size {
                cluster = self.read_fat_entry(cluster)?;
                if cluster >= FAT_EOC_MIN {
                    return Err(FatError::DeviceError);
                }
            }
        }
        Ok(read)
    }

    /// Deletes the file at `path`, freeing its clusters and directory entry.
    ///
    /// # Errors
    ///
    /// Returns [`FatError::FileNotFound`] if the file does not exist,
    /// [`FatError::IsADirectory`] if the path is a directory.
    pub fn delete_file(&mut self, path: &str) -> Result<(), FatError> {
        let (dir_cluster, name_83) = self.resolve_parent(path)?;
        let (entry_cluster, dir_index) = self
            .find_entry_in(dir_cluster, &name_83)?
            .ok_or(FatError::FileNotFound)?;
        self.ensure_not_dir(entry_cluster, dir_index)?;

        let start = self.dir_entry_cluster(entry_cluster, dir_index)?;
        if start != 0 {
            self.free_chain(start)?;
        }

        let mut linked = [(0u32, 0usize); 20];
        let linked_count = self
            .scan_directory(dir_cluster, |cluster, index, _, _, slots| {
                if cluster == entry_cluster && index == dir_index {
                    linked[..slots.len()].copy_from_slice(slots);
                    Some(slots.len())
                } else {
                    None
                }
            })?
            .unwrap_or_default();
        for (cluster, index) in linked[..linked_count]
            .iter()
            .copied()
            .chain(core::iter::once((entry_cluster, dir_index)))
        {
            let mut entry = [0; 32];
            self.read_dir_entry(cluster, index, &mut entry)?;
            entry[0] = ENTRY_FREE;
            self.write_dir_entry(cluster, index, &entry)?;
        }
        Ok(())
    }

    /// Validate an existing directory without allocating its entry listing.
    pub fn check_directory(&mut self, path: &str) -> Result<(), FatError> {
        self.directory_cluster(path).map(|_| ())
    }

    pub(crate) fn directory_key(&mut self, path: &str) -> Result<DirectoryKey, FatError> {
        if !path.starts_with('/') {
            return Err(FatError::AccessDenied);
        }
        let directory = self.directory_cluster(path)?;
        self.checked_directory_key(directory)
    }
    pub(crate) fn file_parent_key(&mut self, path: &str) -> Result<DirectoryKey, FatError> {
        if !path.starts_with('/') {
            return Err(FatError::AccessDenied);
        }
        let (directory, _) = self.resolve_parent(path)?;
        self.checked_directory_key(directory)
    }
    /// Last existing directory on a validated recursive-create path.
    pub(crate) fn creation_parent_key(&mut self, path: &str) -> Result<DirectoryKey, FatError> {
        if !path.starts_with('/') {
            return Err(FatError::AccessDenied);
        }
        let components = path.split('/').filter(|part| !part.is_empty());
        if !components.clone().all(lfn::valid) {
            return Err(FatError::InvalidName);
        }
        let mut directory = self.root_cluster;
        for component in components {
            let Some((cluster, index)) = self.find_entry_in(directory, component)? else {
                break;
            };
            let mut entry = [0; 32];
            self.read_dir_entry(cluster, index, &mut entry)?;
            if entry[11] & ATTR_DIRECTORY == 0 {
                break;
            }
            directory = self.dir_entry_cluster(cluster, index)?;
        }
        self.checked_directory_key(directory)
    }
    fn checked_directory_key(&mut self, cluster: u32) -> Result<DirectoryKey, FatError> {
        let key = DirectoryKey(cluster);
        if !self.directory_contains(DirectoryKey(self.root_cluster), key)? {
            return Err(FatError::DeviceError);
        }
        Ok(key)
    }
    /// Follow validated parent links rather than comparing path spellings.
    pub(crate) fn directory_contains(
        &mut self,
        root: DirectoryKey,
        target: DirectoryKey,
    ) -> Result<bool, FatError> {
        let limit =
            (self.total_sectors - self.first_data_sector) / u32::from(self.sectors_per_cluster);
        let valid = |cluster| cluster >= 2 && cluster < limit + 2;
        if !valid(root.0) || !valid(target.0) {
            return Err(FatError::DeviceError);
        }
        let mut current = target.0;
        for _ in 0..limit {
            if current == root.0 {
                return Ok(true);
            }
            if current == self.root_cluster {
                return Ok(false);
            }
            let mut parent = [0; 32];
            self.read_dir_entry(current, 1, &mut parent)?;
            if parent[..11] != *b"..         " || parent[11] & ATTR_DIRECTORY == 0 {
                return Err(FatError::DeviceError);
            }
            let high = u32::from(u16::from_le_bytes([parent[20], parent[21]]));
            let low = u32::from(u16::from_le_bytes([parent[26], parent[27]]));
            let next = (high << 16) | low;
            let next = if next == 0 { self.root_cluster } else { next };
            if !valid(next) || next == current {
                return Err(FatError::DeviceError);
            }
            current = next;
        }
        Err(FatError::DeviceError)
    }

    fn directory_cluster(&mut self, path: &str) -> Result<u32, FatError> {
        if path.is_empty() || path == "/" {
            return Ok(self.root_cluster);
        }
        let components = path.split('/').filter(|component| !component.is_empty());
        if components.clone().next().is_none() || !components.clone().all(lfn::valid) {
            return Err(FatError::InvalidName);
        }
        let mut directory = self.root_cluster;
        for component in components {
            let (cluster, index) = self
                .find_entry_in(directory, &component)?
                .ok_or(FatError::FileNotFound)?;
            let mut entry = [0; DIR_ENTRY_SIZE];
            self.read_dir_entry(cluster, index, &mut entry)?;
            if entry[11] & ATTR_DIRECTORY == 0 {
                return Err(FatError::NotADirectory);
            }
            directory = self.dir_entry_cluster(cluster, index)?;
        }
        Ok(directory)
    }

    /// Write at a byte offset without replacing unaffected clusters.
    pub fn write_at(&mut self, path: &str, offset: usize, data: &[u8]) -> Result<usize, FatError> {
        let (directory, name) = self.resolve_parent(path)?;
        let (entry_cluster, index) = self
            .find_entry_in(directory, &name)?
            .ok_or(FatError::FileNotFound)?;
        self.ensure_not_dir(entry_cluster, index)?;
        if data.is_empty() {
            return Ok(0);
        }
        let end = offset
            .checked_add(data.len())
            .filter(|&n| n <= u32::MAX as usize)
            .ok_or(FatError::DiskFull)?;
        let mut entry = [0; DIR_ENTRY_SIZE];
        self.read_dir_entry(entry_cluster, index, &mut entry)?;
        let old_size = u32::from_le_bytes(
            entry[28..32]
                .try_into()
                .map_err(|_| FatError::DeviceError)?,
        ) as usize;
        let bytes_per_cluster = usize::from(self.sectors_per_cluster) * SECTOR_SIZE;
        let cluster_count =
            (self.total_sectors - self.first_data_sector) / u32::from(self.sectors_per_cluster);
        let needed = end.max(old_size).div_ceil(bytes_per_cluster);
        if needed > cluster_count as usize {
            return Err(FatError::DiskFull);
        }
        let first = self.dir_entry_cluster(entry_cluster, index)?;
        let mut cluster = first;
        let mut old_count = 0usize;
        let mut old_tail = 0;
        // Validate the complete old chain before mutation. Keep only its length
        // and tail: per-file cluster vectors would bypass native memory budgets.
        while cluster != 0 && cluster < FAT_EOC_MIN {
            if cluster < 2 || cluster >= cluster_count + 2 || old_count >= cluster_count as usize {
                return Err(FatError::DeviceError);
            }
            old_count += 1;
            old_tail = cluster;
            cluster = self.read_fat_entry(cluster)?;
        }
        if old_count < old_size.div_ceil(bytes_per_cluster) {
            return Err(FatError::DeviceError);
        }
        let mut extra = 0;
        let mut extra_tail = 0;
        let allocation = (|| {
            for _ in old_count..needed {
                let cluster = self.allocate_cluster()?;
                if extra == 0 {
                    extra = cluster;
                } else if let Err(error) = self.write_fat_entry(extra_tail, cluster) {
                    let _ = self.free_chain(cluster);
                    return Err(error);
                }
                extra_tail = cluster;
            }
            Ok(())
        })();
        if let Err(error) = allocation {
            if extra != 0 {
                let _ = self.free_chain(extra);
            }
            return Err(error);
        }
        let result = (|| {
            let start = offset.min(old_size);
            let first_block = start / bytes_per_cluster;
            let (mut cluster, skip) = if first_block < old_count {
                (first, first_block)
            } else {
                (extra, first_block - old_count)
            };
            for _ in 0..skip {
                cluster = self.read_fat_entry(cluster)?;
            }
            let mut sector = [0; SECTOR_SIZE];
            for block in first_block..end.div_ceil(bytes_per_cluster) {
                let base = block * bytes_per_cluster;
                for sector_index in 0..usize::from(self.sectors_per_cluster) {
                    let sector_base = base + sector_index * SECTOR_SIZE;
                    let sector_end = sector_base + SECTOR_SIZE;
                    if sector_end <= start || sector_base >= end {
                        continue;
                    }
                    self.read_cluster_at(cluster, sector_index * SECTOR_SIZE, &mut sector)?;
                    let zero_start = old_size.max(sector_base);
                    let zero_end = offset.min(sector_end);
                    if zero_start < zero_end {
                        sector[zero_start - sector_base..zero_end - sector_base].fill(0);
                    }
                    let from = offset.max(sector_base);
                    let to = end.min(sector_end);
                    if from < to {
                        sector[from - sector_base..to - sector_base]
                            .copy_from_slice(&data[from - offset..to - offset]);
                    }
                    self.write_sector_raw(
                        self.cluster_to_lba(cluster) + sector_index as u32,
                        &sector,
                    )?;
                }
                if block + 1 < end.div_ceil(bytes_per_cluster) {
                    cluster = if block + 1 == old_count {
                        extra
                    } else {
                        self.read_fat_entry(cluster)?
                    };
                }
            }
            if extra != 0 && old_count != 0 {
                self.write_fat_entry(old_tail, extra)?;
            }
            set_entry_cluster(&mut entry, if old_count == 0 { extra } else { first });
            entry[28..32].copy_from_slice(&(old_size.max(end) as u32).to_le_bytes());
            self.write_dir_entry(entry_cluster, index, &entry)
        })();
        if let Err(error) = result {
            if extra != 0 {
                if old_count != 0 {
                    let _ = self.write_fat_entry(old_tail, FAT_EOC);
                }
                let _ = self.free_chain(extra);
            }
            return Err(error);
        }
        Ok(data.len())
    }

    /// Read at most the buffer length starting at a byte offset; zero at EOF.
    /// Unlike read_file, a buffer smaller than the file is supported.
    pub fn read_at(
        &mut self,
        path: &str,
        offset: usize,
        buf: &mut [u8],
    ) -> Result<usize, FatError> {
        let (directory, name) = self.resolve_parent(path)?;
        let (entry_cluster, index) = self
            .find_entry_in(directory, &name)?
            .ok_or(FatError::FileNotFound)?;
        self.ensure_not_dir(entry_cluster, index)?;
        let mut entry = [0; DIR_ENTRY_SIZE];
        self.read_dir_entry(entry_cluster, index, &mut entry)?;
        let size = u32::from_le_bytes([entry[28], entry[29], entry[30], entry[31]]) as usize;
        if offset >= size || buf.is_empty() {
            return Ok(0);
        }
        let count = buf.len().min(size - offset);
        let bytes_per_cluster = usize::from(self.sectors_per_cluster) * SECTOR_SIZE;
        let cluster_count =
            (self.total_sectors - self.first_data_sector) / u32::from(self.sectors_per_cluster);
        let mut cluster = self.dir_entry_cluster(entry_cluster, index)?;
        let valid = |cluster| cluster >= 2 && cluster < cluster_count + 2;
        for _ in 0..offset / bytes_per_cluster {
            if !valid(cluster) {
                return Err(FatError::DeviceError);
            }
            cluster = self.read_fat_entry(cluster)?;
        }
        let mut within = offset % bytes_per_cluster;
        let mut read = 0;
        while read < count {
            if !valid(cluster) {
                return Err(FatError::DeviceError);
            }
            let take = (count - read).min(bytes_per_cluster - within);
            self.read_cluster_at(cluster, within, &mut buf[read..read + take])?;
            read += take;
            within = 0;
            if read < count {
                cluster = self.read_fat_entry(cluster)?;
            }
        }
        Ok(read)
    }

    /// Lists the entries in a directory.
    ///
    /// `path` selects the directory: `None`, `""`, or `"/"` lists the root;
    /// otherwise the path must resolve to an existing directory
    /// (e.g. `"/home/Desktop"`). The `.` and `..` entries are omitted.
    ///
    /// # Errors
    ///
    /// Returns [`FatError::FileNotFound`], [`FatError::NotADirectory`],
    /// or [`FatError::DeviceError`].
    pub fn list_dir(&mut self, path: Option<&str>) -> Result<Vec<DirEntry>, FatError> {
        let directory = self.directory_cluster(path.unwrap_or("/"))?;
        let mut entries = Vec::new();
        self.scan_directory(directory, |_, _, entry, long, _| {
            let mut name = [0; 11];
            name.copy_from_slice(&entry[..11]);
            if name != *b".          " && name != *b"..         " {
                entries.push(DirEntry {
                    name,
                    long_name: long.map(String::from),
                    attr: entry[11],
                    size: u32::from_le_bytes(entry[28..32].try_into().expect("directory size")),
                });
            }
            None::<()>
        })?;
        Ok(entries)
    }

    /// Visit a complete directory with bounded stack scratch and borrowed names.
    /// The callback runs under the caller's filesystem lock and must not acquire
    /// scheduler locks or retain names. A callback error stops the scan.
    pub fn visit_dir(
        &mut self,
        path: &str,
        mut visit: impl FnMut(&[u8; 11], &str, u8, u32) -> Result<(), FatError>,
    ) -> Result<(), FatError> {
        let directory = self.directory_cluster(path)?;
        self.scan_directory(directory, |_, _, entry, long, _| {
            let mut alias = [0; 11];
            alias.copy_from_slice(&entry[..11]);
            if alias == *b".          " || alias == *b"..         " {
                return None;
            }
            let mut buffer = [0; 23];
            let name = long.unwrap_or_else(|| short_name_into(&alias, &mut buffer));
            visit(
                &alias,
                name,
                entry[11],
                u32::from_le_bytes(entry[28..32].try_into().expect("directory size")),
            )
            .err()
        })?
        .map_or(Ok(()), Err)
    }

    /// Returns an error if the directory entry is a directory.
    fn ensure_not_dir(&mut self, dir_cluster: u32, index: usize) -> Result<(), FatError> {
        let mut entry = [0_u8; DIR_ENTRY_SIZE];
        self.read_dir_entry(dir_cluster, index, &mut entry)?;
        if entry[11] & ATTR_DIRECTORY != 0 {
            return Err(FatError::IsADirectory);
        }
        Ok(())
    }

    /// Resolves `path` to its parent directory cluster and borrowed final name.
    ///
    /// Intermediate components must exist and be directories. A plain name
    /// like `"HELLO.TXT"` (or `"/HELLO.TXT"`) resolves to the root directory.
    fn resolve_parent<'p>(&mut self, path: &'p str) -> Result<(u32, &'p str), FatError> {
        let mut components = path.split('/').filter(|component| !component.is_empty());
        // Validate the complete path before any lookup, preserving error ordering
        // without allocating a vector or copying component/name strings.
        if !components.clone().all(lfn::valid) {
            return Err(FatError::InvalidName);
        }
        let name = components.next_back().ok_or(FatError::InvalidName)?;
        let mut dir_cluster = self.root_cluster;
        for parent in components {
            let (c, idx) = self
                .find_entry_in(dir_cluster, parent)?
                .ok_or(FatError::FileNotFound)?;
            let mut entry = [0_u8; DIR_ENTRY_SIZE];
            self.read_dir_entry(c, idx, &mut entry)?;
            if entry[11] & ATTR_DIRECTORY == 0 {
                return Err(FatError::NotADirectory);
            }
            dir_cluster = self.dir_entry_cluster(c, idx)?;
        }
        Ok((dir_cluster, name))
    }

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    fn read_sector_raw(&mut self, lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), FatError> {
        self.device
            .read_sector(lba, buf)
            .map_err(|_| FatError::DeviceError)
    }

    fn write_sector_raw(&mut self, lba: u32, buf: &[u8; 512]) -> Result<(), FatError> {
        if self.fat_cache.as_ref().map(|(cached, _)| *cached) == Some(lba) {
            // Even an error may have partially reached the device. Never retain
            // an earlier version of metadata after attempting an overwrite.
            self.fat_cache = None;
        }
        self.device
            .write_sector(lba, buf)
            .map_err(|_| FatError::DeviceError)
    }

    /// Converts a cluster number to its first sector LBA.
    #[must_use]
    fn cluster_to_lba(&self, cluster: u32) -> u32 {
        self.first_data_sector + (cluster - 2) * u32::from(self.sectors_per_cluster)
    }

    fn read_cluster(&mut self, cluster: u32, buf: &mut [u8]) -> Result<(), FatError> {
        self.read_cluster_at(cluster, 0, buf)
    }

    /// Read a cluster subrange into owned caller storage. Only the first/last
    /// partial sectors need scratch; aligned middle sectors retain bulk I/O.
    fn read_cluster_at(
        &mut self,
        cluster: u32,
        offset_bytes: usize,
        buf: &mut [u8],
    ) -> Result<(), FatError> {
        let cluster_bytes = usize::from(self.sectors_per_cluster) * SECTOR_SIZE;
        if offset_bytes
            .checked_add(buf.len())
            .filter(|&end| end <= cluster_bytes)
            .is_none()
        {
            return Err(FatError::DeviceError);
        }
        let mut lba = cluster
            .checked_sub(2)
            .and_then(|c| c.checked_mul(u32::from(self.sectors_per_cluster)))
            .and_then(|offset| self.first_data_sector.checked_add(offset))
            .and_then(|base| base.checked_add((offset_bytes / SECTOR_SIZE) as u32))
            .ok_or(FatError::DeviceError)?;
        let within_sector = offset_bytes % SECTOR_SIZE;
        // The checked cluster bound above also bounds this addition and count.
        let sector_count = (within_sector + buf.len()).div_ceil(SECTOR_SIZE) as u32;
        if lba
            .checked_add(sector_count)
            .filter(|end| *end <= self.total_sectors)
            .is_none()
        {
            return Err(FatError::DeviceError);
        }
        if buf.is_empty() {
            return Ok(());
        }
        let head_bytes = if within_sector == 0 {
            0
        } else {
            buf.len().min(SECTOR_SIZE - within_sector)
        };
        if head_bytes != 0 {
            let mut sector = [0; SECTOR_SIZE];
            self.read_sector_raw(lba, &mut sector)?;
            buf[..head_bytes].copy_from_slice(&sector[within_sector..within_sector + head_bytes]);
            lba += 1;
        }
        let remaining = &mut buf[head_bytes..];
        let count = remaining.len() / SECTOR_SIZE;
        let (full, tail) = remaining.split_at_mut(count * SECTOR_SIZE);
        // SAFETY: Arrays of u8 have alignment 1 and exactly 512 bytes. `full`
        // is an exclusive, initialized, exactly divisible slice of the owned
        // caller buffer; count bounds every array. This reborrow ends before
        // the disjoint tail is accessed, and no reference escapes device I/O.
        let sectors = unsafe {
            core::slice::from_raw_parts_mut(full.as_mut_ptr().cast::<[u8; SECTOR_SIZE]>(), count)
        };
        if self
            .device
            .read_sectors(lba, sectors)
            .map_err(|_| FatError::DeviceError)?
            != count
        {
            return Err(FatError::DeviceError);
        }
        if !tail.is_empty() {
            let mut sector = [0; SECTOR_SIZE];
            self.read_sector_raw(lba + count as u32, &mut sector)?;
            tail.copy_from_slice(&sector[..tail.len()]);
        }
        Ok(())
    }

    fn write_cluster(&mut self, cluster: u32, buf: &[u8]) -> Result<(), FatError> {
        let lba = self.cluster_to_lba(cluster);
        for (i, chunk) in buf.chunks(SECTOR_SIZE).enumerate() {
            let mut sector = [0_u8; SECTOR_SIZE];
            sector[..chunk.len()].copy_from_slice(chunk);
            self.write_sector_raw(lba + i as u32, &sector)?;
        }
        Ok(())
    }

    fn zero_cluster(&mut self, cluster: u32) -> Result<(), FatError> {
        let lba = cluster
            .checked_sub(2)
            .and_then(|c| c.checked_mul(u32::from(self.sectors_per_cluster)))
            .and_then(|offset| self.first_data_sector.checked_add(offset))
            .ok_or(FatError::DeviceError)?;
        if lba
            .checked_add(u32::from(self.sectors_per_cluster))
            .filter(|end| *end <= self.total_sectors)
            .is_none()
        {
            return Err(FatError::DeviceError);
        }
        let zeros = [0; SECTOR_SIZE];
        for sector in 0..u32::from(self.sectors_per_cluster) {
            self.write_sector_raw(lba + sector, &zeros)?;
        }
        Ok(())
    }

    /// Reads a FAT entry, masking to 28 bits.
    fn read_fat_entry(&mut self, cluster: u32) -> Result<u32, FatError> {
        let fat_offset = cluster * 4;
        let sector = u32::from(self.reserved_sectors) + fat_offset / SECTOR_SIZE as u32;
        let entry_offset = (fat_offset % SECTOR_SIZE as u32) as usize;
        if self.fat_cache.as_ref().map(|(cached, _)| *cached) != Some(sector) {
            let mut buf = [0_u8; SECTOR_SIZE];
            self.read_sector_raw(sector, &mut buf)?;
            self.fat_cache = Some((sector, buf));
        }
        let buf = &self.fat_cache.as_ref().expect("loaded FAT sector").1;
        Ok(u32::from_le_bytes([
            buf[entry_offset],
            buf[entry_offset + 1],
            buf[entry_offset + 2],
            buf[entry_offset + 3],
        ]) & FAT_MASK)
    }

    /// Writes a FAT entry to all FAT copies.
    fn write_fat_entry(&mut self, cluster: u32, value: u32) -> Result<(), FatError> {
        let fat_offset = cluster * 4;
        let sector_in_fat = fat_offset / SECTOR_SIZE as u32;
        let entry_offset = (fat_offset % SECTOR_SIZE as u32) as usize;
        for f in 0..self.num_fats {
            let lba = u32::from(self.reserved_sectors)
                + u32::from(f) * self.fat_size_sectors
                + sector_in_fat;
            let mut buf = [0_u8; SECTOR_SIZE];
            self.read_sector_raw(lba, &mut buf)?;
            buf[entry_offset..entry_offset + 4].copy_from_slice(&(value & FAT_MASK).to_le_bytes());
            self.write_sector_raw(lba, &buf)?;
        }
        Ok(())
    }

    /// Raw variant used during `format`, before `self` is fully built.
    fn write_fat_entry_raw(&mut self, cluster: u32, value: u32) -> Result<(), FatError> {
        self.write_fat_entry(cluster, value)
    }

    /// Finds a free cluster, marks it end-of-chain, zeroes it, and returns it.
    fn allocate_cluster(&mut self) -> Result<u32, FatError> {
        let cluster_count =
            (self.total_sectors - self.first_data_sector) / u32::from(self.sectors_per_cluster);
        let hint = self.next_free_cluster.clamp(2, cluster_count + 1);
        for cluster in (hint..cluster_count + 2).chain(2..hint) {
            if self.read_fat_entry(cluster)? == FAT_FREE {
                self.write_fat_entry(cluster, FAT_EOC)?;
                if let Err(error) = self.zero_cluster(cluster) {
                    let _ = self.write_fat_entry(cluster, FAT_FREE);
                    return Err(error);
                }
                self.next_free_cluster = if cluster + 1 < cluster_count + 2 {
                    cluster + 1
                } else {
                    2
                };
                return Ok(cluster);
            }
        }
        Err(FatError::DiskFull)
    }

    /// Releases a cluster chain back to the free pool.
    fn free_chain(&mut self, start: u32) -> Result<(), FatError> {
        let limit =
            (self.total_sectors - self.first_data_sector) / u32::from(self.sectors_per_cluster);
        self.next_free_cluster = self.next_free_cluster.min(start);
        let mut cluster = start;
        for _ in 0..limit {
            if cluster < 2 || cluster >= limit + 2 {
                return Err(FatError::DeviceError);
            }
            let next = self.read_fat_entry(cluster)?;
            self.write_fat_entry(cluster, FAT_FREE)?;
            if next >= FAT_EOC_MIN {
                return Ok(());
            }
            cluster = next;
        }
        Err(FatError::DeviceError)
    }

    /// LBA of the sector containing directory entry `index` in `cluster`.
    fn dir_entry_lba(&self, cluster: u32, index: usize) -> u32 {
        let entries_per_sector = SECTOR_SIZE / DIR_ENTRY_SIZE;
        self.cluster_to_lba(cluster) + (index / entries_per_sector) as u32
    }

    fn read_dir_entry(
        &mut self,
        cluster: u32,
        index: usize,
        out: &mut [u8; DIR_ENTRY_SIZE],
    ) -> Result<(), FatError> {
        let lba = self.dir_entry_lba(cluster, index);
        let offset = (index % (SECTOR_SIZE / DIR_ENTRY_SIZE)) * DIR_ENTRY_SIZE;
        let mut sector = [0_u8; SECTOR_SIZE];
        self.read_sector_raw(lba, &mut sector)?;
        out.copy_from_slice(&sector[offset..offset + DIR_ENTRY_SIZE]);
        Ok(())
    }

    fn write_dir_entry(
        &mut self,
        cluster: u32,
        index: usize,
        entry: &[u8; DIR_ENTRY_SIZE],
    ) -> Result<(), FatError> {
        let lba = self.dir_entry_lba(cluster, index);
        let offset = (index % (SECTOR_SIZE / DIR_ENTRY_SIZE)) * DIR_ENTRY_SIZE;
        let mut sector = [0_u8; SECTOR_SIZE];
        self.read_sector_raw(lba, &mut sector)?;
        sector[offset..offset + DIR_ENTRY_SIZE].copy_from_slice(entry);
        self.write_sector_raw(lba, &sector)?;
        Ok(())
    }

    /// Returns the first cluster of a directory entry.
    fn dir_entry_cluster(&mut self, cluster: u32, index: usize) -> Result<u32, FatError> {
        let mut entry = [0_u8; DIR_ENTRY_SIZE];
        self.read_dir_entry(cluster, index, &mut entry)?;
        let hi = u32::from(u16::from_le_bytes([entry[20], entry[21]]));
        let lo = u32::from(u16::from_le_bytes([entry[26], entry[27]]));
        Ok((hi << 16) | lo)
    }

    /// Finds a directory entry by 8.3 name within `dir_cluster`;
    /// returns `(entry_cluster, index)`.
    fn entries_per_cluster(&self) -> usize {
        usize::from(self.sectors_per_cluster) * SECTOR_SIZE / DIR_ENTRY_SIZE
    }
    fn scan_directory<R>(
        &mut self,
        directory: u32,
        mut visit: impl FnMut(u32, usize, &[u8; 32], Option<&str>, &[(u32, usize)]) -> Option<R>,
    ) -> Result<Option<R>, FatError> {
        let mut cluster = directory;
        let mut decoder = lfn::Decoder::new();
        let mut positions = [(0u32, 0usize); 20];
        let mut position_count = 0;
        let mut name_buffer = [0u8; 765];
        let limit =
            (self.total_sectors - self.first_data_sector) / u32::from(self.sectors_per_cluster);
        for _ in 0..limit {
            if cluster < 2 || cluster >= limit + 2 {
                return Err(FatError::DeviceError);
            }
            let mut sector = [0; SECTOR_SIZE];
            let slots_per_sector = SECTOR_SIZE / DIR_ENTRY_SIZE;
            for index in 0..self.entries_per_cluster() {
                if index % slots_per_sector == 0 {
                    self.read_sector_raw(self.dir_entry_lba(cluster, index), &mut sector)?;
                }
                let offset = (index % slots_per_sector) * DIR_ENTRY_SIZE;
                let mut entry = [0; DIR_ENTRY_SIZE];
                entry.copy_from_slice(&sector[offset..offset + DIR_ENTRY_SIZE]);
                if entry[0] == ENTRY_END {
                    return Ok(None);
                }
                if entry[0] == ENTRY_FREE {
                    decoder.reset();
                    position_count = 0;
                    continue;
                }
                if entry[11] == 15 {
                    if entry[0] & 0x40 != 0 {
                        position_count = 0;
                    }
                    // A valid VFAT sequence has at most twenty linked slots;
                    // feed rejects extra/out-of-order slots before indexing.
                    if decoder.feed(&entry) {
                        positions[position_count] = (cluster, index);
                        position_count += 1;
                    } else {
                        position_count = 0;
                    }
                    continue;
                }
                let mut alias = [0; 11];
                alias.copy_from_slice(&entry[..11]);
                let long = decoder.finish_into(&alias, &mut name_buffer);
                if entry[11] & ATTR_VOLUME_LABEL == 0 {
                    let linked = if long.is_some() {
                        &positions[..position_count]
                    } else {
                        &[]
                    };
                    if let Some(result) = visit(cluster, index, &entry, long, linked) {
                        return Ok(Some(result));
                    }
                }
                position_count = 0;
            }
            let next = self.read_fat_entry(cluster)?;
            if next >= FAT_EOC_MIN {
                return Ok(None);
            }
            cluster = next;
        }
        Err(FatError::DeviceError)
    }
    fn find_entry_in(
        &mut self,
        directory: u32,
        name: &str,
    ) -> Result<Option<(u32, usize)>, FatError> {
        self.scan_directory(directory, |cluster, index, entry, long, _| {
            let mut alias = [0; 11];
            alias.copy_from_slice(&entry[..11]);
            let mut buffer = [0; 23];
            let short = short_name_into(&alias, &mut buffer);
            if name_equal(name, short) || long.is_some_and(|long| name_equal(name, long)) {
                Some((cluster, index))
            } else {
                None
            }
        })
    }
    fn alias(&mut self, directory: u32, name: &str) -> Result<[u8; 11], FatError> {
        if let Ok(alias) = to_83(name) {
            if self.find_short_entry_in(directory, &alias)?.is_none() {
                return Ok(alias);
            }
        }
        let (base, extension) = name
            .rsplit_once('.')
            .filter(|(base, _)| !base.is_empty())
            .unwrap_or((name, ""));
        let clean = |text: &str, output: &mut [u8]| {
            let mut length = 0;
            for character in text
                .chars()
                .filter(|&c| c != ' ' && c != '.')
                .take(output.len())
            {
                let upper = character.to_ascii_uppercase();
                output[length] = if upper.is_ascii() && check_83_char(upper as u8).is_ok() {
                    upper as u8
                } else {
                    b'_'
                };
                length += 1;
            }
            length
        };
        let mut base_bytes = [0; 8];
        let base_length = clean(base, &mut base_bytes);
        let mut extension_bytes = [0; 3];
        let extension_length = clean(extension, &mut extension_bytes);
        for number in 1u32..=999999 {
            let mut suffix = [0; 7];
            let mut start = suffix.len();
            let mut value = number;
            loop {
                start -= 1;
                suffix[start] = b'0' + (value % 10) as u8;
                value /= 10;
                if value == 0 {
                    break;
                }
            }
            start -= 1;
            suffix[start] = b'~';
            let suffix = &suffix[start..];
            let mut alias = [b' '; 11];
            let prefix = (8 - suffix.len()).min(base_length);
            if prefix == 0 {
                alias[0] = b'_';
            } else {
                alias[..prefix].copy_from_slice(&base_bytes[..prefix]);
            }
            let prefix = prefix.max(1);
            alias[prefix..prefix + suffix.len()].copy_from_slice(suffix);
            alias[8..8 + extension_length].copy_from_slice(&extension_bytes[..extension_length]);
            if self.find_short_entry_in(directory, &alias)?.is_none() {
                return Ok(alias);
            }
        }
        Err(FatError::DirFull)
    }
    fn reserve_name_slots(
        &mut self,
        directory: u32,
        needed: usize,
    ) -> Result<([(u32, usize); 21], bool, u32, u32), FatError> {
        if !(1..=21).contains(&needed) {
            return Err(FatError::InvalidName);
        }
        let mut slots = [(0u32, 0usize); 21];
        let mut count = 0;
        let mut cluster = directory;
        let mut end = false;
        let limit =
            (self.total_sectors - self.first_data_sector) / u32::from(self.sectors_per_cluster);
        for _ in 0..limit {
            if cluster < 2 || cluster >= limit + 2 {
                return Err(FatError::DeviceError);
            }
            for index in 0..self.entries_per_cluster() {
                let mut entry = [0; 32];
                self.read_dir_entry(cluster, index, &mut entry)?;
                end |= entry[0] == ENTRY_END;
                if end || entry[0] == ENTRY_FREE {
                    slots[count] = (cluster, index);
                    count += 1;
                    if count == needed {
                        return Ok((slots, end, 0, 0));
                    }
                } else {
                    count = 0;
                }
            }
            let next = self.read_fat_entry(cluster)?;
            if next >= FAT_EOC_MIN {
                let tail = cluster;
                let mut head = 0;
                let mut previous = 0;
                while count < needed {
                    let new = match self.allocate_cluster() {
                        Ok(cluster) => cluster,
                        Err(error) => {
                            if head != 0 {
                                let _ = self.free_chain(head);
                            }
                            return Err(error);
                        }
                    };
                    if head == 0 {
                        head = new;
                    } else if let Err(error) = self.write_fat_entry(previous, new) {
                        let _ = self.free_chain(new);
                        let _ = self.free_chain(head);
                        return Err(error);
                    }
                    previous = new;
                    for index in 0..self.entries_per_cluster() {
                        slots[count] = (new, index);
                        count += 1;
                        if count == needed {
                            break;
                        }
                    }
                }
                return Ok((slots, true, tail, head));
            }
            cluster = next;
        }
        Err(FatError::DeviceError)
    }
    fn install_name(
        &mut self,
        directory: u32,
        name: &str,
        mut short: [u8; 32],
    ) -> Result<(), FatError> {
        let alias = self.alias(directory, name)?;
        short[..11].copy_from_slice(&alias);
        let mut records = [[0; 32]; 21];
        let count = if to_83(name).is_ok() {
            0
        } else {
            lfn::encode_into(name, &alias, &mut records[..20]).map_err(|_| FatError::InvalidName)?
        };
        records[count] = short;
        let records = &records[..count + 1];
        let (slots, end, tail, head) = self.reserve_name_slots(directory, records.len())?;
        let slots = &slots[..records.len()];
        let result = (|| {
            for (&(cluster, index), record) in slots.iter().zip(records) {
                self.write_dir_entry(cluster, index, record)?;
            }
            if end {
                let (cluster, index) = *slots.last().ok_or(FatError::DeviceError)?;
                if index + 1 < self.entries_per_cluster() {
                    self.write_dir_entry(cluster, index + 1, &[0; 32])?;
                } else {
                    let next = self.read_fat_entry(cluster)?;
                    if next < FAT_EOC_MIN {
                        self.write_dir_entry(next, 0, &[0; 32])?;
                    }
                }
            }
            if head != 0 {
                self.write_fat_entry(tail, head)?;
            }
            Ok(())
        })();
        if result.is_err() {
            if head != 0 {
                let _ = self.write_fat_entry(tail, FAT_EOC);
                let _ = self.free_chain(head);
            }
            for (position, &(cluster, index)) in slots.iter().enumerate() {
                if head != 0 && cluster == head {
                    break;
                }
                let mut entry = [0; 32];
                entry[0] = if end && position == 0 {
                    ENTRY_END
                } else {
                    ENTRY_FREE
                };
                let _ = self.write_dir_entry(cluster, index, &entry);
            }
        }
        result
    }

    fn find_short_entry_in(
        &mut self,
        dir_cluster: u32,
        name_83: &[u8; 11],
    ) -> Result<Option<(u32, usize)>, FatError> {
        let mut cluster = dir_cluster;
        let limit =
            (self.total_sectors - self.first_data_sector) / u32::from(self.sectors_per_cluster);
        for _ in 0..limit {
            if cluster < 2 || cluster >= limit + 2 {
                return Err(FatError::DeviceError);
            }
            for index in 0..self.entries_per_cluster() {
                let mut entry = [0_u8; DIR_ENTRY_SIZE];
                self.read_dir_entry(cluster, index, &mut entry)?;
                if entry[0] == ENTRY_END {
                    return Ok(None);
                }
                if entry[0] == ENTRY_FREE {
                    continue;
                }
                if entry[11] & 0x08 != 0 {
                    continue; // volume label entry
                }
                if entry[0..11] == name_83[..] {
                    return Ok(Some((cluster, index)));
                }
            }
            let next = self.read_fat_entry(cluster)?;
            if next >= FAT_EOC_MIN {
                return Ok(None);
            }
            cluster = next;
        }
        Err(FatError::DeviceError)
    }
}

/// Writes a cluster number into a raw directory entry's cluster fields.
fn set_entry_cluster(entry: &mut [u8; DIR_ENTRY_SIZE], cluster: u32) {
    entry[20..22].copy_from_slice(&((cluster >> 16) as u16).to_le_bytes());
    entry[26..28].copy_from_slice(&(cluster as u16).to_le_bytes());
}

/// Converts `"NAME.EXT"` to 8.3 format (`b"NAME    EXT"`).
///
/// The name part may be up to 8 characters and the extension up to 3;
/// characters are uppercased. Returns [`FatError::InvalidName`] for empty
/// names, overlong parts, or disallowed characters.
fn to_83(name: &str) -> Result<[u8; 11], FatError> {
    let mut out = [b' '; 11];
    let mut parts = name.split('.');
    let base = parts.next().ok_or(FatError::InvalidName)?;
    let ext = parts.next().unwrap_or("");
    if parts.next().is_some() {
        return Err(FatError::InvalidName);
    }
    if base.is_empty() || base.len() > 8 || ext.len() > 3 {
        return Err(FatError::InvalidName);
    }
    for (i, c) in base.bytes().enumerate() {
        out[i] = check_83_char(c)?;
    }
    for (i, c) in ext.bytes().enumerate() {
        out[8 + i] = check_83_char(c)?;
    }
    Ok(out)
}

/// Validates and uppercases one 8.3 filename character.
const fn check_83_char(c: u8) -> Result<u8, FatError> {
    let upper = c.to_ascii_uppercase();
    match upper {
        b'A'..=b'Z'
        | b'0'..=b'9'
        | b'!'
        | b'#'
        | b'$'
        | b'%'
        | b'&'
        | b'\''
        | b'('
        | b')'
        | b'-'
        | b'@'
        | b'^'
        | b'_'
        | b'`'
        | b'{'
        | b'}'
        | b'~' => Ok(upper),
        _ => Err(FatError::InvalidName),
    }
}

#[cfg(test)]
mod tests {

    use super::{to_83, BlockDevice, Fat32, FatError, SECTOR_SIZE};
    use alloc::{string::String, vec::Vec};

    /// RAM-backed block device for unit tests.
    #[derive(Debug)]
    struct MemDisk {
        data: Vec<u8>,
        reads: Vec<usize>,
        writes: Vec<usize>,
        fail_write_lba: Option<u32>,
        fail_write_nth: Option<(u32, usize)>,
        fail_read_lba: Option<u32>,
        heap_ceiling: Option<usize>,
    }

    impl MemDisk {
        fn new(sectors: u32) -> Self {
            Self {
                data: alloc::vec![0_u8; sectors as usize * SECTOR_SIZE],
                reads: alloc::vec![0; sectors as usize],
                writes: alloc::vec![0; sectors as usize],
                fail_write_lba: None,
                fail_write_nth: None,
                fail_read_lba: None,
                heap_ceiling: None,
            }
        }
    }

    impl BlockDevice for MemDisk {
        type Error = ();

        fn read_sector(
            &mut self,
            lba: u32,
            buf: &mut [u8; SECTOR_SIZE],
        ) -> Result<(), Self::Error> {
            if let Some(ceiling) = self.heap_ceiling {
                assert!(
                    crate::allocator::allocated_bytes() <= ceiling,
                    "filesystem metadata/read scratch must not allocate uncharged heap storage"
                );
            }
            if self.fail_read_lba == Some(lba) {
                return Err(());
            }
            self.reads[lba as usize] += 1;
            let start = lba as usize * SECTOR_SIZE;
            buf.copy_from_slice(&self.data[start..start + SECTOR_SIZE]);
            Ok(())
        }

        fn write_sector(&mut self, lba: u32, buf: &[u8; 512]) -> Result<(), Self::Error> {
            if let Some(ceiling) = self.heap_ceiling {
                assert!(
                    crate::allocator::allocated_bytes() <= ceiling,
                    "filesystem write scratch must not allocate uncharged heap storage"
                );
            }
            if self.fail_write_lba == Some(lba)
                || self.fail_write_nth == Some((lba, self.writes[lba as usize] + 1))
            {
                return Err(());
            }
            self.writes[lba as usize] += 1;
            let start = lba as usize * SECTOR_SIZE;
            self.data[start..start + SECTOR_SIZE].copy_from_slice(buf);
            Ok(())
        }
    }

    // Bound the disposable RAM volume and its per-sector I/O counters.
    const TEST_SECTORS: u32 = 512;

    #[test_case]
    fn filesystem_grants_resolve_aliases_and_keep_search_separate_from_listing() {
        use crate::fs::access::{Authority, Rights};
        let mut fs =
            Fat32::format(MemDisk::new(TEST_SECTORS), TEST_SECTORS, b"TESTVOLUME ").unwrap();
        fs.create_dir("/A long café home/child").unwrap();
        fs.create_dir("/OTHER").unwrap();
        let entries = fs.list_dir(None).unwrap();
        let alias = super::short_name(
            &entries
                .iter()
                .find(|entry| entry.long_name.as_deref() == Some("A long café home"))
                .unwrap()
                .name,
        );
        let alias_root = alloc::format!("/{alias}");
        let alias_file = alloc::format!("/{alias}/new");
        fs.device.heap_ceiling = Some(crate::allocator::allocated_bytes());
        let grants = Authority::System
            .narrow(&mut fs, &[("/A long café home", Rights::READ_WRITE)])
            .unwrap();
        assert_eq!(
            grants.file(&mut fs, &alias_file, Rights::READ_WRITE),
            Ok(())
        );
        assert_eq!(
            grants
                .narrow(&mut fs, &[(&alias_root, Rights::READ)])
                .unwrap()
                .file(&mut fs, "/A long café home/new", Rights::READ),
            Ok(())
        );
        assert_eq!(grants.search(&mut fs, "/"), Ok(()));
        assert_eq!(
            grants.directory(&mut fs, "/", Rights::READ),
            Err(FatError::AccessDenied)
        );
        assert_eq!(
            grants.recursive_create(&mut fs, "/A long café home/new/deep"),
            Ok(())
        );
        assert_eq!(
            grants.recursive_create(&mut fs, "/OTHER/new/deep"),
            Err(FatError::AccessDenied)
        );
        assert!(grants
            .narrow(&mut fs, &[("/A long café home", Rights::READ); 9])
            .is_err());
        assert!(grants
            .narrow(&mut fs, &[("relative", Rights::READ)])
            .is_err());
        let combined = Authority::System
            .narrow(
                &mut fs,
                &[
                    ("/A long café home", Rights::READ),
                    ("/A long café home/child", Rights::WRITE),
                ],
            )
            .unwrap();
        assert_eq!(
            combined.file(&mut fs, "/A long café home/child/new", Rights::READ_WRITE),
            Ok(())
        );
        assert!(combined
            .file(&mut fs, "/A long café home/new", Rights::WRITE)
            .is_err());
        fs.device.heap_ceiling = None;
    }

    #[test_case]
    fn filesystem_grant_roots_reject_malformed_directory_links() {
        use crate::fs::access::{Authority, Rights};
        let mut fs =
            Fat32::format(MemDisk::new(TEST_SECTORS), TEST_SECTORS, b"TESTVOLUME ").unwrap();
        fs.create_dir("/BROKEN").unwrap();
        let (entry_cluster, index) = fs
            .find_entry_in(fs.root_cluster, "BROKEN")
            .unwrap()
            .unwrap();
        let mut entry = [0; 32];
        fs.read_dir_entry(entry_cluster, index, &mut entry).unwrap();
        let original = entry;
        super::set_entry_cluster(&mut entry, 0);
        fs.write_dir_entry(entry_cluster, index, &entry).unwrap();
        assert!(
            Authority::System
                .narrow(&mut fs, &[("/BROKEN", Rights::READ)])
                .is_err(),
            "a malformed directory entry must not mint a sealed grant"
        );
        fs.write_dir_entry(entry_cluster, index, &original).unwrap();
        let directory = fs.dir_entry_cluster(entry_cluster, index).unwrap();
        let mut parent = [0; 32];
        fs.read_dir_entry(directory, 1, &mut parent).unwrap();
        super::set_entry_cluster(&mut parent, directory);
        fs.write_dir_entry(directory, 1, &parent).unwrap();
        assert!(
            Authority::System
                .narrow(&mut fs, &[("/BROKEN", Rights::READ)])
                .is_err(),
            "cyclic parents must fail without unbounded traversal"
        );
    }

    #[test_case]
    fn filesystem_grants_follow_directory_identity_and_only_attenuate() {
        use crate::fs::access::{Authority, Rights};
        let mut fs =
            Fat32::format(MemDisk::new(TEST_SECTORS), TEST_SECTORS, b"TESTVOLUME ").unwrap();
        fs.create_dir("/A long home/sub").unwrap();
        fs.create_dir("/PRIVATE").unwrap();
        fs.create_dir("/OUTSIDE").unwrap();
        fs.create_file("/A long home/READ.TXT").unwrap();
        let access = Authority::System
            .narrow(
                &mut fs,
                &[("/A long home", Rights::READ), ("/PRIVATE", Rights::WRITE)],
            )
            .unwrap();
        assert_eq!(
            access.file(&mut fs, "/A long home/READ.TXT", Rights::READ),
            Ok(())
        );
        assert_eq!(
            access.file(&mut fs, "/A long home/READ.TXT", Rights::WRITE),
            Err(FatError::AccessDenied)
        );
        assert_eq!(access.file(&mut fs, "/PRIVATE/new", Rights::WRITE), Ok(()));
        assert_eq!(
            access.file(&mut fs, "/PRIVATE/new", Rights::READ),
            Err(FatError::AccessDenied)
        );
        assert_eq!(
            access.file(&mut fs, "/OUTSIDE/new", Rights::WRITE),
            Err(FatError::AccessDenied)
        );
        assert_eq!(access.search(&mut fs, "/"), Ok(()));
        assert_eq!(access.search(&mut fs, "/A long home"), Ok(()));
        assert_eq!(
            access.search(&mut fs, "/OUTSIDE"),
            Err(FatError::AccessDenied)
        );
        assert!(access
            .narrow(&mut fs, &[("/A long home", Rights::READ_WRITE)])
            .is_err());
        assert!(access.narrow(&mut fs, &[("/", Rights::READ)]).is_err());
        let child = access
            .narrow(&mut fs, &[("/A long home/sub", Rights::READ)])
            .unwrap();
        assert!(child
            .file(&mut fs, "/A long home/READ.TXT", Rights::READ)
            .is_err());
        assert!(child
            .file(&mut fs, "/A long home/sub/new", Rights::READ)
            .is_ok());
        let empty = child.narrow(&mut fs, &[]).unwrap();
        assert!(empty.search(&mut fs, "/").is_err());
        assert!(empty
            .narrow(&mut fs, &[("/A long home/sub", Rights::READ)])
            .is_err());
    }

    #[test_case]
    fn directory_creation_metadata_failures_reclaim_the_new_cluster() {
        let mut fs =
            Fat32::format(MemDisk::new(TEST_SECTORS), TEST_SECTORS, b"TESTVOLUME ").unwrap();
        let limit = (fs.total_sectors - fs.first_data_sector) / u32::from(fs.sectors_per_cluster);
        let free = (2..limit + 2)
            .filter(|&cluster| fs.read_fat_entry(cluster) == Ok(0))
            .count();
        fs.device.heap_ceiling = Some(crate::allocator::allocated_bytes());
        for _ in 0..3 {
            for failing_write in [2, 3] {
                let hint = fs.next_free_cluster.clamp(2, limit + 1);
                let cluster = (hint..limit + 2)
                    .chain(2..hint)
                    .find(|&cluster| fs.read_fat_entry(cluster) == Ok(super::FAT_FREE))
                    .unwrap();
                fs.device.writes.fill(0);
                fs.device.fail_write_nth = Some((fs.cluster_to_lba(cluster), failing_write));
                assert_eq!(
                    fs.create_dir("/Fresh long directory"),
                    Err(FatError::DeviceError)
                );
                fs.device.fail_write_nth = None;
                assert_eq!(
                    fs.check_directory("/Fresh long directory"),
                    Err(FatError::FileNotFound)
                );
                assert_eq!(
                    (2..limit + 2)
                        .filter(|&cluster| fs.read_fat_entry(cluster) == Ok(0))
                        .count(),
                    free,
                    "failed dot/dotdot initialization must reclaim the unpublished directory"
                );
            }
        }
        fs.create_dir("/Fresh long directory").unwrap();
        assert_eq!(fs.check_directory("/Fresh long directory"), Ok(()));
        fs.create_dir("/Fresh long directory").unwrap();
        assert_eq!(
            (2..limit + 2)
                .filter(|&cluster| fs.read_fat_entry(cluster) == Ok(0))
                .count(),
            free - 1
        );
        fs.device.heap_ceiling = None;
    }

    #[test_case]
    fn name_publication_failure_across_directory_clusters_rolls_back_growth() {
        let mut fs =
            Fat32::format(MemDisk::new(TEST_SECTORS), TEST_SECTORS, b"TESTVOLUME ").unwrap();
        let paths: Vec<String> = (0..4)
            .map(|index| alloc::format!("{}{}", "a".repeat(254), index))
            .collect();
        for path in &paths[..3] {
            fs.create_file(path).unwrap();
        }
        let limit = (fs.total_sectors - fs.first_data_sector) / u32::from(fs.sectors_per_cluster);
        let free = (2..limit + 2)
            .filter(|&cluster| fs.read_fat_entry(cluster) == Ok(0))
            .count();
        fs.device.heap_ceiling = Some(crate::allocator::allocated_bytes());
        let hint = fs.next_free_cluster.clamp(2, limit + 1);
        let cluster = (hint..limit + 2)
            .chain(2..hint)
            .find(|&cluster| fs.read_fat_entry(cluster) == Ok(super::FAT_FREE))
            .unwrap();
        fs.device.writes.fill(0);
        fs.device.fail_write_nth = Some((fs.cluster_to_lba(cluster), 2));
        assert_eq!(fs.create_file(&paths[3]), Err(FatError::DeviceError));
        fs.device.fail_write_nth = None;
        assert_eq!(
            (2..limit + 2)
                .filter(|&cluster| fs.read_fat_entry(cluster) == Ok(0))
                .count(),
            free
        );
        assert_eq!(fs.file_size(&paths[3]), Err(FatError::FileNotFound));
        for path in &paths[..3] {
            assert_eq!(fs.file_size(path), Ok(0));
        }
        fs.create_file(&paths[3]).unwrap();
        assert_eq!(fs.file_size(&paths[3]), Ok(0));
        let mut count = 0;
        fs.visit_dir("/", |_, _, _, _| {
            count += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(count, 4);
        fs.device.heap_ceiling = None;
    }

    #[test_case]
    fn name_creation_alias_collisions_growth_and_deletion_use_bounded_scratch() {
        let mut fs =
            Fat32::format(MemDisk::new(TEST_SECTORS), TEST_SECTORS, b"TESTVOLUME ").unwrap();
        let paths: Vec<String> = (0..12)
            .map(|index| {
                alloc::format!("/Café directory/collision {} {index}.txt", "x".repeat(200))
            })
            .collect();
        let maximum = alloc::format!("/Café directory/{}", "文".repeat(255));
        fs.device.reads.fill(0);
        fs.device.writes.fill(0);
        assert_eq!(
            fs.reserve_name_slots(fs.root_cluster, 0),
            Err(FatError::InvalidName)
        );
        assert_eq!(
            fs.reserve_name_slots(fs.root_cluster, 22),
            Err(FatError::InvalidName)
        );
        assert_eq!(fs.device.reads.iter().sum::<usize>(), 0);
        assert_eq!(fs.device.writes.iter().sum::<usize>(), 0);
        fs.device.heap_ceiling = Some(crate::allocator::allocated_bytes());
        fs.create_dir("/Café directory/文書").unwrap();
        for (index, path) in paths.iter().enumerate() {
            fs.create_file(path).unwrap();
            let (directory, name) = fs.resolve_parent(path).unwrap();
            let (cluster, position) = fs.find_entry_in(directory, name).unwrap().unwrap();
            let mut entry = [0; 32];
            fs.read_dir_entry(cluster, position, &mut entry).unwrap();
            if index == 0 {
                assert_eq!(&entry[..11], b"COLLIS~1TXT");
            }
            if index == 9 {
                assert_eq!(&entry[..11], b"COLLI~10TXT");
            }
            assert_eq!(fs.file_size(path), Ok(0));
        }
        fs.create_file(&maximum).unwrap();
        assert_eq!(fs.file_size(&maximum), Ok(0));
        for _ in 0..3 {
            fs.delete_file(&maximum).unwrap();
            fs.create_file(&maximum).unwrap();
        }
        for path in &paths {
            fs.delete_file(path).unwrap();
        }
        let mut count = 0;
        fs.visit_dir("/Café directory", |_, _, _, _| {
            count += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(
            count, 2,
            "only the subdirectory and maximum-length file remain"
        );
        fs.device.heap_ceiling = None;
    }

    #[test_case]
    fn range_write_io_failures_release_unpublished_extension_chains() {
        let mut fs =
            Fat32::format(MemDisk::new(TEST_SECTORS), TEST_SECTORS, b"TESTVOLUME ").unwrap();
        fs.create_file("/KEEP.BIN").unwrap();
        let contents = alloc::vec![0x5a; 1024];
        let mut output = alloc::vec![0; contents.len()];
        fs.write_file("/KEEP.BIN", &contents).unwrap();
        let (directory, name) = fs.resolve_parent("/KEEP.BIN").unwrap();
        let (entry, index) = fs.find_entry_in(directory, name).unwrap().unwrap();
        let first = fs.dir_entry_cluster(entry, index).unwrap();
        let metadata_lba = fs.dir_entry_lba(entry, index);
        let limit = (fs.total_sectors - fs.first_data_sector) / u32::from(fs.sectors_per_cluster);
        let free = (2..limit + 2)
            .filter(|&cluster| fs.read_fat_entry(cluster) == Ok(0))
            .count();
        fs.device.heap_ceiling = Some(crate::allocator::allocated_bytes());
        for _ in 0..3 {
            // Failed publication occurs after staging/writing the new chain.
            fs.device.fail_write_lba = Some(metadata_lba);
            assert_eq!(
                fs.write_at("/KEEP.BIN", 6000, b"tail"),
                Err(FatError::DeviceError)
            );
            fs.device.fail_write_lba = None;
            assert_eq!(fs.file_size("/KEEP.BIN"), Ok(contents.len()));
            assert_eq!(fs.read_file("/KEEP.BIN", &mut output), Ok(contents.len()));
            assert_eq!(output, contents);
            assert_eq!(fs.dir_entry_cluster(entry, index), Ok(first));
            assert!(fs.read_fat_entry(first).unwrap() >= super::FAT_EOC_MIN);
            assert_eq!(
                (2..limit + 2)
                    .filter(|&cluster| fs.read_fat_entry(cluster) == Ok(0))
                    .count(),
                free
            );
            // Read failure while zeroing a gap must also discard staged growth.
            fs.device.fail_read_lba = Some(fs.cluster_to_lba(first) + 2);
            assert_eq!(
                fs.write_at("/KEEP.BIN", 6000, b"tail"),
                Err(FatError::DeviceError)
            );
            fs.device.fail_read_lba = None;
            assert_eq!(fs.file_size("/KEEP.BIN"), Ok(contents.len()));
            assert_eq!(
                (2..limit + 2)
                    .filter(|&cluster| fs.read_fat_entry(cluster) == Ok(0))
                    .count(),
                free
            );
        }
        fs.device.heap_ceiling = None;
    }

    #[test_case]
    fn cluster_subranges_preserve_guards_bounds_and_read_only_needed_sectors() {
        let mut fs =
            Fat32::format(MemDisk::new(TEST_SECTORS), TEST_SECTORS, b"TESTVOLUME ").unwrap();
        let cluster = fs.allocate_cluster().unwrap();
        let bytes = usize::from(fs.sectors_per_cluster) * SECTOR_SIZE;
        let mut contents = alloc::vec![0; bytes];
        for (index, byte) in contents.iter_mut().enumerate() {
            *byte = index as u8;
        }
        fs.write_cluster(cluster, &contents).unwrap();
        let mut output = alloc::vec![0xa5; bytes + 2];
        for offset in [0, 1, 511, 512, 513, bytes - 1, bytes] {
            for length in [0, 1, 511, 512, bytes] {
                output.fill(0xa5);
                fs.device.reads.fill(0);
                let result = fs.read_cluster_at(cluster, offset, &mut output[1..1 + length]);
                if offset + length <= bytes {
                    assert_eq!(result, Ok(()));
                    assert_eq!(&output[1..1 + length], &contents[offset..offset + length]);
                    let needed = if length == 0 {
                        0
                    } else {
                        (offset % SECTOR_SIZE + length).div_ceil(SECTOR_SIZE)
                    };
                    assert_eq!(fs.device.reads.iter().sum::<usize>(), needed);
                } else {
                    assert_eq!(result, Err(FatError::DeviceError));
                    assert_eq!(fs.device.reads.iter().sum::<usize>(), 0);
                    assert!(output.iter().all(|&byte| byte == 0xa5));
                }
                assert_eq!(output[0], 0xa5);
                assert!(output[1 + length..].iter().all(|&byte| byte == 0xa5));
            }
        }
        for offset in [usize::MAX, usize::MAX - 1] {
            assert_eq!(
                fs.read_cluster_at(cluster, offset, &mut output[..2]),
                Err(FatError::DeviceError)
            );
        }
        fs.device.writes.fill(0);
        assert_eq!(fs.zero_cluster(0), Err(FatError::DeviceError));
        assert_eq!(fs.zero_cluster(u32::MAX), Err(FatError::DeviceError));
        assert_eq!(fs.device.writes.iter().sum::<usize>(), 0);
    }

    #[test_case]
    fn file_metadata_reads_and_replacement_use_bounded_scratch() {
        let mut fs =
            Fat32::format(MemDisk::new(TEST_SECTORS), TEST_SECTORS, b"TESTVOLUME ").unwrap();
        let path = "/A long café folder/文書 contents.bin";
        fs.create_dir("/A long café folder").unwrap();
        fs.create_file(path).unwrap();
        let mut contents = alloc::vec![0; 3 * 2048 + 77];
        for (index, byte) in contents.iter_mut().enumerate() {
            *byte = index as u8;
        }
        fs.write_file(path, &contents).unwrap();
        let mut output = alloc::vec![0xa5; 16 * 1024 + 2];
        fs.device.heap_ceiling = Some(crate::allocator::allocated_bytes());
        assert_eq!(fs.create_dir("/A long café folder"), Ok(()));
        assert_eq!(fs.file_size(path), Ok(contents.len()));
        for offset in [
            0,
            1,
            511,
            512,
            513,
            2047,
            2048,
            2049,
            contents.len() - 1,
            contents.len(),
            usize::MAX,
        ] {
            for length in [0, 1, 511, 512, 513, 2048, contents.len()] {
                output.fill(0xa5);
                let expected = contents.len().saturating_sub(offset).min(length);
                assert_eq!(
                    fs.read_at(path, offset, &mut output[1..1 + length]),
                    Ok(expected)
                );
                assert_eq!(
                    &output[1..1 + expected],
                    &contents[offset.min(contents.len())..offset.min(contents.len()) + expected]
                );
                assert!(output[..1]
                    .iter()
                    .chain(output[1 + expected..].iter())
                    .all(|&byte| byte == 0xa5));
            }
        }
        output.fill(0xa5);
        assert_eq!(
            fs.read_file(path, &mut output[1..1 + contents.len()]),
            Ok(contents.len())
        );
        assert_eq!(&output[1..1 + contents.len()], contents.as_slice());
        assert_eq!(output[0], 0xa5);
        assert_eq!(output[contents.len() + 1], 0xa5);
        assert_eq!(
            fs.read_file(path, &mut output[..1]),
            Err(FatError::BufferTooSmall)
        );
        assert_eq!(
            fs.file_size("/A long café folder/MISSING"),
            Err(FatError::FileNotFound)
        );
        assert_eq!(
            fs.file_size("/A long café folder"),
            Err(FatError::IsADirectory)
        );
        assert_eq!(
            fs.file_size("/MISSING/bad?name"),
            Err(FatError::InvalidName)
        );
        assert_eq!(fs.file_size("///"), Err(FatError::InvalidName));
        assert_eq!(fs.write_at(path, 511, b"PATCH"), Ok(5));
        contents[511..516].copy_from_slice(b"PATCH");
        let extension = contents.len() + 4097;
        assert_eq!(fs.write_at(path, extension, b"tail"), Ok(4));
        output.fill(0xa5);
        assert_eq!(
            fs.read_at(path, 0, &mut output[1..extension + 5]),
            Ok(extension + 4)
        );
        assert_eq!(&output[1..1 + contents.len()], contents.as_slice());
        assert!(output[1 + contents.len()..1 + extension]
            .iter()
            .all(|&byte| byte == 0));
        assert_eq!(&output[1 + extension..1 + extension + 4], b"tail");
        assert_eq!(output[0], 0xa5);
        assert_eq!(output[extension + 5], 0xa5);
        assert_eq!(
            fs.write_at(path, TEST_SECTORS as usize * SECTOR_SIZE, b"x"),
            Err(FatError::DiskFull)
        );
        assert_eq!(fs.file_size(path), Ok(extension + 4));
        assert_eq!(fs.write_file(path, &contents[..77]), Ok(77));
        assert_eq!(fs.file_size(path), Ok(77));
        assert_eq!(fs.read_file(path, &mut output), Ok(77));
        assert_eq!(&output[..77], &contents[..77]);
        assert_eq!(fs.write_file(path, &[]), Ok(0));
        assert_eq!(fs.file_size(path), Ok(0));
        fs.device.heap_ceiling = None;
    }

    #[test_case]
    fn directory_scanning_uses_bounded_scratch_without_heap_growth() {
        let mut fs =
            Fat32::format(MemDisk::new(TEST_SECTORS), TEST_SECTORS, b"TESTVOLUME ").unwrap();
        fs.create_file("/A long café 文書 filename.txt").unwrap();
        let allocated = crate::allocator::allocated_bytes();
        let reads: usize = fs.device.reads.iter().sum();
        let mut visited = false;
        fs.scan_directory(fs.root_cluster, |_, _, _, long, _| {
            assert_eq!(
                crate::allocator::allocated_bytes(),
                allocated,
                "directory scan scratch and decoded names must not allocate"
            );
            assert_eq!(long, Some("A long café 文書 filename.txt"));
            visited = true;
            None::<()>
        })
        .unwrap();
        assert!(visited);
        assert_eq!(
            fs.device.reads.iter().sum::<usize>() - reads,
            1,
            "a scan reads each directory sector once, not once per slot"
        );
        fs.create_dir("/A long café folder").unwrap();
        fs.create_file("/A long café folder/PLAIN.TXT").unwrap();
        let allocated = crate::allocator::allocated_bytes();
        let mut visited = false;
        fs.visit_dir("/A long café folder", |alias, name, attr, size| {
            assert_eq!(crate::allocator::allocated_bytes(), allocated);
            assert_eq!(alias, b"PLAIN   TXT");
            assert_eq!(name, "PLAIN.TXT");
            assert_eq!(attr, 0x20);
            assert_eq!(size, 0);
            visited = true;
            Ok(())
        })
        .unwrap();
        assert!(visited);
        assert_eq!(
            fs.visit_dir("/A long café folder", |_, _, _, _| Err(
                FatError::AccessDenied
            )),
            Err(FatError::AccessDenied)
        );
    }

    #[test_case]
    fn root_entry_protection_resolves_aliases_and_reserves_missing_names() {
        let mut fs =
            Fat32::format(MemDisk::new(TEST_SECTORS), TEST_SECTORS, b"TESTVOLUME ").unwrap();
        assert_eq!(fs.in_root_entry("/EtC/new/child", "ETC"), Ok(true));
        assert_eq!(fs.in_root_entry("/ETCETERA/file", "ETC"), Ok(false));
        fs.create_dir("/Protected Long Directory").unwrap();
        fs.create_file("/Protected Long Filename.txt").unwrap();
        let entries = fs.list_dir(None).unwrap();
        for entry in entries {
            let root = entry.long_name.as_deref().unwrap();
            let alias = super::short_name(&entry.name);
            assert_ne!(root, alias);
            let long = alloc::format!("/{root}");
            let short = alloc::format!("/{alias}");
            assert_eq!(fs.in_root_entry(&long, root), Ok(true));
            assert_eq!(fs.in_root_entry(&short, root), Ok(true));
            // Both sides resolve aliases, including an alias used as the label.
            assert_eq!(fs.in_root_entry(&long, &alias), Ok(true));
            if entry.is_dir() {
                assert_eq!(
                    fs.in_root_entry(&alloc::format!("{short}/missing"), root),
                    Ok(true)
                );
            }
            assert_eq!(fs.in_root_entry("/elsewhere/file", root), Ok(false));
        }
        for path in ["relative", "/ETC/../elsewhere", "/./ETC", "/ETC/bad?name"] {
            assert_eq!(fs.in_root_entry(path, "ETC"), Err(FatError::InvalidName));
        }
        assert_eq!(fs.in_root_entry("/", "ETC"), Ok(false));
        for root in ["", ".", "..", "/ETC", "ETC/sub"] {
            assert_eq!(fs.in_root_entry("/file", root), Err(FatError::InvalidName));
        }
        assert!(fs.device.writes.iter().any(|n| *n > 0));
        fs.device.writes.fill(0);
        assert_eq!(fs.in_root_entry("/ETC/new", "ETC"), Ok(true));
        assert!(
            fs.device.writes.iter().all(|n| *n == 0),
            "authorization must not write"
        );
    }

    #[test_case]
    fn range_write_does_not_rewrite_unaffected_file_clusters() {
        let mut fs =
            Fat32::format(MemDisk::new(TEST_SECTORS), TEST_SECTORS, b"TESTVOLUME ").unwrap();
        fs.create_file("RANGE.BIN").unwrap();
        fs.write_file("RANGE.BIN", &[b'a'; 16384]).unwrap();
        let (directory, name) = fs.resolve_parent("RANGE.BIN").unwrap();
        let (entry, index) = fs.find_entry_in(directory, &name).unwrap().unwrap();
        let first = fs.dir_entry_cluster(entry, index).unwrap();
        let second = fs.read_fat_entry(first).unwrap();
        let second_lba = fs.cluster_to_lba(second) as usize;
        fs.device.writes.fill(0);
        assert_eq!(fs.write_at("RANGE.BIN", 100, b"patch"), Ok(5));
        assert!(
            fs.device.writes[second_lba..].iter().all(|&n| n == 0),
            "small overwrite wrote unrelated file data"
        );
        let limit = (fs.total_sectors - fs.first_data_sector) / u32::from(fs.sectors_per_cluster);
        let free = (2..limit + 2)
            .filter(|&c| fs.read_fat_entry(c) == Ok(0))
            .count();
        let offset = (limit as usize - 1) * usize::from(fs.sectors_per_cluster) * SECTOR_SIZE;
        assert_eq!(
            fs.write_at("RANGE.BIN", offset, b"x"),
            Err(FatError::DiskFull)
        );
        let after = (2..limit + 2)
            .filter(|&c| fs.read_fat_entry(c) == Ok(0))
            .count();
        assert_eq!(after, free, "failed extension leaked its staged chain");
        assert_eq!(fs.file_size("RANGE.BIN"), Ok(16384));
    }

    #[test_case]
    fn long_names_aliases_cluster_boundaries_and_deletion() {
        let mut fs =
            Fat32::format(MemDisk::new(TEST_SECTORS), TEST_SECTORS, b"TESTVOLUME ").unwrap();
        fs.create_dir("/A long directory name").unwrap();
        for index in 0..40 {
            let path = alloc::format!("/A long directory name/a repeated long name {}.txt", index);
            fs.create_file(&path).unwrap();
            fs.write_at(&path, 0, &[index]).unwrap();
        }
        let name = "/A long directory name/café 文書.txt";
        fs.create_file(name).unwrap();
        fs.write_file(name, b"unicode").unwrap();
        let mut data = [0; 7];
        assert_eq!(
            fs.read_file("/a LONG directory NAME/CAFÉ 文書.TXT", &mut data),
            Ok(7)
        );
        assert_eq!(&data, b"unicode");
        let entries = fs.list_dir(Some("/A long directory name")).unwrap();
        assert_eq!(entries.len(), 41);
        assert!(entries
            .iter()
            .any(|entry| entry.long_name.as_deref() == Some("café 文書.txt")));
        for i in 0..entries.len() {
            for j in i + 1..entries.len() {
                assert_ne!(entries[i].name, entries[j].name);
            }
        }
        fs.delete_file(name).unwrap();
        assert_eq!(fs.read_file(name, &mut data), Err(FatError::FileNotFound));
        fs.create_file(name).unwrap();
        fs.write_at(name, 0, b"reuse").unwrap();
        assert_eq!(fs.read_at(name, 0, &mut data), Ok(5));
        assert_eq!(&data[..5], b"reuse");
        assert_eq!(fs.create_file("/bad*name"), Err(FatError::InvalidName));
    }

    #[test_case]
    fn offset_writes_preserve_unaffected_data_and_zero_gaps() {
        let mut fs =
            Fat32::format(MemDisk::new(TEST_SECTORS), TEST_SECTORS, b"TESTVOLUME ").unwrap();
        fs.create_file("EDIT.BIN").unwrap();
        fs.write_file("EDIT.BIN", &[b'a'; 2000]).unwrap();
        let (directory, name) = fs.resolve_parent("EDIT.BIN").unwrap();
        let (entry, index) = fs.find_entry_in(directory, &name).unwrap().unwrap();
        let first = fs.dir_entry_cluster(entry, index).unwrap();
        assert_eq!(fs.write_at("EDIT.BIN", 510, b"PATCH"), Ok(5));
        assert_eq!(
            fs.dir_entry_cluster(entry, index).unwrap(),
            first,
            "overwrite must retain existing data clusters"
        );
        let mut out = [0; 2000];
        fs.read_file("EDIT.BIN", &mut out).unwrap();
        assert!(out[..510].iter().all(|&b| b == b'a'));
        assert_eq!(&out[510..515], b"PATCH");
        assert!(out[515..].iter().all(|&b| b == b'a'));
        assert_eq!(fs.write_at("EDIT.BIN", 3000, b"tail"), Ok(4));
        let mut gap = [1; 1004];
        assert_eq!(fs.read_at("EDIT.BIN", 2000, &mut gap), Ok(1004));
        assert!(gap[..1000].iter().all(|&b| b == 0));
        assert_eq!(&gap[1000..], b"tail");
        assert_eq!(
            fs.write_at("EDIT.BIN", usize::MAX, b"x"),
            Err(FatError::DiskFull)
        );
        assert_eq!(fs.write_at("EDIT.BIN", 0, &[]), Ok(0));
        assert_eq!(fs.write_at("MISSING", 0, b"x"), Err(FatError::FileNotFound));
        assert_eq!(
            fs.write_at("EDIT.BIN", TEST_SECTORS as usize * SECTOR_SIZE, b"x"),
            Err(FatError::DiskFull)
        );
        assert_eq!(fs.read_at("EDIT.BIN", 3000, &mut gap[..4]), Ok(4));
        assert_eq!(&gap[..4], b"tail");
    }

    #[test_case]
    fn offset_reads_cross_clusters_and_stop_at_eof() {
        let mut fs =
            Fat32::format(MemDisk::new(TEST_SECTORS), TEST_SECTORS, b"TESTVOLUME ").unwrap();
        fs.create_file("READ.BIN").unwrap();
        let data: Vec<u8> = (0..1800).map(|i| (i % 251) as u8).collect();
        fs.write_file("READ.BIN", &data).unwrap();
        let mut out = [0; 700];
        assert_eq!(fs.read_at("READ.BIN", 500, &mut out), Ok(700));
        assert_eq!(&out, &data[500..1200]);
        assert_eq!(fs.read_at("READ.BIN", 1790, &mut out), Ok(10));
        assert_eq!(&out[..10], &data[1790..]);
        assert_eq!(fs.read_at("READ.BIN", usize::MAX, &mut out), Ok(0));
        assert_eq!(fs.read_at("READ.BIN", 0, &mut []), Ok(0));
        assert_eq!(
            fs.read_at("MISSING", 0, &mut out),
            Err(FatError::FileNotFound)
        );
        assert_eq!(fs.check_directory("/"), Ok(()));
        assert_eq!(fs.check_directory("READ.BIN"), Err(FatError::NotADirectory));
    }

    #[test_case]
    fn failed_replacement_preserves_original_and_reclaims_clusters() {
        let mut fs =
            Fat32::format(MemDisk::new(TEST_SECTORS), TEST_SECTORS, b"TESTVOLUME ").unwrap();
        fs.create_file("KEEP.TXT").unwrap();
        fs.write_file("KEEP.TXT", b"keep me").unwrap();
        let cluster_count =
            (fs.total_sectors - fs.first_data_sector) / u32::from(fs.sectors_per_cluster);
        let free_before = (2..cluster_count + 2)
            .filter(|&c| fs.read_fat_entry(c) == Ok(0))
            .count();
        let too_large = alloc::vec![42; TEST_SECTORS as usize * SECTOR_SIZE];
        assert_eq!(
            fs.write_file("KEEP.TXT", &too_large),
            Err(FatError::DiskFull)
        );
        let mut original = [0; 7];
        assert_eq!(fs.read_file("KEEP.TXT", &mut original), Ok(7));
        assert_eq!(&original, b"keep me");
        let free_after = (2..cluster_count + 2)
            .filter(|&c| fs.read_fat_entry(c) == Ok(0))
            .count();
        assert_eq!(
            free_after, free_before,
            "failed replacement must release its partial chain"
        );
    }

    #[test_case]
    fn sequential_file_read_reuses_fat_metadata_sectors() {
        let mut fs =
            Fat32::format(MemDisk::new(TEST_SECTORS), TEST_SECTORS, b"TESTVOLUME ").unwrap();
        fs.create_file("LARGE.BIN").unwrap();
        let data: Vec<u8> = (0..100 * 1024).map(|i| (i % 251) as u8).collect();
        fs.write_file("LARGE.BIN", &data).unwrap();
        fs.device.reads.fill(0);
        let mut out = alloc::vec![0; data.len()];
        assert_eq!(fs.read_file("LARGE.BIN", &mut out), Ok(data.len()));
        assert_eq!(out, data);
        let first = fs.reserved_sectors as usize;
        let metadata_reads: usize = fs.device.reads[first..first + fs.fat_size_sectors as usize]
            .iter()
            .sum();
        assert!(
            metadata_reads <= 2,
            "sequential chain reread FAT metadata {} times",
            metadata_reads
        );
    }

    #[test_case]
    fn fat_cache_invalidates_on_success_and_failed_metadata_writes() {
        let mut fs =
            Fat32::format(MemDisk::new(TEST_SECTORS), TEST_SECTORS, b"TESTVOLUME ").unwrap();
        assert_eq!(fs.read_fat_entry(4), Ok(0));
        fs.write_fat_entry(4, 7).unwrap();
        assert_eq!(fs.read_fat_entry(4), Ok(7));
        let sector = fs.reserved_sectors as u32;
        fs.device.fail_write_lba = Some(sector);
        assert_eq!(fs.write_fat_entry(4, 9), Err(FatError::DeviceError));
        fs.device.reads.fill(0);
        assert_eq!(fs.read_fat_entry(4), Ok(7));
        assert_eq!(
            fs.device.reads[sector as usize], 1,
            "a failed write must discard the cached version too"
        );
        fs.device.fail_write_lba = None;
        fs.write_fat_entry(4, 11).unwrap();
        assert_eq!(fs.read_fat_entry(4), Ok(11));
    }

    #[test_case]
    fn default_bulk_reads_stop_on_overflow_and_preserve_unread_buffers() {
        struct Probe {
            calls: usize,
            fail_at: Option<u32>,
        }
        impl BlockDevice for Probe {
            type Error = u32;
            fn read_sector(&mut self, lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), u32> {
                self.calls += 1;
                if self.fail_at == Some(lba) {
                    return Err(lba);
                }
                buf.fill(lba as u8);
                Ok(())
            }
            fn write_sector(&mut self, lba: u32, _: &[u8; SECTOR_SIZE]) -> Result<(), u32> {
                Err(lba)
            }
        }
        let mut disk = Probe {
            calls: 0,
            fail_at: None,
        };
        assert_eq!(disk.read_sectors(u32::MAX, &mut []), Ok(0));
        assert_eq!(disk.calls, 0);
        let mut out = [[0xa5; SECTOR_SIZE]; 3];
        assert_eq!(disk.read_sectors(u32::MAX - 1, &mut out), Ok(2));
        assert_eq!(disk.calls, 2);
        assert_eq!(out[0], [0xfe; SECTOR_SIZE]);
        assert_eq!(out[1], [0xff; SECTOR_SIZE]);
        assert_eq!(out[2], [0xa5; SECTOR_SIZE]);
        disk.fail_at = Some(4);
        out.fill([0xa5; SECTOR_SIZE]);
        assert_eq!(disk.read_sectors(3, &mut out), Err(4));
        assert_eq!(out[0], [3; SECTOR_SIZE]);
        assert_eq!(out[1], [0xa5; SECTOR_SIZE]);
        assert_eq!(out[2], [0xa5; SECTOR_SIZE]);
    }

    #[test_case]
    fn cluster_bulk_read_checks_geometry_short_io_and_partial_sector_bounds() {
        use super::ROOT_CLUSTER;
        let mut fs =
            Fat32::format(MemDisk::new(TEST_SECTORS), TEST_SECTORS, b"TESTVOLUME ").unwrap();
        let mut guarded = [0xa5; SECTOR_SIZE + 3];
        fs.read_cluster(ROOT_CLUSTER, &mut guarded[1..SECTOR_SIZE + 2])
            .unwrap();
        assert_eq!(guarded[0], 0xa5);
        assert_eq!(guarded[SECTOR_SIZE + 2], 0xa5);
        assert_eq!(&guarded[1..SECTOR_SIZE + 2], &[0; SECTOR_SIZE + 1]);
        assert_eq!(
            fs.read_cluster(0, &mut guarded[..1]),
            Err(FatError::DeviceError)
        );
        assert_eq!(
            fs.read_cluster(u32::MAX, &mut guarded[..1]),
            Err(FatError::DeviceError)
        );
        let mut oversized = alloc::vec![0; usize::from(fs.sectors_per_cluster)*SECTOR_SIZE+1];
        assert_eq!(
            fs.read_cluster(ROOT_CLUSTER, &mut oversized),
            Err(FatError::DeviceError)
        );
        struct ShortDisk(MemDisk);
        impl BlockDevice for ShortDisk {
            type Error = ();
            fn read_sector(&mut self, lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), ()> {
                self.0.read_sector(lba, buf)
            }
            fn read_sectors(&mut self, _: u32, _: &mut [[u8; SECTOR_SIZE]]) -> Result<usize, ()> {
                Ok(0)
            }
            fn write_sector(&mut self, lba: u32, buf: &[u8; SECTOR_SIZE]) -> Result<(), ()> {
                self.0.write_sector(lba, buf)
            }
        }
        let mut short = Fat32::open(ShortDisk(fs.into_device())).unwrap();
        assert_eq!(
            short.read_cluster(ROOT_CLUSTER, &mut guarded[1..SECTOR_SIZE + 1]),
            Err(FatError::DeviceError)
        );
    }

    #[test_case]
    fn file_size_handles_nested_empty_and_error_cases() {
        let mut fs =
            Fat32::format(MemDisk::new(TEST_SECTORS), TEST_SECTORS, b"TESTVOLUME ").unwrap();
        fs.create_dir("/BIN").unwrap();
        fs.create_file("/BIN/TINY.BIN").unwrap();
        assert_eq!(fs.file_size("/BIN/TINY.BIN"), Ok(0));
        fs.write_file("/BIN/TINY.BIN", b"flat binary").unwrap();
        assert_eq!(fs.file_size("/BIN/TINY.BIN"), Ok(11));
        assert_eq!(
            fs.file_size("/BIN/MISSING.BIN"),
            Err(FatError::FileNotFound)
        );
        assert_eq!(fs.file_size("/BIN"), Err(FatError::IsADirectory));
    }

    #[test_case]
    fn test_to_83_uppercases_and_pads() {
        assert_eq!(to_83("hi.txt").unwrap(), *b"HI      TXT");
        assert_eq!(to_83("README").unwrap(), *b"README     ");
        assert_eq!(to_83("a.b").unwrap(), *b"A       B  ");
    }

    #[test_case]
    fn test_to_83_rejects_bad_names() {
        assert_eq!(to_83("").unwrap_err(), FatError::InvalidName);
        assert_eq!(to_83("toolongname.txt").unwrap_err(), FatError::InvalidName);
        assert_eq!(to_83("a.b.c").unwrap_err(), FatError::InvalidName);
        assert_eq!(to_83("bad*name").unwrap_err(), FatError::InvalidName);
        assert_eq!(to_83("ext.toolong").unwrap_err(), FatError::InvalidName);
    }

    #[test_case]
    fn test_format_then_open_round_trips_bpb() {
        let disk = MemDisk::new(TEST_SECTORS);
        let fs = Fat32::format(disk, TEST_SECTORS, b"TESTVOLUME ").unwrap();
        assert_eq!(fs.total_bytes(), u64::from(TEST_SECTORS) * 512);

        let disk = fs.into_device();
        let fs2 = Fat32::open(disk).unwrap();
        assert_eq!(fs2.total_bytes(), u64::from(TEST_SECTORS) * 512);
    }

    #[test_case]
    fn test_open_falls_back_to_totsec16() {
        let disk = MemDisk::new(TEST_SECTORS);
        let fs = Fat32::format(disk, TEST_SECTORS, b"TESTVOLUME ").unwrap();
        let mut disk = fs.into_device();
        // Simulate an mformat-style volume: the sector count lives in the
        // 16-bit total-sectors field (BPB offset 19) and the 32-bit field
        // (offset 32) is zero.
        disk.data[19] = (TEST_SECTORS & 0xFF) as u8;
        disk.data[20] = ((TEST_SECTORS >> 8) & 0xFF) as u8;
        disk.data[32..36].fill(0);
        let fs2 = Fat32::open(disk).unwrap();
        assert_eq!(fs2.total_bytes(), u64::from(TEST_SECTORS) * 512);
    }

    #[test_case]
    fn test_open_rejects_non_fat32() {
        let disk = MemDisk::new(TEST_SECTORS);
        assert_eq!(Fat32::open(disk).unwrap_err(), FatError::InvalidSignature);
    }

    #[test_case]
    fn test_create_write_read_small_file() {
        let disk = MemDisk::new(TEST_SECTORS);
        let mut fs = Fat32::format(disk, TEST_SECTORS, b"TESTVOLUME ").unwrap();

        fs.create_file("HELLO.TXT").unwrap();
        let data = b"Hello, FAT32!";
        let n = fs.write_file("HELLO.TXT", data).unwrap();
        assert_eq!(n, data.len());

        let mut buf = [0_u8; 64];
        let n = fs.read_file("HELLO.TXT", &mut buf).unwrap();
        assert_eq!(&buf[..n], data);
    }

    #[test_case]
    fn test_write_read_multi_cluster_file() {
        let disk = MemDisk::new(TEST_SECTORS);
        let mut fs = Fat32::format(disk, TEST_SECTORS, b"TESTVOLUME ").unwrap();

        fs.create_file("BIG.BIN").unwrap();
        // 10 KiB spans 5 clusters (2 KiB each) — exercises chain allocation.
        let data: Vec<u8> = (0_usize..10 * 1024).map(|i| (i % 251) as u8).collect();
        fs.write_file("BIG.BIN", &data).unwrap();

        let mut buf = alloc::vec![0_u8; 10 * 1024];
        let n = fs.read_file("BIG.BIN", &mut buf).unwrap();
        assert_eq!(n, data.len());
        assert_eq!(buf, data);
    }

    #[test_case]
    fn test_overwrite_replaces_contents() {
        let disk = MemDisk::new(TEST_SECTORS);
        let mut fs = Fat32::format(disk, TEST_SECTORS, b"TESTVOLUME ").unwrap();

        fs.create_file("F.TXT").unwrap();
        fs.write_file("F.TXT", b"first contents, a bit longer")
            .unwrap();
        fs.write_file("F.TXT", b"second").unwrap();

        let mut buf = [0_u8; 64];
        let n = fs.read_file("F.TXT", &mut buf).unwrap();
        assert_eq!(&buf[..n], b"second");
    }

    #[test_case]
    fn test_list_dir_and_delete() {
        let disk = MemDisk::new(TEST_SECTORS);
        let mut fs = Fat32::format(disk, TEST_SECTORS, b"TESTVOLUME ").unwrap();

        fs.create_file("A.TXT").unwrap();
        fs.create_file("B.TXT").unwrap();
        assert_eq!(fs.list_dir(None).unwrap().len(), 2);

        fs.delete_file("A.TXT").unwrap();
        let names = fs.list_dir(None).unwrap();
        assert_eq!(names.len(), 1);
        assert_eq!(&names[0].name, b"B       TXT");
        assert!(!names[0].is_dir());

        assert_eq!(
            fs.read_file("A.TXT", &mut [0_u8; 8]).unwrap_err(),
            FatError::FileNotFound
        );
    }

    #[test_case]
    fn test_mkdir_creates_nested_dirs() {
        let disk = MemDisk::new(TEST_SECTORS);
        let mut fs = Fat32::format(disk, TEST_SECTORS, b"TESTVOLUME ").unwrap();

        // mkdir -p semantics: parents are created as needed.
        fs.create_dir("/home/Desktop").unwrap();

        let root = fs.list_dir(None).unwrap();
        assert_eq!(root.len(), 1);
        assert_eq!(&root[0].name, b"HOME       ");
        assert!(root[0].is_dir());

        let home = fs.list_dir(Some("/home")).unwrap();
        assert_eq!(home.len(), 1);
        assert_eq!(&home[0].name, b"DESKTOP    ");
        assert!(home[0].is_dir());

        // The new directory starts empty (`.`/`..` are hidden).
        assert!(fs.list_dir(Some("/home/Desktop")).unwrap().is_empty());
    }

    #[test_case]
    fn test_mkdir_is_idempotent_and_validates() {
        let disk = MemDisk::new(TEST_SECTORS);
        let mut fs = Fat32::format(disk, TEST_SECTORS, b"TESTVOLUME ").unwrap();

        fs.create_dir("/home").unwrap();
        // Creating an existing directory is a no-op.
        fs.create_dir("/home").unwrap();
        fs.create_dir("home").unwrap(); // relative form works too

        // A file in the way of a directory component is an error.
        fs.create_file("/BLOCKER").unwrap();
        assert_eq!(
            fs.create_dir("/BLOCKER/sub").unwrap_err(),
            FatError::NotADirectory
        );

        // Empty path is invalid.
        assert_eq!(fs.create_dir("").unwrap_err(), FatError::InvalidName);
        assert_eq!(fs.create_dir("/").unwrap_err(), FatError::InvalidName);
    }

    #[test_case]
    fn test_file_ops_in_subdirectory() {
        let disk = MemDisk::new(TEST_SECTORS);
        let mut fs = Fat32::format(disk, TEST_SECTORS, b"TESTVOLUME ").unwrap();

        fs.create_dir("/home/Desktop").unwrap();
        fs.create_file("/home/Desktop/NOTE.TXT").unwrap();
        let n = fs.write_file("/home/Desktop/NOTE.TXT", b"nested!").unwrap();
        assert_eq!(n, 7);

        let mut buf = [0_u8; 32];
        let n = fs.read_file("/home/Desktop/NOTE.TXT", &mut buf).unwrap();
        assert_eq!(&buf[..n], b"nested!");

        // Root-level file with the same name is a different file.
        fs.create_file("NOTE.TXT").unwrap();
        fs.write_file("NOTE.TXT", b"root").unwrap();
        let n = fs.read_file("NOTE.TXT", &mut buf).unwrap();
        assert_eq!(&buf[..n], b"root");
        let n = fs.read_file("/home/Desktop/NOTE.TXT", &mut buf).unwrap();
        assert_eq!(&buf[..n], b"nested!");

        fs.delete_file("/home/Desktop/NOTE.TXT").unwrap();
        assert_eq!(
            fs.read_file("/home/Desktop/NOTE.TXT", &mut buf)
                .unwrap_err(),
            FatError::FileNotFound
        );
    }

    #[test_case]
    fn test_path_resolution_errors() {
        let disk = MemDisk::new(TEST_SECTORS);
        let mut fs = Fat32::format(disk, TEST_SECTORS, b"TESTVOLUME ").unwrap();

        fs.create_file("/AFILE").unwrap();

        // Missing intermediate directory.
        assert_eq!(
            fs.create_file("/nope/F.TXT").unwrap_err(),
            FatError::FileNotFound
        );
        // File used as a directory.
        assert_eq!(
            fs.create_file("/AFILE/F.TXT").unwrap_err(),
            FatError::NotADirectory
        );
        // File operations on a directory.
        fs.create_dir("/adir").unwrap();
        assert_eq!(
            fs.read_file("/adir", &mut [0_u8; 8]).unwrap_err(),
            FatError::IsADirectory
        );
        assert_eq!(
            fs.write_file("/adir", b"x").unwrap_err(),
            FatError::IsADirectory
        );
        assert_eq!(fs.delete_file("/adir").unwrap_err(), FatError::IsADirectory);
        // Listing a file as a directory.
        assert_eq!(
            fs.list_dir(Some("/AFILE")).unwrap_err(),
            FatError::NotADirectory
        );
        assert_eq!(
            fs.list_dir(Some("/missing")).unwrap_err(),
            FatError::FileNotFound
        );
    }

    #[test_case]
    fn test_list_dir_root_forms() {
        let disk = MemDisk::new(TEST_SECTORS);
        let mut fs = Fat32::format(disk, TEST_SECTORS, b"TESTVOLUME ").unwrap();
        fs.create_file("A.TXT").unwrap();

        assert_eq!(fs.list_dir(None).unwrap().len(), 1);
        assert_eq!(fs.list_dir(Some("")).unwrap().len(), 1);
        assert_eq!(fs.list_dir(Some("/")).unwrap().len(), 1);
    }

    #[test_case]
    fn test_duplicate_create_and_missing_file_errors() {
        let disk = MemDisk::new(TEST_SECTORS);
        let mut fs = Fat32::format(disk, TEST_SECTORS, b"TESTVOLUME ").unwrap();

        fs.create_file("X.TXT").unwrap();
        assert_eq!(fs.create_file("X.TXT").unwrap_err(), FatError::FileExists);
        assert_eq!(
            fs.read_file("NOPE.TXT", &mut [0_u8; 8]).unwrap_err(),
            FatError::FileNotFound
        );
        assert_eq!(
            fs.write_file("NOPE.TXT", b"x").unwrap_err(),
            FatError::FileNotFound
        );
    }

    #[test_case]
    fn test_read_with_small_buffer_fails() {
        let disk = MemDisk::new(TEST_SECTORS);
        let mut fs = Fat32::format(disk, TEST_SECTORS, b"TESTVOLUME ").unwrap();

        fs.create_file("F.TXT").unwrap();
        fs.write_file("F.TXT", b"12345678").unwrap();
        assert_eq!(
            fs.read_file("F.TXT", &mut [0_u8; 4]).unwrap_err(),
            FatError::BufferTooSmall
        );
    }
}

fn short_name(alias: &[u8; 11]) -> String {
    String::from(short_name_into(alias, &mut [0; 23]))
}

// At most eleven Latin-1 bytes encoded as two UTF-8 bytes each, plus a dot.
fn short_name_into<'a>(alias: &[u8; 11], buffer: &'a mut [u8; 23]) -> &'a str {
    let base = alias[..8].iter().position(|&b| b == b' ').unwrap_or(8);
    let ext = alias[8..].iter().position(|&b| b == b' ').unwrap_or(3);
    let mut length = 0;
    for &byte in &alias[..base] {
        length += char::from(byte).encode_utf8(&mut buffer[length..]).len();
    }
    if ext != 0 {
        buffer[length] = b'.';
        length += 1;
        for &byte in &alias[8..8 + ext] {
            length += char::from(byte).encode_utf8(&mut buffer[length..]).len();
        }
    }
    core::str::from_utf8(&buffer[..length]).expect("encoded characters")
}

fn name_equal(a: &str, b: &str) -> bool {
    a.chars()
        .flat_map(char::to_uppercase)
        .eq(b.chars().flat_map(char::to_uppercase))
}

/// Whether a path component uses the legacy short-name spelling.
pub(crate) fn short_component(name: &str) -> bool {
    to_83(name).is_ok()
}
