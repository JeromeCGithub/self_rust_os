//! Integration test for the ATA PIO driver.
//!
//! Boots the kernel in QEMU with a 16 MiB test disk attached as the primary
//! slave (`tests/fixtures/disk-ata.img`, see `e2e/ata.sh`), then exercises
//! the real hardware path: IDENTIFY, sector write, sector read-back, and
//! checksum verification.
//!
//! The test only touches the last 8 sectors of the disk so it never
//! interferes with the boot disk (primary master) or with the FAT32 test
//! fixture, which uses the start of its own image.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(self_rust_os::test_runner)]
#![reexport_test_harness_main = "test_main"]
#![allow(missing_docs)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::missing_assert_message)]

mod common;

use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;
use self_rust_os::{
    ata::{AtaDisk, AtaDrive, SECTOR_SIZE},
    println, serial_println,
};
use x86_64::VirtAddr;

entry_point!(main);

#[expect(clippy::undocumented_unsafe_blocks)]
fn main(boot_info: &'static BootInfo) -> ! {
    let phys_mem_offset = VirtAddr::new(boot_info.physical_memory_offset);
    self_rust_os::init(phys_mem_offset, common::FRAMEBUFFER_INFO, common::framebuffer_addr());

    test_main();
    #[expect(clippy::empty_loop)]
    loop {}
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    self_rust_os::test_panic_handler(info)
}

/// Number of sectors exercised by the round-trip test.
const TEST_SECTORS: u32 = 8;

/// Deterministic test pattern: every byte is a pure function of the LBA and
/// the byte offset, so the test needs no RNG and is fully reproducible.
fn pattern_byte(lba: u32, offset: usize) -> u8 {
    (lba as u8) ^ (offset as u8) ^ ((offset >> 8) as u8) ^ 0xA5
}

/// FNV-1a checksum used as an integrity metric for the round-trip.
fn checksum(buf: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in buf {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[test_case]
fn when_identifying_primary_slave_then_disk_is_present() {
    let mut disk = AtaDisk::primary(AtaDrive::Slave);
    let info = disk
        .identify()
        .expect("ATA IDENTIFY timed out or errored")
        .expect("no disk on primary slave; run e2e/ata.sh to attach tests/fixtures/disk-ata.img");

    serial_println!(
        "[ata] model=\"{}\" sectors={} ({} MiB)",
        info.model_str(),
        info.sector_count,
        info.sector_count / 2048
    );
    println!(
        "[ata] disk: {} ({} MiB)",
        info.model_str(),
        info.sector_count / 2048
    );
    assert!(info.sector_count >= 2048, "test disk must be at least 1 MiB");
    assert!(
        info.model_str().contains("QEMU"),
        "expected QEMU test disk, got \"{}\"",
        info.model_str()
    );
}

#[test_case]
fn when_writing_and_reading_sectors_then_data_round_trips() {
    let mut disk = AtaDisk::primary(AtaDrive::Slave);
    let info = disk
        .identify()
        .expect("ATA IDENTIFY failed")
        .expect("no disk on primary slave");

    // Use the last TEST_SECTORS of the disk: far away from sector 0 and from
    // anything the FAT32 tests might use.
    let base_lba = info.sector_count - TEST_SECTORS;

    let mut write_buf = [0u8; SECTOR_SIZE];
    let mut read_buf = [0u8; SECTOR_SIZE];
    let mut total_checksum: u64 = 0;

    for i in 0..TEST_SECTORS {
        let lba = base_lba + i;
        for (offset, slot) in write_buf.iter_mut().enumerate() {
            *slot = pattern_byte(lba, offset);
        }
        disk.write_sector(lba, &write_buf)
            .expect("ATA sector write failed");

        read_buf.fill(0);
        disk.read_sector(lba, &mut read_buf)
            .expect("ATA sector read failed");

        assert_eq!(
            write_buf, read_buf,
            "round-trip mismatch at LBA {lba}"
        );
        total_checksum ^= checksum(&read_buf);
    }

    serial_println!(
        "[ata] round-trip OK: {} sectors ({} bytes), checksum={:#018x}",
        TEST_SECTORS,
        TEST_SECTORS as usize * SECTOR_SIZE,
        total_checksum
    );
    println!(
        "[ata] round-trip OK: {} sectors, checksum={:#018x}",
        TEST_SECTORS, total_checksum
    );
}
