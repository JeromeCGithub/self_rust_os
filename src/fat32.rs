//! FAT32 filesystem.
//!
//! A small, self-contained FAT32 implementation used as the on-disk filesystem.
//! It is generic over a [`BlockDevice`] so the logic can be unit-tested
//! against a RAM-backed mock and used with the real [`AtaDisk`](crate::ata::AtaDisk)
//! in production.
//!
//! # Scope (v1)
//!
//! - Formats disks, creates/reads/writes/deletes files in the **root directory**
//!   (no subdirectories yet).
//! - 8.3 short filenames (`"README.TXT"`).
//! - Single-threaded; the caller must serialize access.
//!
//! # Example
//!
//! ```no_run
//! use self_rust_os::ata::{AtaDisk, AtaDrive};
//! use self_rust_os::fat32::Fat32;
//!
//! let disk = AtaDisk::secondary(AtaDrive::Master);
//! let mut fs = Fat32::format(disk, 32768, b"RUSTOS     ").expect("format failed");
//! fs.create_file("HELLO.TXT").expect("create failed");
//! fs.write_file("HELLO.TXT", b"Hello, disk!").expect("write failed");
//!
//! let mut buf = [0u8; 64];
//! let n = fs.read_file("HELLO.TXT", &mut buf).expect("read failed");
//! assert_eq!(&buf[..n], b"Hello, disk!");
//! ```
//!
//! # Implementation notes
//!
//! Sector/cluster arithmetic intentionally uses truncating integer division
//! and `as` casts: all values are sector or cluster indices bounded by the
//! 32-bit LBA space, so truncation cannot occur in practice.

#![allow(clippy::integer_division, reason = "Sector/cluster math uses truncating division by design")]
#![allow(clippy::cast_possible_truncation, reason = "LBA/cluster indices fit in u32 by construction")]

use alloc::vec::Vec;
use core::{cmp, fmt::Debug};

use crate::ata::{AtaDisk, AtaError, SECTOR_SIZE};

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
    /// A file with this name already exists.
    FileExists,
    /// No free clusters left on the disk.
    DiskFull,
    /// No free directory entries left.
    DirFull,
    /// The read buffer is smaller than the file.
    BufferTooSmall,
}

// Layout constants for the volumes created by `format`.
const SECTORS_PER_CLUSTER: u8 = 4;
const RESERVED_SECTORS: u16 = 32;
const NUM_FATS: u8 = 2;
const ROOT_CLUSTER: u32 = 2;
const FSINFO_SECTOR: u32 = 1;
const BACKUP_BOOT_SECTOR: u32 = 6;
const DIR_ENTRY_SIZE: usize = 32;
const ENTRIES_PER_CLUSTER: usize = (SECTOR_SIZE * SECTORS_PER_CLUSTER as usize) / DIR_ENTRY_SIZE;

// FAT32 cluster chain markers.
const FAT_FREE: u32 = 0x0000_0000;
const FAT_EOC: u32 = 0x0FFF_FFFF;
const FAT_MASK: u32 = 0x0FFF_FFFF;

