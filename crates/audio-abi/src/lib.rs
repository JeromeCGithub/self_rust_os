//! Native exclusive PCM stream ABI. Sample mixing/decoding is userspace policy.
#![no_std]
/// Open the fixed stereo signed16-bit little-endian48kHz device; all args zero.
pub const SYS_OPEN: u64 = 82;
/// Write one whole number of PCM frames, at most4096 bytes: handle, pointer, bytes.
pub const SYS_WRITE: u64 = 83;
/// Close/pause/resume an owned stream: handle, operation, zero.
pub const SYS_CONTROL: u64 = 84;
/// Copy one complete Status to user memory: handle, pointer, record size.
pub const SYS_STATUS: u64 = 85;
/// PCM frames per second.
pub const RATE: u32 = 48000;
/// Interleaved channel count.
pub const CHANNELS: u32 = 2;
/// Bytes in one stereo PCM frame.
pub const FRAME_BYTES: usize = 4;
/// Maximum bytes accepted by one nonblocking write.
pub const MAX_WRITE: usize = 4096;
/// Status record ABI revision.
pub const VERSION: u32 = 1;
/// Close, pause, resume operation codes.
pub const CLOSE: u64 = 0;
/// Pause without discarding queued PCM.
pub const PAUSE: u64 = 1;
/// Resume queued PCM.
pub const RESUME: u64 = 2;
/// Padding-free counters. Positions/counts are PCM frames, not bytes or samples.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Status {
    /// ABI revision.
    pub version: u32,
    /// Bit0 running DMA, bit1 paused. Remaining bits zero.
    pub flags: u32,
    /// Total PCM frames accepted by this stream.
    pub submitted_frames: u64,
    /// PCM frames consumed by DMA, including a partial current descriptor.
    pub played_frames: u64,
    /// Accepted frames remaining, including a partial current descriptor.
    pub queued_frames: u32,
    /// Free descriptors (each accepts up to1024 PCM frames).
    pub free_slots: u32,
    /// Hardware-reported FIFO errors; end-of-queue is not counted as underrun.
    pub underruns: u64,
}
/// Typed native audio failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum Error {
    /// Malformed arguments, incomplete frames or invalid user mapping.
    Invalid = 1,
    /// No initialized audio controller.
    NoDevice = 2,
    /// Another process owns the exclusive device.
    Busy = 3,
    /// All bounded descriptors are queued.
    WouldBlock = 4,
    /// Hardware or counter state failed; device may be quarantined.
    Failed = 5,
    /// Stale handle or caller does not own this stream.
    NotOwner = 6,
}
const BASE: u64 = u64::MAX - 1023;
impl Error {
    /// Encode below platform/network/control error ranges.
    pub const fn encode(self) -> u64 {
        BASE + self as u64
    }
}
/// Decode scalar/handle or error; unknown reserved values fail closed.
pub fn decode(value: u64) -> Result<u64, Error> {
    if value < BASE {
        return Ok(value);
    }
    Err(match value - BASE {
        1 => Error::Invalid,
        2 => Error::NoDevice,
        3 => Error::Busy,
        4 => Error::WouldBlock,
        6 => Error::NotOwner,
        _ => Error::Failed,
    })
}
