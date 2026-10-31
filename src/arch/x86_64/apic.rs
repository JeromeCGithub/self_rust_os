//! xAPIC register access. Device IRQs retain BSP PIC virtual-wire delivery.
use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::registers::model_specific::Msr;

/// Dedicated supervisor, uncached local APIC mapping.
pub const VIRTUAL_BASE: u64 = 0xffff_b000_0000_0000;
/// Local scheduling interrupt vector.
pub const TIMER_VECTOR: u8 = 0x40;
/// Reschedule interrupt vector.
pub const RESCHEDULE_VECTOR: u8 = 0x41;
static BASE: AtomicU64 = AtomicU64::new(0);

/// Publish the mapped register page before starting APs.
pub fn publish() {
    BASE.store(VIRTUAL_BASE, Ordering::Release);
}
fn write(offset: u64, value: u32) {
    let base = BASE.load(Ordering::Acquire);
    assert!(base != 0, "local APIC is not mapped");
    // SAFETY: Boot publishes a supervisor-only uncached 4 KiB APIC mapping;
    // offsets are fixed register offsets. Each CPU addresses its own LAPIC.
    unsafe {
        ((base + offset) as *mut u32).write_volatile(value);
    }
}
fn read(offset: u64) -> u32 {
    // SAFETY: Same mapping and fixed-offset contract as write.
    unsafe { ((BASE.load(Ordering::Acquire) + offset) as *const u32).read_volatile() }
}
/// Enable the local APIC; BSP receives external IRQs through LINT0 virtual wire.
pub fn init(bsp: bool) {
    // SAFETY: IA32_APIC_BASE is present on the checked xAPIC platform. Preserve
    // its physical base/BSP bit; x2APIC mode is explicitly unsupported.
    unsafe {
        let mut msr = Msr::new(0x1b);
        let value = msr.read();
        assert!(value & (1 << 10) == 0, "x2APIC mode unsupported");
        msr.write(value | (1 << 11));
    }
    write(0xf0, 0x100 | 0xff);
    write(0x80, 0);
    write(0x320, 1 << 16);
    write(0x350, if bsp { 7 << 8 } else { 1 << 16 });
    write(0x360, 4 << 8);
    write(0x370, 1 << 16);
    eoi();
}
/// Acknowledge one local interrupt.
pub fn eoi() {
    write(0xb0, 0);
}
/// Whether local APIC initialization has begun.
pub fn available() -> bool {
    BASE.load(Ordering::Acquire) != 0
}
/// Send a fixed, INIT, STARTUP or NMI IPI with bounded delivery polling.
pub fn send(id: u8, command: u32) {
    for _ in 0..1_000_000 {
        if read(0x300) & (1 << 12) == 0 {
            write(0x310, u32::from(id) << 24);
            write(0x300, command);
            return;
        }
        core::hint::spin_loop();
    }
    panic!("APIC delivery timeout");
}
/// Start a calibrated periodic local scheduling timer.
pub fn timer(initial: u32) {
    write(0x3e0, 3); // divide by 16
    write(0x320, (1 << 17) | u32::from(TIMER_VECTOR));
    write(0x380, initial.max(1));
}
/// Mask an idle AP's scheduling ticks; the countdown remains periodic so
/// unmasking on user dispatch restores preemption without recalibration.
pub fn idle_timer(idle: bool) {
    write(
        0x320,
        (1 << 17) | u32::from(TIMER_VECTOR) | if idle { 1 << 16 } else { 0 },
    );
}
/// Start a masked calibration countdown at divide-by-16.
pub fn calibration_start() {
    write(0x3e0, 3);
    write(0x320, 1 << 16);
    write(0x380, u32::MAX);
}
/// Read the calibration countdown.
pub fn remaining() -> u32 {
    read(0x390)
}
