//! Bootstrap processor discovery, AP startup and lock-independent TLB shootdown.
#[cfg(feature = "grub")]
use super::acpi;
use super::{acpi::MAX_CPUS, apic};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

static IDS: [AtomicU32; MAX_CPUS] = [const { AtomicU32::new(u32::MAX) }; MAX_CPUS];
static ONLINE: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];
static COUNT: AtomicUsize = AtomicUsize::new(1);
static RELEASED: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "grub")]
static TIMER: AtomicU32 = AtomicU32::new(0);
static BOOT_CR3: AtomicU64 = AtomicU64::new(0);
static GENERATION: AtomicU64 = AtomicU64::new(0);
static ACK: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static IDLE_STACKS: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// Hardware xAPIC identifier. The supported topology has IDs below 256.
pub fn apic_id() -> u8 {
    // SAFETY: CPUID is supported by the x86-64 baseline; leaf 1 is available.
    unsafe { (core::arch::x86_64::__cpuid(1).ebx >> 24) as u8 }
}
/// Dense logical CPU index, independent of sparse firmware APIC IDs.
pub fn current_cpu() -> usize {
    let id = u32::from(apic_id());
    (0..COUNT.load(Ordering::Acquire))
        .find(|&i| IDS[i].load(Ordering::Relaxed) == id)
        .unwrap_or(0)
}
/// Number of online logical CPUs; APs publish readiness before release.
pub fn online_count() -> usize {
    if !RELEASED.load(Ordering::Acquire) {
        return 1;
    }
    COUNT.load(Ordering::Acquire)
}
/// Restore the boot address space before freeing retired process page tables.
pub fn boot_cr3() -> u64 {
    BOOT_CR3.load(Ordering::Acquire)
}
/// CPU-owned idle stack, retained for the lifetime of this CPU.
pub fn idle_stack() -> u64 {
    IDLE_STACKS[current_cpu()].load(Ordering::Acquire)
}

#[cfg(feature = "grub")]
fn physical(address: u64, len: usize) -> Option<&'static [u8]> {
    // GRUB's BSP identity map covers the first GiB. Tables are immutable firmware
    // bytes and read only during discovery, before application mappings exist.
    if address < 0x400 || len > 1024 * 1024 || address.checked_add(len as u64)? > 0x4000_0000 {
        return None;
    }
    // SAFETY: Bounds above prove the complete boot identity mapping, byte
    // alignment suffices, and this function is used only during BSP discovery.
    Some(unsafe { core::slice::from_raw_parts(address as *const u8, len) })
}
#[cfg(feature = "grub")]
fn table(address: u64) -> Option<&'static [u8]> {
    let head = physical(address, 36)?;
    let len = u32::from_le_bytes(head[4..8].try_into().ok()?) as usize;
    if len < 36 {
        return None;
    }
    let bytes = physical(address, len)?;
    (bytes.iter().fold(0u8, |a, b| a.wrapping_add(*b)) == 0).then_some(bytes)
}
#[cfg(feature = "grub")]
fn discover() -> Option<acpi::Topology> {
    // SAFETY: BIOS data area is present in the boot identity map; use an
    // unaligned integer read and create no reference to firmware fields.
    let ebda = u64::from(unsafe { (0x40e as *const u16).read_unaligned() }) << 4;
    for (start, end) in [(ebda, ebda + 1024), (0xe0000, 0x100000)] {
        for address in (start..end).step_by(16) {
            let Some(rsdp) = physical(address, 20) else {
                continue;
            };
            if &rsdp[..8] != b"RSD PTR " || rsdp.iter().fold(0u8, |a, b| a.wrapping_add(*b)) != 0 {
                continue;
            }
            let mut stride = 4;
            let mut root = u64::from(u32::from_le_bytes(rsdp[16..20].try_into().ok()?));
            if rsdp[15] >= 2 {
                let extended = physical(address, 36)?;
                let length = u32::from_le_bytes(extended[20..24].try_into().ok()?) as usize;
                if length < 36 || length > 4096 {
                    return None;
                }
                if physical(address, length)?
                    .iter()
                    .fold(0u8, |a, b| a.wrapping_add(*b))
                    != 0
                {
                    return None;
                }
                let xsdt = u64::from_le_bytes(extended[24..32].try_into().ok()?);
                if xsdt != 0 {
                    root = xsdt;
                    stride = 8;
                }
            }
            let root = table(root)?;
            if &root[..4] != if stride == 8 { b"XSDT" } else { b"RSDT" }
                || (root.len() - 36) % stride != 0
            {
                return None;
            }
            for pointer in root[36..].chunks_exact(stride) {
                let address = if stride == 8 {
                    u64::from_le_bytes(pointer.try_into().ok()?)
                } else {
                    u64::from(u32::from_le_bytes(pointer.try_into().ok()?))
                };
                let Some(bytes) = table(address) else {
                    continue;
                };
                if &bytes[..4] == b"APIC" {
                    return Some(
                        acpi::parse_madt(bytes).expect("invalid or unsupported MADT topology"),
                    );
                }
            }
        }
    }
    None
}

