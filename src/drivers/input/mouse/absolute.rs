//! VMware-compatible absolute pointer protocol exposed by QEMU's vmmouse.
//!
//! Absolute events avoid depending on host desktop pointer-lock support.
//! Protocol reference: <https://github.com/torvalds/linux/blob/master/drivers/input/mouse/vmmouse.c>.
use core::sync::atomic::{AtomicBool, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(false);
const MAGIC: u32 = 0x564d_5868;
const DATA: u32 = 39;
const STATUS: u32 = 40;
const COMMAND: u32 = 41;

// The port returns EAX, EBX, ECX and EDX. RBX is reserved by LLVM, so exchange
// it with RSI around the instruction, restoring the caller's RBX afterward.
fn request(command: u32, argument: u32) -> (u32, u32, u32, u32) {
    let (mut a, mut b, mut c, mut d) = (MAGIC, argument, command, 0x5658u32);
    // SAFETY: Called in ring 0. This is the hypervisor's low-bandwidth I/O
    // interface; all clobbered registers are declared and RBX is preserved.
    unsafe {
        core::arch::asm!(
            "xchg rbx, rsi",
            "in eax, dx",
            "xchg rbx, rsi",
            inout("eax") a,
            inout("esi") b,
            inout("ecx") c,
            inout("edx") d,
            options(nostack),
        );
    }
    (a, b, c, d)
}

pub(super) fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

pub(super) fn init() {
    let (version, response, _, _) = request(10, !MAGIC);
    if version == u32::MAX || response != MAGIC {
        return;
    }
    request(COMMAND, 0x4541_4552); // Enable; queues the protocol version ID.
    if request(STATUS, 0).0 != 1 || request(DATA, 1).0 != 0x3442_554a {
        request(COMMAND, 0xf5);
        return;
    }
    request(COMMAND, 0x5342_4152); // Request absolute coordinates.
    ENABLED.store(true, Ordering::Relaxed);
    crate::serial_println!("[mouse] absolute pointer enabled (QEMU vmmouse)");
}

pub(super) fn poll() {
    if !enabled() {
        return;
    }
    // Bound work per syscall even if the host sends a continuous event stream.
    for _ in 0..64 {
        let status = request(STATUS, 0).0;
        let queued = status & 0xffff;
        if status & 0xffff_0000 != 0 || queued % 4 != 0 {
            request(COMMAND, 0xf5);
            ENABLED.store(false, Ordering::Relaxed);
            return;
        }
        if queued == 0 {
            return;
        }
        let (flags, x, y, _) = request(DATA, 4);
        let mut dev = super::MOUSE.lock();
        if flags & 0x10000 == 0 {
            dev.state.x =
                (u64::from(x.min(65535)) * (super::screen_width() - 1) as u64 / 65535) as i32;
            dev.state.y =
                (u64::from(y.min(65535)) * (super::screen_height() - 1) as u64 / 65535) as i32;
        }
        dev.state.buttons = u8::from(flags & 0x20 != 0)
            | (u8::from(flags & 0x10 != 0) << 1)
            | (u8::from(flags & 0x08 != 0) << 2);
    }
}
