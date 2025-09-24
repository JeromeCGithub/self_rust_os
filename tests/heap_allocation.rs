//! Tests for heap allocation in a `no_std` environment using a custom allocator.
#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(self_rust_os::test_runner)]
#![reexport_test_harness_main = "test_main"]
#![allow(missing_docs)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::missing_assert_message)]
extern crate alloc;

use alloc::boxed::Box;
use alloc::vec::Vec;
use self_rust_os::allocator::HEAP_SIZE;

use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;
use self_rust_os::{
    allocator,
    memory::{self, BootInfoFrameAllocator},
};
use x86_64::VirtAddr;

entry_point!(main);

#[expect(clippy::undocumented_unsafe_blocks)]
#[expect(clippy::expect_used)]
fn main(boot_info: &'static BootInfo) -> ! {
    self_rust_os::init();
    let phys_mem_offset = VirtAddr::new(boot_info.physical_memory_offset);
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

#[test_case]
fn when_creating_simple_allocation_then_memory_is_allocated() {
    let heap_value_1 = Box::new(41);
    let heap_value_2 = Box::new(42);

    assert_eq!(*heap_value_1, 41);
    assert_eq!(*heap_value_2, 42);
}

#[test_case]
fn when_creating_large_vec_then_memory_is_allocated() {
    let mut vec = Vec::new();
    for i in 0..1000 {
        vec.push(i);
    }
    assert_eq!(vec.iter().sum::<u64>(), (0..1000).sum());
}

#[test_case]
fn when_many_boxes_are_allocated_then_memory_is_allocated() {
    for i in 0..HEAP_SIZE {
        let x = Box::new(i);
        assert_eq!(*x, i);
    }
}