/// Discover firmware and map LAPIC registers before initializing any CPU tables.
#[cfg(feature = "grub")]
pub fn prepare(
    mapper: &mut x86_64::structures::paging::OffsetPageTable<'_>,
    frames: &mut crate::memory::RegionFrameAllocator,
    regions: &[crate::memory::MemRegion],
) {
    use x86_64::{
        structures::paging::{Mapper, Page, PageTableFlags, PhysFrame},
        PhysAddr, VirtAddr,
    };
    let mut topology = discover().expect("ACPI MADT required for GRUB CPU discovery");
    let bsp = apic_id();
    let pos = topology.ids[..topology.count]
        .iter()
        .position(|&id| id == bsp)
        .expect("BSP absent from MADT");
    topology.ids.swap(0, pos);
    assert!(
        regions.iter().any(|r| r.start <= 0x8000 && r.end >= 0x9000),
        "AP trampoline must be usable RAM"
    );
    // SAFETY: The dedicated virtual page is unused, LAPIC is MMIO not an
    // allocated RAM frame, and cache flags avoid a write-back MMIO alias.
    unsafe {
        mapper
            .map_to(
                Page::<x86_64::structures::paging::Size4KiB>::containing_address(VirtAddr::new(
                    apic::VIRTUAL_BASE,
                )),
                PhysFrame::containing_address(PhysAddr::new(topology.local_apic)),
                PageTableFlags::PRESENT
                    | PageTableFlags::WRITABLE
                    | PageTableFlags::NO_CACHE
                    | PageTableFlags::WRITE_THROUGH,
                frames,
            )
            .expect("map local APIC")
            .flush();
    }
    for i in 0..topology.count {
        IDS[i].store(u32::from(topology.ids[i]), Ordering::Relaxed);
    }
    BOOT_CR3.store(
        x86_64::registers::control::Cr3::read()
            .0
            .start_address()
            .as_u64(),
        Ordering::Release,
    );
    COUNT.store(topology.count, Ordering::Release);
    apic::publish();
}

/// Bounded PIT-channel-2 delay. Channel 0's public tick units are unchanged.
#[cfg(feature = "grub")]
fn delay(microseconds: u32) {
    use x86_64::instructions::port::Port;
    let count = (u64::from(microseconds) * 1_193_182 / 1_000_000).clamp(1, 65535) as u16;
    // SAFETY: BSP-only startup with IRQs off; channel 2 and speaker gate are
    // restored after a bounded poll, without changing the periodic PIT channel.
    unsafe {
        let mut gate = Port::<u8>::new(0x61);
        let old = gate.read();
        gate.write(old & !3);
        Port::<u8>::new(0x43).write(0xb0);
        let mut data = Port::<u8>::new(0x42);
        data.write(count as u8);
        data.write((count >> 8) as u8);
        gate.write((old & !2) | 1);
        for _ in 0..5_000_000 {
            if gate.read() & 0x20 != 0 {
                gate.write(old);
                return;
            }
            core::hint::spin_loop();
        }
        gate.write(old);
    }
    panic!("PIT calibration timeout");
}