// Directory entry attribute bits.
const ATTR_ARCHIVE: u8 = 0x20;

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

    /// Creates an empty file with the given 8.3 name (e.g. `"HELLO.TXT"`).
    ///
    /// # Errors
    ///
    /// Returns [`FatError::InvalidName`] for bad names, [`FatError::FileExists`]
    /// if the file exists, [`FatError::DirFull`] if the root directory is full.
    pub fn create_file(&mut self, name: &str) -> Result<(), FatError> {
        let name_83 = to_83(name)?;
        if self.find_entry(&name_83)?.is_some() {
            return Err(FatError::FileExists);
        }
        let (cluster, index) = self.find_free_entry()?.ok_or(FatError::DirFull)?;
        let mut entry = [0_u8; DIR_ENTRY_SIZE];
        entry[0..11].copy_from_slice(&name_83);
        entry[11] = ATTR_ARCHIVE;
        // first cluster = 0, size = 0 for an empty file.
        self.write_dir_entry(cluster, index, &entry)?;
        Ok(())
    }

    /// Writes `data` to the file, replacing its previous contents.
    ///
    /// The file must exist (see [`create_file`](Self::create_file)). Returns
    /// the number of bytes written.
    ///
    /// # Errors
    ///
    /// Returns [`FatError::FileNotFound`], [`FatError::DiskFull`], or
    /// [`FatError::DeviceError`].
    pub fn write_file(&mut self, name: &str, data: &[u8]) -> Result<usize, FatError> {
        let name_83 = to_83(name)?;
        let (dir_cluster, dir_index) =
            self.find_entry(&name_83)?.ok_or(FatError::FileNotFound)?;

        // Free the previous cluster chain, if any.
        let old_start = self.dir_entry_cluster(dir_cluster, dir_index)?;
        if old_start != 0 {
            self.free_chain(old_start)?;
        }

        // Allocate and fill clusters.
        let bytes_per_cluster = usize::from(self.sectors_per_cluster) * SECTOR_SIZE;
        let mut first_cluster = 0_u32;
        let mut prev_cluster = 0_u32;
        let mut written = 0_usize;

        if !data.is_empty() {
            for chunk in data.chunks(bytes_per_cluster) {
                let cluster = self.allocate_cluster()?;
                if first_cluster == 0 {
                    first_cluster = cluster;
                }
                if prev_cluster != 0 {
                    self.write_fat_entry(prev_cluster, cluster)?;
                }
                prev_cluster = cluster;

                let mut buf = alloc::vec![0_u8; bytes_per_cluster];
                buf[..chunk.len()].copy_from_slice(chunk);
                self.write_cluster(cluster, &buf)?;
                written += chunk.len();
            }
            self.write_fat_entry(prev_cluster, FAT_EOC)?;
        }

        // Update the directory entry: first cluster + size.
        let mut entry = [0_u8; DIR_ENTRY_SIZE];
        self.read_dir_entry(dir_cluster, dir_index, &mut entry)?;
        entry[20..22].copy_from_slice(&((first_cluster >> 16) as u16).to_le_bytes());
        entry[26..28].copy_from_slice(&(first_cluster as u16).to_le_bytes());
        let size = u32::try_from(data.len()).map_err(|_| FatError::DiskFull)?;
        entry[28..32].copy_from_slice(&size.to_le_bytes());
        self.write_dir_entry(dir_cluster, dir_index, &entry)?;

        Ok(written)
    }

    /// Reads the file into `buf`, returning the number of bytes read.
    ///
    /// # Errors
    ///
    /// Returns [`FatError::FileNotFound`] if the file does not exist,
    /// [`FatError::BufferTooSmall`] if `buf` is smaller than the file.
    pub fn read_file(&mut self, name: &str, buf: &mut [u8]) -> Result<usize, FatError> {
        let name_83 = to_83(name)?;
        let (dir_cluster, dir_index) =
            self.find_entry(&name_83)?.ok_or(FatError::FileNotFound)?;

        let mut entry = [0_u8; DIR_ENTRY_SIZE];
        self.read_dir_entry(dir_cluster, dir_index, &mut entry)?;
        let size = u32::from_le_bytes([entry[28], entry[29], entry[30], entry[31]]) as usize;
        if buf.len() < size {
            return Err(FatError::BufferTooSmall);
        }

        let first_cluster = self.dir_entry_cluster(dir_cluster, dir_index)?;
        if size == 0 || first_cluster == 0 {
            return Ok(0);
        }

        let bytes_per_cluster = usize::from(self.sectors_per_cluster) * SECTOR_SIZE;
        let mut cluster = first_cluster;
        let mut read = 0_usize;
        let mut cluster_buf = alloc::vec![0_u8; bytes_per_cluster];

        while read < size {
            self.read_cluster(cluster, &mut cluster_buf)?;
            let take = cmp::min(size - read, bytes_per_cluster);
            buf[read..read + take].copy_from_slice(&cluster_buf[..take]);
            read += take;
            if read < size {
                cluster = self.read_fat_entry(cluster)?;
                if cluster >= FAT_EOC {
                    return Err(FatError::DeviceError);
                }
            }
        }
        Ok(read)
    }

    /// Deletes the file, freeing its clusters and directory entry.
    ///
    /// # Errors
    ///
    /// Returns [`FatError::FileNotFound`] if the file does not exist.
    pub fn delete_file(&mut self, name: &str) -> Result<(), FatError> {
        let name_83 = to_83(name)?;
        let (dir_cluster, dir_index) =
            self.find_entry(&name_83)?.ok_or(FatError::FileNotFound)?;

        let start = self.dir_entry_cluster(dir_cluster, dir_index)?;
        if start != 0 {
            self.free_chain(start)?;
        }

        // Mark the entry as free (0xE5).
        let lba = self.dir_entry_lba(dir_cluster, dir_index);
        let sector_offset = (dir_index % (SECTOR_SIZE / DIR_ENTRY_SIZE)) * DIR_ENTRY_SIZE;
        let mut sector = [0_u8; SECTOR_SIZE];
        self.read_sector_raw(lba, &mut sector)?;
        sector[sector_offset] = ENTRY_FREE;
        self.write_sector_raw(lba, &sector)?;
        Ok(())
    }

    /// Lists the names in the root directory as 8.3 byte arrays.
    ///
    /// # Errors
    ///
    /// Returns [`FatError::DeviceError`] on I/O failure.
    pub fn list_dir(&mut self) -> Result<Vec<[u8; 11]>, FatError> {
        let mut names = Vec::new();
        let mut cluster = self.root_cluster;
        loop {
            for index in 0..ENTRIES_PER_CLUSTER {
                let mut entry = [0_u8; DIR_ENTRY_SIZE];
                self.read_dir_entry(cluster, index, &mut entry)?;
                if entry[0] == ENTRY_END {
                    return Ok(names);
                }
                if entry[0] == ENTRY_FREE {
                    continue;
                }
                let mut name = [0_u8; 11];
                name.copy_from_slice(&entry[0..11]);
                names.push(name);
            }
            let next = self.read_fat_entry(cluster)?;
            if next >= FAT_EOC {
                return Ok(names);
            }
            cluster = next;
        }
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
        let lba = self.cluster_to_lba(cluster);
        for (i, chunk) in buf.chunks_mut(SECTOR_SIZE).enumerate() {
            let mut sector = [0_u8; SECTOR_SIZE];
            self.read_sector_raw(lba + i as u32, &mut sector)?;
            chunk.copy_from_slice(&sector[..chunk.len()]);
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
        let bytes = usize::from(self.sectors_per_cluster) * SECTOR_SIZE;
        self.write_cluster(cluster, &alloc::vec![0_u8; bytes])
    }

    /// Reads a FAT entry, masking to 28 bits.
    fn read_fat_entry(&mut self, cluster: u32) -> Result<u32, FatError> {
        let fat_offset = cluster * 4;
        let sector = u32::from(self.reserved_sectors) + fat_offset / SECTOR_SIZE as u32;
        let entry_offset = (fat_offset % SECTOR_SIZE as u32) as usize;
        let mut buf = [0_u8; SECTOR_SIZE];
        self.read_sector_raw(sector, &mut buf)?;
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
            let lba =
                u32::from(self.reserved_sectors) + u32::from(f) * self.fat_size_sectors + sector_in_fat;
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
        for cluster in 2..cluster_count + 2 {
            if self.read_fat_entry(cluster)? == FAT_FREE {
                self.write_fat_entry(cluster, FAT_EOC)?;
                self.zero_cluster(cluster)?;
                return Ok(cluster);
            }
        }
        Err(FatError::DiskFull)
    }

    /// Releases a cluster chain back to the free pool.
    fn free_chain(&mut self, start: u32) -> Result<(), FatError> {
        let mut cluster = start;
        loop {
            let next = self.read_fat_entry(cluster)?;
            self.write_fat_entry(cluster, FAT_FREE)?;
            if next >= FAT_EOC {
                return Ok(());
            }
            cluster = next;
        }
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

    /// Finds a directory entry by 8.3 name; returns `(cluster, index)`.
    fn find_entry(&mut self, name_83: &[u8; 11]) -> Result<Option<(u32, usize)>, FatError> {
        let mut cluster = self.root_cluster;
        loop {
            for index in 0..ENTRIES_PER_CLUSTER {
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
            if next >= FAT_EOC {
                return Ok(None);
            }
            cluster = next;
        }
    }

    /// Finds a free directory entry slot; extends the directory if needed.
    fn find_free_entry(&mut self) -> Result<Option<(u32, usize)>, FatError> {
        let mut cluster = self.root_cluster;
        loop {
            for index in 0..ENTRIES_PER_CLUSTER {
                let mut entry = [0_u8; DIR_ENTRY_SIZE];
                self.read_dir_entry(cluster, index, &mut entry)?;
                if entry[0] == ENTRY_END || entry[0] == ENTRY_FREE {
                    return Ok(Some((cluster, index)));
                }
            }
            let next = self.read_fat_entry(cluster)?;
            if next >= FAT_EOC {
                let new_cluster = self.allocate_cluster()?;
                self.write_fat_entry(cluster, new_cluster)?;
                return Ok(Some((new_cluster, 0)));
            }
            cluster = next;
        }
    }
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
        b'A'..=b'Z' | b'0'..=b'9' | b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'(' | b')'
        | b'-' | b'@' | b'^' | b'_' | b'`' | b'{' | b'}' | b'~' => Ok(upper),
        _ => Err(FatError::InvalidName),
    }
}

#[cfg(test)]
mod tests {
    use core::convert::Infallible;

    use super::{to_83, BlockDevice, Fat32, FatError, SECTOR_SIZE};
    use alloc::vec::Vec;

    /// RAM-backed block device for unit tests.
    #[derive(Debug)]
    struct MemDisk {
        data: Vec<u8>,
    }

    impl MemDisk {
        fn new(sectors: u32) -> Self {
            Self {
                data: alloc::vec![0_u8; sectors as usize * SECTOR_SIZE],
            }
        }
    }

    impl BlockDevice for MemDisk {
        type Error = Infallible;

        fn read_sector(&mut self, lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), Self::Error> {
            let start = lba as usize * SECTOR_SIZE;
            buf.copy_from_slice(&self.data[start..start + SECTOR_SIZE]);
            Ok(())
        }

        fn write_sector(&mut self, lba: u32, buf: &[u8; 512]) -> Result<(), Self::Error> {
            let start = lba as usize * SECTOR_SIZE;
            self.data[start..start + SECTOR_SIZE].copy_from_slice(buf);
            Ok(())
        }
    }

    // Small enough to fit in the 1 MiB kernel heap with room to spare.
    const TEST_SECTORS: u32 = 512;

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
        assert_eq!(
            Fat32::open(disk).unwrap_err(),
            FatError::InvalidSignature
        );
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
        fs.write_file("F.TXT", b"first contents, a bit longer").unwrap();
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
        assert_eq!(fs.list_dir().unwrap().len(), 2);

        fs.delete_file("A.TXT").unwrap();
        let names = fs.list_dir().unwrap();
        assert_eq!(names.len(), 1);
        assert_eq!(&names[0], b"B       TXT");

        assert_eq!(
            fs.read_file("A.TXT", &mut [0_u8; 8]).unwrap_err(),
            FatError::FileNotFound
        );
    }

    #[test_case]
    fn test_duplicate_create_and_missing_file_errors() {
        let disk = MemDisk::new(TEST_SECTORS);
        let mut fs = Fat32::format(disk, TEST_SECTORS, b"TESTVOLUME ").unwrap();

        fs.create_file("X.TXT").unwrap();
        assert_eq!(
            fs.create_file("X.TXT").unwrap_err(),
            FatError::FileExists
        );
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
