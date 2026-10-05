//! ATA PIO mode driver for IDE hard disks.
//!
//! This module implements a polling (non-interrupt) ATA driver using Programmed
//! I/O in LBA28 mode. It supports the primary IDE channel and both the master
//! and slave drives on it.
//!
//! # Design notes
//!
//! - All operations are synchronous and poll the status register with a bounded
//!   timeout, so a missing or misbehaving device can never hang the kernel.
//! - Interrupts from the disk are not used; the driver never enables the IDE
//!   IRQ. This keeps the driver self-contained and easy to test.
//! - Only LBA28 addressing is supported (up to 128 GiB), which is more than
//!   enough for the disk images used by this project.
//!
//! # Example
//!
//! ```no_run
//! use self_rust_os::ata::{AtaDisk, AtaDrive};
//!
//! let mut disk = AtaDisk::primary(AtaDrive::Slave);
//! let info = disk.identify().expect("no disk on primary slave").expect("identify failed");
//! let mut sector = [0_u8; 512];
//! disk.read_sector(0, &mut sector).expect("read failed");
//! ```

use core::str::from_utf8;

use x86_64::instructions::port::{Port, PortReadOnly, PortWriteOnly};

/// I/O base port of the primary ATA channel.
pub const PRIMARY_IO_BASE: u16 = 0x1F0;
/// Control base port of the primary ATA channel (alternate status / device control).
pub const PRIMARY_CTRL_BASE: u16 = 0x3F6;
/// I/O base port of the secondary ATA channel.
pub const SECONDARY_IO_BASE: u16 = 0x170;
/// Control base port of the secondary ATA channel.
pub const SECONDARY_CTRL_BASE: u16 = 0x376;

/// Size of one disk sector in bytes.
pub const SECTOR_SIZE: usize = 512;

// Register offsets relative to the I/O base port.
const REG_DATA: u16 = 0;
const REG_SECTOR_COUNT: u16 = 2;
const REG_LBA_LO: u16 = 3;
const REG_LBA_MID: u16 = 4;
const REG_LBA_HI: u16 = 5;
const REG_DRIVE_SELECT: u16 = 6;
const REG_STATUS: u16 = 7;
const REG_COMMAND: u16 = 7;

// Status register bits.
const STATUS_ERR: u8 = 0x01;
const STATUS_DRQ: u8 = 0x08;
const STATUS_BSY: u8 = 0x80;

// ATA commands.
const CMD_IDENTIFY: u8 = 0xEC;
const CMD_READ_SECTORS: u8 = 0x20;
const CMD_WRITE_SECTORS: u8 = 0x30;
const CMD_FLUSH_CACHE: u8 = 0xE7;

// Device Control register bits (control base port).
const CTRL_NIEN: u8 = 0x02; // Bit 1: disable device interrupts (active low logic).

// Drive/head register bits: 0xE0 selects LBA mode, bit 4 selects slave.
const DRIVE_LBA_MODE: u8 = 0xE0;
const DRIVE_SLAVE_BIT: u8 = 0x10;

/// Upper bound for status polling loops.
///
/// This makes every operation fail with [`AtaError::Timeout`] instead of
/// hanging forever when no device (or a broken one) is attached.
const POLL_LIMIT: u32 = 1_000_000;

/// Which device on an ATA channel to talk to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtaDrive {
    /// The master device (drive/head bit 4 = 0).
    Master,
    /// The slave device (drive/head bit 4 = 1).
    Slave,
}

/// Errors that can occur during ATA operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtaError {
    /// The device did not respond in time (or no device is attached).
    Timeout,
    /// The device reported an error via the ERR status bit.
    DeviceError,
}

/// Identification data returned by the ATA IDENTIFY command.
#[derive(Debug, Clone)]
pub struct DiskInfo {
    /// Model string, e.g. `"QEMU HARDDISK"`.
    pub model: [u8; 40],
    /// Serial number string.
    pub serial: [u8; 20],
    /// Total number of addressable 512-byte sectors (LBA28).
    pub sector_count: u32,
}

impl DiskInfo {
    /// Returns the model string trimmed of trailing spaces.
    #[must_use]
    pub fn model_str(&self) -> &str {
        let end = self
            .model
            .iter()
            .rposition(|&b| b != b' ')
            .map_or(0, |i| i + 1);
        from_utf8(&self.model[..end]).unwrap_or("<invalid utf-8>")
    }
}

