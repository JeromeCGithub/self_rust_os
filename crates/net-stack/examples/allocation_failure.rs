//! Executable regression: allocation failure at socket-open is an error, not OOM.
use rustos_net_abi::{Error, Kind};
use rustos_net_stack::Network;
use smoltcp::phy::{Loopback, Medium};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, Ordering};

struct FailAllocator;
static FAIL: AtomicBool = AtomicBool::new(false);
// SAFETY: Successful allocations/deallocations are delegated unchanged to System;
// returning null is GlobalAlloc's documented failure contract. The test toggles
// failure only around a single-threaded operation and does not alter live blocks.
unsafe impl GlobalAlloc for FailAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if FAIL.load(Ordering::Relaxed) {
            core::ptr::null_mut()
        } else {
            // SAFETY: The caller supplies a valid Layout; forwarded unchanged.
            unsafe { System.alloc(layout) }
        }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: All non-null blocks originated from System with this Layout.
        unsafe {
            System.dealloc(pointer, layout);
        }
    }
}
#[global_allocator]
static ALLOCATOR: FailAllocator = FailAllocator;

fn main() {
    let mut net = Network::new(Loopback::new(Medium::Ethernet), [2, 1, 2, 3, 4, 5]);
    for kind in [Kind::Icmp, Kind::Udp, Kind::Tcp] {
        FAIL.store(true, Ordering::Relaxed);
        let result = net.open(1, kind);
        FAIL.store(false, Ordering::Relaxed);
        assert_eq!(result, Err(Error::NoResources));
        assert_eq!(net.count(), 0);
        let handle = net
            .open(1, kind)
            .expect("socket opens after failed allocation");
        net.close(1, handle).unwrap();
    }
    println!("PASS: allocation failures return NoResources and recover for ICMP/UDP/TCP");
}