/// Allocate stable idle stacks, start APs and release scheduling after readiness.
pub fn start() {
    let count = COUNT.load(Ordering::Acquire);
    for i in 0..count {
        let stack = alloc::vec![0u128;4096].into_boxed_slice();
        let top = stack.as_ptr() as u64 + 65536;
        IDLE_STACKS[i].store(top, Ordering::Release);
        let _ = alloc::boxed::Box::leak(stack); // CPU-lifetime ownership, bounded at 32.
    }
    ONLINE[0].store(true, Ordering::Release);
    #[cfg(feature = "grub")]
    {
        apic::init(true);
        apic::calibration_start();
        delay(10_000);
        let timer = u32::MAX - apic::remaining();
        TIMER.store(timer.max(1), Ordering::Release);
        unsafe extern "C" {
            static ap_trampoline_start: u8;
            static ap_trampoline_end: u8;
        }
        let begin = core::ptr::addr_of!(ap_trampoline_start);
        let len = core::ptr::addr_of!(ap_trampoline_end) as usize - begin as usize;
        assert!(len < 0xf00);
        // SAFETY: prepare checked usable conventional RAM; allocator starts
        // above the kernel image so this page is permanently reserved. Only
        // the BSP writes it; AP startup is sequential and readiness is acquired.
        unsafe {
            core::ptr::copy_nonoverlapping(begin, 0x8000 as *mut u8, len);
        }
        for i in 1..count {
            assert!(
                BOOT_CR3.load(Ordering::Acquire) <= u64::from(u32::MAX),
                "AP transition root must fit 32-bit CR3"
            );
            // SAFETY: Same reserved page, aligned fixed-width startup parameters.
            // Each AP consumes them before publishing ONLINE; timed-out boot
            // panics and never reuses the page or AP stacks.
            unsafe {
                (0x8f00 as *mut u32).write(BOOT_CR3.load(Ordering::Acquire) as u32);
                (0x8f08 as *mut u64).write(IDLE_STACKS[i].load(Ordering::Acquire));
                (0x8f10 as *mut u64).write(ap_main as *const () as u64);
            }
            core::sync::atomic::fence(Ordering::SeqCst);
            let id = IDS[i].load(Ordering::Relaxed) as u8;
            apic::send(id, 0xc500);
            delay(10_000);
            apic::send(id, 0x8500);
            delay(200);
            apic::send(id, 0x608);
            delay(200);
            if !ONLINE[i].load(Ordering::Acquire) {
                apic::send(id, 0x608);
            }
            let mut ready = false;
            for _ in 0..20_000_000 {
                if ONLINE[i].load(Ordering::Acquire) {
                    ready = true;
                    break;
                }
                core::hint::spin_loop();
            }
            assert!(ready, "AP startup timed out; resources retained");
        }
        apic::timer(timer);
    }
    RELEASED.store(true, Ordering::Release);
    crate::serial_println!("[smp] online={}", count);
}

#[cfg(feature = "grub")]
extern "C" fn ap_main() -> ! {
    crate::gdt::init();
    crate::interrupts::init_idt();
    apic::init(false);
    ONLINE[current_cpu()].store(true, Ordering::Release);
    while !RELEASED.load(Ordering::Acquire) {
        core::hint::spin_loop();
    }
    apic::timer(TIMER.load(Ordering::Acquire));
    super::entry::idle()
}

/// Wake CPUs after publishing ready work. Fixed IPIs only request rescheduling.
pub fn wake_idle() {
    if !RELEASED.load(Ordering::Acquire) || !apic::available() {
        return;
    }
    let here = current_cpu();
    let mask = crate::scheduler::idle_targets();
    for i in 0..COUNT.load(Ordering::Acquire) {
        if i != here && mask & (1u32 << i) != 0 {
            apic::send(
                IDS[i].load(Ordering::Relaxed) as u8,
                u32::from(apic::RESCHEDULE_VECTOR),
            );
        }
    }
}
/// NMI handler: no locks or allocation, including when another CPU holds entry.
pub fn invalidate_nmi() {
    let generation = GENERATION.load(Ordering::Acquire);
    let (frame, flags) = x86_64::registers::control::Cr3::read();
    // SAFETY: Reload the currently active valid P4; PCID/global mappings are
    // disabled by this kernel, so this flushes its complete translation cache.
    unsafe {
        x86_64::registers::control::Cr3::write(frame, flags);
    }
    ACK[current_cpu()].store(generation, Ordering::Release);
}
/// Flush all CPUs before physical frame reuse. Caller holds kernel entry gate.
pub fn shootdown() {
    let generation = GENERATION.fetch_add(1, Ordering::AcqRel) + 1;
    invalidate_nmi();
    if !RELEASED.load(Ordering::Acquire) || !apic::available() {
        return;
    }
    let here = current_cpu();
    for i in 0..COUNT.load(Ordering::Acquire) {
        if i != here {
            apic::send(IDS[i].load(Ordering::Relaxed) as u8, 4 << 8);
        }
    }
    for i in 0..COUNT.load(Ordering::Acquire) {
        let mut done = false;
        for _ in 0..20_000_000 {
            if ACK[i].load(Ordering::Acquire) >= generation {
                done = true;
                break;
            }
            core::hint::spin_loop();
        }
        assert!(done, "TLB shootdown timed out; frames not recycled");
    }
}