/// An ATA disk on a given channel.
///
/// The driver uses Programmed I/O and polls the status register; it never
/// relies on disk interrupts.
#[derive(Debug)]
pub struct AtaDisk {
    io_base: u16,
    ctrl_base: u16,
    drive: AtaDrive,
}

impl AtaDisk {
    /// Creates a handle for a drive on the primary IDE channel.
    #[must_use]
    pub const fn primary(drive: AtaDrive) -> Self {
        Self {
            io_base: PRIMARY_IO_BASE,
            ctrl_base: PRIMARY_CTRL_BASE,
            drive,
        }
    }

    /// Creates a handle for a drive on the secondary IDE channel.
    #[must_use]
    pub const fn secondary(drive: AtaDrive) -> Self {
        Self {
            io_base: SECONDARY_IO_BASE,
            ctrl_base: SECONDARY_CTRL_BASE,
            drive,
        }
    }

    /// Reads the alternate status register (does not clear pending interrupts).
    fn alt_status(&mut self) -> u8 {
        // SAFETY: Reading the ATA alternate status port is side-effect free.
        unsafe { PortReadOnly::new(self.ctrl_base).read() }
    }

    /// Waits ~400ns as required by the ATA spec after drive selection,
    /// implemented by reading the alternate status register four times.
    fn delay_400ns(&mut self) {
        for _ in 0..4 {
            self.alt_status();
        }
    }

    /// Disables interrupt generation from the devices on this channel.
    ///
    /// This driver is polling-based and the kernel has no IDE interrupt
    /// handler; an unexpected IRQ 14/15 would raise a double fault. Setting
    /// the `nIEN` bit in the Device Control register stops the devices from
    /// asserting their interrupt line. This is idempotent and is applied
    /// before every command.
    fn disable_device_interrupts(&mut self) {
        // SAFETY: Writing nIEN to the Device Control register is the defined
        // way to suppress interrupts from the ATA channel.
        unsafe {
            PortWriteOnly::new(self.ctrl_base).write(CTRL_NIEN);
        }
    }

    /// Reads the primary status register.
    fn status(&mut self) -> u8 {
        // SAFETY: Reading the ATA status port is side-effect free.
        unsafe { Port::new(self.io_base + REG_STATUS).read() }
    }

    /// Waits until the BSY bit clears, with a bounded timeout.
    fn wait_not_busy(&mut self) -> Result<(), AtaError> {
        for _ in 0..POLL_LIMIT {
            if self.status() & STATUS_BSY == 0 {
                return Ok(());
            }
        }
        Err(AtaError::Timeout)
    }

    /// Waits until the DRQ bit is set (data ready), with a bounded timeout.
    /// Fails if the ERR bit is set while waiting.
    fn wait_data_ready(&mut self) -> Result<(), AtaError> {
        for _ in 0..POLL_LIMIT {
            let status = self.status();
            if status & STATUS_ERR != 0 {
                return Err(AtaError::DeviceError);
            }
            if status & STATUS_DRQ != 0 {
                return Ok(());
            }
        }
        Err(AtaError::Timeout)
    }

    /// Selects this drive and programs the LBA28 address registers.
    fn select_drive_and_lba(&mut self, lba: u32) {
        // SAFETY: Writing the ATA drive/head and LBA ports is the defined way
        // to select a drive and address on the IDE channel.
        unsafe {
            Port::new(self.io_base + REG_DRIVE_SELECT)
                .write(drive_select_byte(self.drive, lba));
            self.delay_400ns();
            PortWriteOnly::new(self.io_base + REG_SECTOR_COUNT).write(0_u8);
            let (lo, mid, hi, _) = split_lba(lba);
            PortWriteOnly::new(self.io_base + REG_LBA_LO).write(lo);
            PortWriteOnly::new(self.io_base + REG_LBA_MID).write(mid);
            PortWriteOnly::new(self.io_base + REG_LBA_HI).write(hi);
        }
    }

    /// Sends an ATA command and waits for the device to become ready.
    fn send_command(&mut self, command: u8) -> Result<(), AtaError> {
        // SAFETY: Writing the ATA command port issues a command to the selected drive.
        unsafe {
            PortWriteOnly::new(self.io_base + REG_COMMAND).write(command);
        }
        self.delay_400ns();
        self.wait_not_busy()
    }

