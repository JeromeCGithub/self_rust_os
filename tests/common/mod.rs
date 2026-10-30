//! Shared framebuffer backing for integration tests.
//!
//! The legacy bootloader supplies an 8bpp VGA mode, while the graphics driver
//! expects 32bpp. Use mapped RAM so tests can safely exercise the driver.

use self_rust_os::drivers::video::framebuffer::FbInfo;
use x86_64::VirtAddr;

pub const FRAMEBUFFER_INFO: FbInfo = FbInfo {
    addr: 0,
    width: 320,
    height: 200,
    pitch: 320 * 4,
    bpp: 32,
};

static mut FRAMEBUFFER: [u32; 320 * 200] = [0; 320 * 200];

pub fn framebuffer_addr() -> VirtAddr {
    VirtAddr::from_ptr(core::ptr::addr_of_mut!(FRAMEBUFFER))
}
