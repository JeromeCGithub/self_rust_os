//! Ring geometry and Ethernet capabilities, separated from MMIO for wire tests.
use smoltcp::phy::{DeviceCapabilities, Medium};
pub(super) const COUNT: usize = 32;
pub(super) const FRAME_BYTES: usize = 1514;
pub(super) fn capabilities() -> DeviceCapabilities {
    let mut caps = DeviceCapabilities::default();
    caps.medium = Medium::Ethernet;
    caps.max_transmission_unit = FRAME_BYTES;
    // A legacy ring leaves one descriptor between hardware head and tail.
    // smoltcp also uses this limit to cap the peer's advertised TCP window.
    caps.max_burst_size = Some(COUNT - 1);
    caps
}