    /// Runs the IDENTIFY command and returns the disk information.
    ///
    /// Returns `Ok(None)` when no device is attached (floating bus or
    /// non-ATA device).
    ///
    /// # Errors
    ///
    /// Returns [`AtaError`] if the device times out or reports an error.
    pub fn identify(&mut self) -> Result<Option<DiskInfo>, AtaError> {
        self.disable_device_interrupts();
        self.wait_not_busy()?;
        self.select_drive_and_lba(0);

        // SAFETY: Issuing IDENTIFY to the selected drive via the command port.
        unsafe {
            PortWriteOnly::new(self.io_base + REG_COMMAND).write(CMD_IDENTIFY);
        }
        self.delay_400ns();

        // Status 0x00 or 0xFF means the bus is floating: no device attached.
        let status = self.status();
        if status == 0 || status == 0xFF {
            return Ok(None);
        }

        self.wait_not_busy()?;

        // After BSY clears, a valid ATA device asserts DRQ (or ERR on failure).
        let status = self.status();
        if status & STATUS_ERR != 0 || status & STATUS_DRQ == 0 {
            return Ok(None);
        }

        let mut words = [0_u16; 256];
        for word in &mut words {
            // SAFETY: The device asserted DRQ, so 256 words are available.
            *word = unsafe { Port::new(self.io_base + REG_DATA).read() };
        }

        let mut model = [0_u8; 40];
        let mut serial = [0_u8; 20];
        parse_ata_string(&words, 27, &mut model);
        parse_ata_string(&words, 10, &mut serial);

        Ok(Some(DiskInfo {
            model,
            serial,
            sector_count: u32::from(words[60]) | (u32::from(words[61]) << 16),
        }))
    }

    /// Reads a single 512-byte sector via LBA28 PIO.
    ///
    /// # Errors
    ///
    /// Returns [`AtaError`] if the device times out or reports an error.
    pub fn read_sector(&mut self, lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), AtaError> {
        self.disable_device_interrupts();
        self.wait_not_busy()?;
        self.select_drive_and_lba(lba);
        // SAFETY: Writing the sector count port is part of the READ command setup.
        unsafe {
            PortWriteOnly::new(self.io_base + REG_SECTOR_COUNT).write(1_u8);
        }
        self.send_command(CMD_READ_SECTORS)?;
        self.wait_data_ready()?;

        for chunk in buf.chunks_exact_mut(2) {
            // SAFETY: The device asserted DRQ, so data words are available.
            let word: u16 = unsafe { Port::new(self.io_base + REG_DATA).read() };
            chunk.copy_from_slice(&word.to_le_bytes());
        }

        self.delay_400ns();
        Ok(())
    }

    /// Writes a single 512-byte sector via LBA28 PIO and flushes the cache.
    ///
    /// # Errors
    ///
    /// Returns [`AtaError`] if the device times out or reports an error.
    pub fn write_sector(&mut self, lba: u32, buf: &[u8; SECTOR_SIZE]) -> Result<(), AtaError> {
        self.disable_device_interrupts();
        self.wait_not_busy()?;
        self.select_drive_and_lba(lba);
        // SAFETY: Writing the sector count port is part of the WRITE command setup.
        unsafe {
            PortWriteOnly::new(self.io_base + REG_SECTOR_COUNT).write(1_u8);
        }
        self.send_command(CMD_WRITE_SECTORS)?;
        self.wait_data_ready()?;

        for chunk in buf.chunks_exact(2) {
            let word = u16::from_le_bytes([chunk[0], chunk[1]]);
            // SAFETY: The device asserted DRQ, so it accepts data words.
            unsafe {
                PortWriteOnly::new(self.io_base + REG_DATA).write(word);
            }
        }

        // Flush the device cache so the data is durable before we return.
        self.send_command(CMD_FLUSH_CACHE)?;
        self.delay_400ns();
        Ok(())
    }
}

/// Computes the drive/head register value for LBA28 addressing.
///
/// Bit layout: `1 1 1 D 0 LBA27-24` where D selects slave (1) or master (0).
#[must_use]
pub const fn drive_select_byte(drive: AtaDrive, lba: u32) -> u8 {
    let slave_bit = match drive {
        AtaDrive::Master => 0,
        AtaDrive::Slave => DRIVE_SLAVE_BIT,
    };
    DRIVE_LBA_MODE | slave_bit | ((lba >> 24) as u8 & 0x0F)
}