#[cfg(feature = "grub")]
core::arch::global_asm!(
    r#"
.section .rodata.ap_trampoline,"a"
.code16
.global ap_trampoline_start
ap_trampoline_start:
    cli
    cld
    xorw %ax,%ax
    movw %ax,%ds
    movw %ax,%es
    movw %ax,%ss
    lgdt 0x8000+(ap_gdt_ptr-ap_trampoline_start)
    movl %cr0,%eax
    orl $1,%eax
    movl %eax,%cr0
    ljmpl $8,$0x8000+(ap_protected-ap_trampoline_start)
.code32
ap_protected:
    movw $16,%ax
    movw %ax,%ds
    movw %ax,%es
    movw %ax,%ss
    movl %cr4,%eax
    orl $32,%eax
    movl %eax,%cr4
    movl 0x8f00,%eax
    movl %eax,%cr3
    movl $0xc0000080,%ecx
    rdmsr
    orl $256,%eax
    wrmsr
    # INIT retains reset CR0.CD/NW. Flush firmware cache contents before
    # enabling normal cached execution and paging on this still-private AP.
    wbinvd
    movl %cr0,%eax
    andl $0x9fffffff,%eax
    orl $0x80000000,%eax
    movl %eax,%cr0
    ljmpl $24,$0x8000+(ap_long-ap_trampoline_start)
.code64
ap_long:
    movq 0x8f08,%rsp
    xorq %rbp,%rbp
    call *0x8f10
1:  cli
    hlt
    jmp 1b
.balign 8
ap_gdt:
    .quad 0
    .quad 0x00cf9a000000ffff
    .quad 0x00cf92000000ffff
    .quad 0x00209a0000000000
ap_gdt_ptr:
    .word 31
    .long 0x8000+(ap_gdt-ap_trampoline_start)
.global ap_trampoline_end
ap_trampoline_end:
.code64
"#,
    options(att_syntax)
);

/// Request prompt preemption after a process is stopped on another CPU.
pub fn reschedule_all() {
    if !RELEASED.load(Ordering::Acquire) || !apic::available() {
        return;
    }
    let here = current_cpu();
    for i in 0..COUNT.load(Ordering::Acquire) {
        if i != here {
            apic::send(
                IDS[i].load(Ordering::Relaxed) as u8,
                u32::from(apic::RESCHEDULE_VECTOR),
            );
        }
    }
}

/// Preserve the original supervisor root for idle/reclamation on both boot paths.
pub fn retain_boot_root() {
    let root = x86_64::registers::control::Cr3::read()
        .0
        .start_address()
        .as_u64();
    let _ = BOOT_CR3.compare_exchange(0, root, Ordering::Release, Ordering::Relaxed);
}

/// Wake gate waiters without accessing scheduler locks or user memory.
pub fn wake_cpus(mask: u32) {
    if !RELEASED.load(Ordering::Acquire) || !apic::available() {
        return;
    }
    let here = current_cpu();
    for i in 0..COUNT.load(Ordering::Acquire) {
        if i != here && mask & (1u32 << i) != 0 {
            apic::send(
                IDS[i].load(Ordering::Relaxed) as u8,
                u32::from(apic::RESCHEDULE_VECTOR),
            );
        }
    }
}
