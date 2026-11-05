//! Kernel device inventory, separate from the scheduler and filesystem namespace.
//! Only successful boot initialization registers hardware. No IRQ registers or
//! logs devices; readers hold this lock only while copying bounded scalar records.
mod registry;
pub(crate) use registry::{Bus, Details, Device};
static REGISTRY: spin::Mutex<registry::Registry> = spin::Mutex::new(registry::Registry::new());

pub(crate) fn register(device: Device) {
    // Duplicate IDs or exhaustion with the fixed supported boot configuration
    // violate a kernel invariant; no user operation can allocate registry slots.
    REGISTRY
        .lock()
        .register(device)
        .expect("boot device inventory capacity/identity");
}

pub(crate) fn render(out: &mut impl core::fmt::Write) -> core::fmt::Result {
    out.write_str("software/\n  null driver=null\n  zero driver=zero\n  random driver=rdrand\n")?;
    // No allocation, driver operation, syscall or scheduling while held. Driver
    // initialization releases its own resource locks before publishing a record.
    REGISTRY.lock().render(out)
}
