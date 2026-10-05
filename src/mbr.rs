//! MBR partition table parsing and partitioned block devices.
//!
//! When booting from a GRUB-installed disk, the kernel lives on a FAT32
//! *partition*, not at a raw disk offset. This module parses the classic MBR
//! partition table (sector 0) to locate the FAT32 partition, and provides
//! [`OffsetDisk`], a [`BlockDevice`](crate::fat32::BlockDevice) adapter that
//! shifts sector numbers by the partition's start LBA.

use crate::{
    ata::{AtaDisk, AtaError, SECTOR_SIZE},
    fat32::BlockDevice,
};

/// Byte offset of the partition table inside the MBR sector.
const PARTITION_TABLE_OFFSET: usize = 0x1BE;
/// Number of partition entries.
const PARTITION_COUNT: usize = 4;
/// Size of one partition entry in bytes.
const PARTITION_ENTRY_SIZE: usize = 16;
/// MBR boot signature (`0xAA55`, little-endian).
const MBR_SIGNATURE: u16 = 0xAA55;

/// Partition type for FAT32 (CHS-addressed).
const PART_FAT32_CHS: u8 = 0x0B;
/// Partition type for FAT32 (LBA-addressed).
const PART_FAT32_LBA: u8 = 0x0C;

/// Errors from MBR parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MbrError {
    /// The disk did not return sector 0.
    Io,
    /// Sector 0 lacks the `0xAA55` boot signature.
    InvalidSignature,
}

/// A parsed MBR partition entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Partition {
    /// Whether the bootable flag (`0x80`) is set.
    pub bootable: bool,
    /// Partition type byte (e.g. `0x0C` for FAT32 LBA).
    pub kind: u8,
    /// LBA of the partition's first sector.
    pub lba_start: u32,
    /// Number of sectors in the partition.
    pub sector_count: u32,
}

impl Partition {
    /// Returns `true` for FAT32 partition types.
    #[must_use]
    pub const fn is_fat32(self) -> bool {
        self.kind == PART_FAT32_CHS || self.kind == PART_FAT32_LBA
    }
}

/// Parses the four MBR partition entries from a 512-byte sector.
///
/// # Errors
///
/// Returns [`MbrError::InvalidSignature`] when the boot signature is wrong.
pub fn parse_mbr(sector: &[u8; SECTOR_SIZE]) -> Result<[Partition; PARTITION_COUNT], MbrError> {
    let signature = u16::from_le_bytes([sector[510], sector[511]]);
    if signature != MBR_SIGNATURE {
        return Err(MbrError::InvalidSignature);
    }

    let mut partitions = [Partition {
        bootable: false,
        kind: 0,
        lba_start: 0,
        sector_count: 0,
    }; PARTITION_COUNT];

    for (i, slot) in partitions.iter_mut().enumerate() {
        let base = PARTITION_TABLE_OFFSET + i * PARTITION_ENTRY_SIZE;
        let entry = &sector[base..base + PARTITION_ENTRY_SIZE];
        *slot = Partition {
            bootable: entry[0] == 0x80,
            kind: entry[4],
            lba_start: u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]),
            sector_count: u32::from_le_bytes([entry[12], entry[13], entry[14], entry[15]]),
        };
    }

    Ok(partitions)
}

/// Reads sector 0 from `disk` and parses its partition table.
///
/// # Errors
///
/// Returns [`MbrError::Io`] on disk failure, or
/// [`MbrError::InvalidSignature`] for a bad signature.
pub fn read_partition_table(
    disk: &mut AtaDisk,
) -> Result<[Partition; PARTITION_COUNT], MbrError> {
    let mut sector = [0_u8; SECTOR_SIZE];
    disk.read_sector(0, &mut sector).map_err(|_: AtaError| MbrError::Io)?;
    parse_mbr(&sector)
}

/// Returns the first FAT32 partition, if any.
#[must_use]
pub fn find_first_fat32(partitions: &[Partition; PARTITION_COUNT]) -> Option<Partition> {
    partitions.iter().find(|p| p.is_fat32()).copied()
}

/// A block device shifted by a fixed LBA offset.
///
/// Used to expose an MBR partition as a standalone disk: sector `n` of the
/// partition is sector `n + lba_offset` of the underlying disk.
#[derive(Debug)]
pub struct OffsetDisk<D> {
    inner: D,
    lba_offset: u32,
}

