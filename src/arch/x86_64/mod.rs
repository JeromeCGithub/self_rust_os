//! x86-64 CPU discovery, startup, interrupt entry and local execution state.
pub mod acpi;
pub mod apic;
/// Canonical interrupt register frame.
pub mod context;
pub mod entry;
pub mod gdt;
/// CPU interrupt tables and IRQ dispatch.
pub mod interrupts;
pub mod smp;