/// Splits a 28-bit LBA into the (lo, mid, hi, top) register bytes.
#[must_use]
pub const fn split_lba(lba: u32) -> (u8, u8, u8, u8) {
    (
        (lba & 0xFF) as u8,
        ((lba >> 8) & 0xFF) as u8,
        ((lba >> 16) & 0xFF) as u8,
        ((lba >> 24) & 0x0F) as u8,
    )
}

/// Extracts an ATA string (byte-swapped words) from an IDENTIFY buffer.
///
/// ATA strings are stored as words with the two bytes swapped; this function
/// unswaps words starting at `start_word` into `out`.
pub fn parse_ata_string(words: &[u16; 256], start_word: usize, out: &mut [u8]) {
    for (i, slot) in out.iter_mut().enumerate() {
        let word = words[start_word + (i >> 1)];
        *slot = if i % 2 == 0 {
            (word >> 8) as u8
        } else {
            (word & 0xFF) as u8
        };
    }
}

/// Returns `true` if the BSY (busy) status bit is set.
#[must_use]
pub const fn status_is_busy(status: u8) -> bool {
    status & STATUS_BSY != 0
}

/// Returns `true` if the ERR (error) status bit is set.
#[must_use]
pub const fn status_is_error(status: u8) -> bool {
    status & STATUS_ERR != 0
}

/// Returns `true` if the DRQ (data request) status bit is set.
#[must_use]
pub const fn status_data_ready(status: u8) -> bool {
    status & STATUS_DRQ != 0
}

#[cfg(test)]
mod tests {
    use super::{drive_select_byte, parse_ata_string, split_lba, status_data_ready, status_is_busy, status_is_error, AtaDrive};

    #[test_case]
    fn test_drive_select_byte_master_lba_mode() {
        // Master, LBA 0: 0xE0, slave bit clear, top nibble 0.
        assert_eq!(drive_select_byte(AtaDrive::Master, 0), 0xE0);
    }

    #[test_case]
    fn test_drive_select_byte_slave_sets_bit4() {
        assert_eq!(drive_select_byte(AtaDrive::Slave, 0), 0xF0);
    }

    #[test_case]
    fn test_drive_select_byte_carries_lba_top_nibble() {
        // LBA 0x0ABC_DEF0 -> top nibble 0xA.
        assert_eq!(drive_select_byte(AtaDrive::Master, 0x0ABC_DEF0), 0xEA);
        assert_eq!(drive_select_byte(AtaDrive::Slave, 0x0ABC_DEF0), 0xFA);
    }

    #[test_case]
    fn test_split_lba_byte_order() {
        assert_eq!(split_lba(0x0A1B_2C3D), (0x3D, 0x2C, 0x1B, 0x0A));
        assert_eq!(split_lba(0), (0, 0, 0, 0));
        // Only 28 bits are kept.
        assert_eq!(split_lba(0xFFFF_FFFF), (0xFF, 0xFF, 0xFF, 0x0F));
    }

    #[test_case]
    fn test_status_bit_helpers() {
        assert!(status_is_busy(0x80));
        assert!(!status_is_busy(0x50));
        assert!(status_is_error(0x01));
        assert!(!status_is_error(0x50));
        assert!(status_data_ready(0x08));
        assert!(!status_data_ready(0x50));
    }

    #[test_case]
    fn test_parse_ata_string_unswaps_bytes() {
        // In ATA, "AB" is stored as word 0x4142 ('A' << 8 | 'B').
        let mut words = [0_u16; 256];
        words[27] = 0x4142;
        words[28] = 0x4344;
        let mut model = [0_u8; 4];
        parse_ata_string(&words, 27, &mut model);
        assert_eq!(&model, b"ABCD");
    }

    #[test_case]
    fn test_disk_info_model_str_trims_spaces() {
        let mut model = [b' '; 40];
        model[..4].copy_from_slice(b"QEMU");
        let info = super::DiskInfo {
            model,
            serial: [b' '; 20],
            sector_count: 32768,
        };
        assert_eq!(info.model_str(), "QEMU");
    }
}