impl<D> OffsetDisk<D> {
    /// Wraps `inner`, shifting all sector accesses by `lba_offset`.
    #[must_use]
    pub const fn new(inner: D, lba_offset: u32) -> Self {
        Self { inner, lba_offset }
    }

    /// Returns the underlying disk.
    #[must_use]
    pub fn into_inner(self) -> D {
        self.inner
    }
}

impl<D: BlockDevice> BlockDevice for OffsetDisk<D> {
    type Error = D::Error;

    fn read_sector(&mut self, lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), Self::Error> {
        self.inner
            .read_sector(lba.wrapping_add(self.lba_offset), buf)
    }

    fn write_sector(&mut self, lba: u32, buf: &[u8; 512]) -> Result<(), Self::Error> {
        self.inner
            .write_sector(lba.wrapping_add(self.lba_offset), buf)
    }
}

#[cfg(test)]
mod tests {
    use super::{find_first_fat32, parse_mbr, MbrError, PARTITION_COUNT};

    /// Builds a fake MBR sector with the given partition entries.
    fn fake_mbr(entries: &[(u8, u8, u32, u32)]) -> [u8; 512] {
        let mut sector = [0_u8; 512];
        for (i, &(bootable, kind, lba_start, sector_count)) in entries.iter().enumerate() {
            let base = 0x1BE + i * 16;
            sector[base] = bootable;
            sector[base + 4] = kind;
            sector[base + 8..base + 12].copy_from_slice(&lba_start.to_le_bytes());
            sector[base + 12..base + 16].copy_from_slice(&sector_count.to_le_bytes());
        }
        sector[510] = 0x55;
        sector[511] = 0xAA;
        sector
    }

    #[test_case]
    fn test_parse_mbr_finds_fat32() {
        let sector = fake_mbr(&[
            (0x00, 0x0C, 2048, 253952), // FAT32 LBA partition
            (0x00, 0x83, 256000, 10000), // Linux partition
        ]);
        let parts = parse_mbr(&sector).expect("valid MBR must parse");
        assert_eq!(parts[0].lba_start, 2048);
        assert_eq!(parts[0].sector_count, 253952);
        assert!(parts[0].is_fat32());
        assert!(!parts[1].is_fat32());

        let fat32 = find_first_fat32(&parts).expect("FAT32 partition must be found");
        assert_eq!(fat32.lba_start, 2048);
    }

    #[test_case]
    fn test_parse_mbr_rejects_bad_signature() {
        let mut sector = fake_mbr(&[(0x80, 0x0C, 2048, 100)]);
        sector[510] = 0x00;
        let err = parse_mbr(&sector).expect_err("bad signature must fail");
        assert_eq!(err, MbrError::InvalidSignature);
    }

    #[test_case]
    fn test_parse_mbr_empty_table() {
        let sector = fake_mbr(&[]);
        let parts: [_; PARTITION_COUNT] = parse_mbr(&sector).expect("empty MBR must parse");
        assert!(find_first_fat32(&parts).is_none());
        assert!(parts.iter().all(|p| p.kind == 0));
    }

    #[test_case]
    fn test_offset_disk_shifts_sectors() {
        use crate::fat32::BlockDevice;

        // Minimal in-memory mock disk: sector N contains byte N.
        struct MockDisk {
            sectors: [[u8; 512]; 4],
        }
        impl BlockDevice for MockDisk {
            type Error = core::convert::Infallible;
            fn read_sector(
                &mut self,
                lba: u32,
                buf: &mut [u8; 512],
            ) -> Result<(), Self::Error> {
                buf.copy_from_slice(&self.sectors[lba as usize]);
                Ok(())
            }
            fn write_sector(
                &mut self,
                _lba: u32,
                _buf: &[u8; 512],
            ) -> Result<(), Self::Error> {
                Ok(())
            }
        }

        let mut sectors = [[0_u8; 512]; 4];
        for (i, s) in sectors.iter_mut().enumerate() {
            s[0] = i as u8;
        }
        let mut disk = super::OffsetDisk::new(MockDisk { sectors }, 2);

        let mut buf = [0_u8; 512];
        disk.read_sector(0, &mut buf).unwrap();
        assert_eq!(buf[0], 2, "partition sector 0 must read disk sector 2");
        disk.read_sector(1, &mut buf).unwrap();
        assert_eq!(buf[0], 3, "partition sector 1 must read disk sector 3");
    }
}
