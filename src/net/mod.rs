//! Kernel networking layers: PCI -> Ethernet DMA -> smoltcp -> process sockets.
//! Only this facade crosses into scheduler/syscall ownership. Protocol service
//! code is hardware independent in crates/net-stack; application policy is Ring 3.
use crate::drivers::net::e1000;
pub use crate::userspace::usercopy;

use rustos_net_abi::{self as abi, Error, Info, Kind, Packet};
use rustos_net_stack::Network;
use spin::Mutex;
use x86_64::{structures::paging::OffsetPageTable, VirtAddr};

static NETWORK: Mutex<Option<Network<e1000::E1000>>> = Mutex::new(None);
/// Initialize optional Ethernet before scheduler ownership of the frame allocator.
pub fn init(
    mapper: &mut OffsetPageTable<'_>,
    frames: &mut crate::memory::RegionFrameAllocator,
    offset: VirtAddr,
) {
    match e1000::E1000::init(mapper, frames, offset) {
        Ok(device) => {
            let mac = device.mac();
            crate::serial_println!(
                "[net] e1000 {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}; 10.0.2.15/24 via 10.0.2.2",
                mac[0],
                mac[1],
                mac[2],
                mac[3],
                mac[4],
                mac[5]
            );
            *NETWORK.lock() = Some(Network::new(device, mac));
        }
        Err(error) => {
            crate::serial_println!("[net] Ethernet unavailable: {:?}", error);
        }
    }
}

/// Release dead-process endpoints. Lock order is scheduler -> network; the
/// network service never calls scheduler or accesses process page tables.
pub fn cleanup(pid: u32) {
    if let Some(network) = NETWORK.lock().as_mut() {
        network.cleanup(pid);
    }
}

fn millis() -> i64 {
    // PIT frequency is 1193182/65536 Hz. Integer conversion preserves monotonic
    // time and avoids floating point; clamp before converting to smoltcp's i64.
    ((u128::from(crate::interrupts::ticks()) * 65_536_000 / 1_193_182).min(i64::MAX as u128)) as i64
}

/// Execute a networking syscall with owned buffers. No user references survive
/// a copy, network lock acquisition, protocol callback or syscall return.
pub fn syscall(number: u64, first: u64, second: u64, third: u64) -> u64 {
    match dispatch(number, first, second, third) {
        Ok(value) => value,
        Err(error) => error.encode(),
    }
}

fn valid(ptr: u64, len: u64, write: bool) -> bool {
    usercopy::validate(ptr, len, write)
}

fn dispatch(number: u64, first: u64, second: u64, third: u64) -> Result<u64, Error> {
    // Obtain scheduler metadata before network lock, consistently with cleanup.
    let pid = crate::scheduler::current_pid().ok_or(Error::Invalid)?;
    if number == abi::SYS_INFO {
        if second != core::mem::size_of::<Info>() as u64
            || third != 0
            || !valid(first, second, true)
        {
            return Err(Error::Invalid);
        }
        let mut info = Info {
            version: abi::VERSION,
            ..Info::default()
        };
        if let Some(network) = NETWORK.lock().as_ref() {
            info.state = 1 | if network.device().link_up() { 2 } else { 0 };
            info.address = [10, 0, 2, 15];
            info.gateway = [10, 0, 2, 2];
            info.prefix = 24;
            info.mtu = 1500;
            info.mac = network.device().mac();
            info.open_sockets = network.count() as u32;
        }
        // SAFETY: Fixed padding-free fully initialized record; every output page
        // was checked writable/user. No scheduling/unmapping or user reference is
        // possible before this nonoverlapping byte copy on the single CPU.
        unsafe {
            core::ptr::copy_nonoverlapping(
                core::ptr::addr_of!(info).cast::<u8>(),
                first as *mut u8,
                core::mem::size_of::<Info>(),
            );
        }
        return Ok(0);
    }
    let mut packet = Packet::empty();
    if number == abi::SYS_SEND || number == abi::SYS_RECV {
        if third != core::mem::size_of::<Packet>() as u64
            || !valid(second, third, number == abi::SYS_RECV)
        {
            return Err(Error::Invalid);
        }
        if number == abi::SYS_SEND {
            // SAFETY: Complete readable user mapping was checked; Packet has
            // only byte/integer fields, no padding, references or invalid bit
            // patterns. Copy ends before protocol operations; no user borrow.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    second as *const u8,
                    core::ptr::addr_of_mut!(packet).cast::<u8>(),
                    core::mem::size_of::<Packet>(),
                );
            }
        }
    } else if third != 0 || (number != abi::SYS_CONNECT && second != 0) {
        return Err(Error::Invalid);
    }
    let mut guard = NETWORK.lock();
    let network = guard.as_mut().ok_or(Error::NoDevice)?;
    network.progress(millis());
    match number {
        abi::SYS_SOCKET => {
            if !network.device().link_up() {
                return Err(Error::NoDevice);
            }
            network.open(pid, Kind::from_raw(first)?)
        }
        abi::SYS_CONNECT => {
            if second >> 48 != 0 {
                return Err(Error::Invalid);
            }
            network.connect(
                pid,
                first,
                ((second >> 16) as u32).to_be_bytes(),
                second as u16,
            )?;
            Ok(0)
        }
        abi::SYS_SEND => {
            if !network.device().link_up() {
                return Err(Error::NoDevice);
            }
            Ok(network.send(pid, first, &packet)? as u64)
        }
        abi::SYS_RECV => {
            let received = network.recv(pid, first)?;
            let len = received.len;
            drop(guard);
            // SAFETY: Complete writable user output was checked before locking;
            // syscall cannot be preempted and no operation unmaps process memory.
            // Received contains only initialized padding-free metadata/payload.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    core::ptr::addr_of!(received).cast::<u8>(),
                    second as *mut u8,
                    core::mem::size_of::<Packet>(),
                );
            }
            Ok(u64::from(len))
        }
        abi::SYS_CLOSE => {
            network.close(pid, first)?;
            Ok(0)
        }
        abi::SYS_POLL => network.poll(pid, first),
        _ => Err(Error::Invalid),
    }
}
