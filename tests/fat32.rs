//! Integration test for the FAT32 filesystem.
//!
//! Boots the kernel in QEMU with a 16 MiB test disk attached as the secondary
//! master (`tests/fixtures/disk-fat32.img`, see `e2e/fat32.sh`), then exercises
//! the real hardware path: format, file create/write/read/delete, and
//! directory listing through the ATA driver.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(self_rust_os::test_runner)]
#![reexport_test_harness_main = "test_main"]
#![allow(missing_docs)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::missing_assert_message)]

extern crate alloc;

use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;
use self_rust_os::{
    allocator,
    ata::{AtaDisk, AtaDrive},
    fat32::Fat32,
    memory::{self, BootInfoFrameAllocator},
    println, serial_println,
};
use x86_64::VirtAddr;

entry_point!(main);

#[expect(clippy::undocumented_unsafe_blocks)]
#[expect(clippy::expect_used)]
fn main(boot_info: &'static BootInfo) -> ! {
    let phys_mem_offset = VirtAddr::new(boot_info.physical_memory_offset);
    self_rust_os::init(phys_mem_offset);
    let mut mapper = unsafe { memory::init(phys_mem_offset) };
    let mut frame_allocator = unsafe { BootInfoFrameAllocator::new(&boot_info.memory_map) };
    allocator::init_heap(&mut mapper, &mut frame_allocator).expect("heap initialization failed");

    test_main();
    #[expect(clippy::empty_loop)]
    loop {}
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    self_rust_os::test_panic_handler(info)
}

/// Total sectors of the 16 MiB test disk.
const DISK_SECTORS: u32 = 32768;

#[test_case]
fn when_formatting_then_opening_then_volume_is_valid() {
    let disk = AtaDisk::secondary(AtaDrive::Master);
    let fs = Fat32::format(disk, DISK_SECTORS, b"RUSTOS     ").expect("format failed");
    serial_println!("[fat32] formatted {} bytes", fs.total_bytes());
    println!("[fat32] formatted {} MiB", fs.total_bytes() / (1024 * 1024));

    // Re-open from the raw device to prove the on-disk structures parse.
    let disk = fs.into_device();
    let fs = Fat32::open(disk).expect("open failed");
    assert_eq!(fs.total_bytes(), u64::from(DISK_SECTORS) * 512);
    serial_println!("[fat32] open OK, volume size verified");
}

#[test_case]
fn when_writing_and_reading_files_then_contents_match() {
    let disk = AtaDisk::secondary(AtaDrive::Master);
    // Fresh format so the test is independent of execution order.
    let mut fs = Fat32::format(disk, DISK_SECTORS, b"RUSTOS     ").expect("format failed");

    // Small file (single cluster).
    fs.create_file("HELLO.TXT").expect("create failed");
    let msg = b"Hello from the FAT32 integration test!";
    assert_eq!(fs.write_file("HELLO.TXT", msg).expect("write failed"), msg.len());

    // Multi-cluster file: 20 KiB spans 10 clusters, exercising chain allocation.
    fs.create_file("BIG.BIN").expect("create failed");
    let mut big = alloc::vec![0_u8; 20 * 1024];
    for (i, slot) in big.iter_mut().enumerate() {
        *slot = (i % 251) as u8;
    }
    assert_eq!(
        fs.write_file("BIG.BIN", &big).expect("write failed"),
        big.len()
    );

    // Read back and verify byte-for-byte.
    let mut buf = [0_u8; 128];
    let n = fs.read_file("HELLO.TXT", &mut buf).expect("read failed");
    assert_eq!(&buf[..n], msg);

    let mut big_buf = alloc::vec![0_u8; 20 * 1024];
    let n = fs.read_file("BIG.BIN", &mut big_buf).expect("read failed");
    assert_eq!(n, big.len());
    assert_eq!(big_buf, big);

    let names = fs.list_dir().expect("list failed");
    assert_eq!(names.len(), 2);

    // Delete and verify.
    fs.delete_file("HELLO.TXT").expect("delete failed");
    assert_eq!(fs.list_dir().expect("list failed").len(), 1);

    serial_println!("[fat32] 2 files written+verified (20 KiB multi-cluster OK), delete OK");
    println!("[fat32] files OK: 2 written, 20 KiB verified, delete OK");
}
