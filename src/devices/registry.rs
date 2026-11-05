//! Bounded boot-time inventory. Records contain hardware identity, never live pointers.
use core::fmt::{self, Write};

pub const CAPACITY: usize = 64;
/// Discovery/ownership branch of the device inventory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bus {
    Acpi,
    Boot,
    Legacy,
    Pci,
}
/// Public hardware properties that are safe to report to applications.
#[derive(Clone, Copy, Debug)]
pub enum Details {
    None,
    Cpu { apic_id: u8, bsp: bool },
    Framebuffer { width: usize, height: usize },
    Block { partition_lba: u32, sectors: u32 },
    Ethernet { mac: [u8; 6] },
}
/// Stable identity of an initialized driver instance.
#[derive(Clone, Copy, Debug)]
pub struct Device {
    pub name: &'static str,
    pub instance: u8,
    pub bus: Bus,
    pub driver: &'static str,
    pub details: Details,
}
/// Registration is boot-only, bounded, and rejects ambiguous identities.
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Duplicate,
    Full,
}

pub struct Registry {
    records: [Option<Device>; CAPACITY],
    len: usize,
}
impl Registry {
    pub const fn new() -> Self {
        Self {
            records: [None; CAPACITY],
            len: 0,
        }
    }
    pub fn register(&mut self, device: Device) -> Result<(), Error> {
        if self.records[..self.len]
            .iter()
            .flatten()
            .any(|d| d.name == device.name && d.instance == device.instance)
        {
            return Err(Error::Duplicate);
        }
        if self.len == CAPACITY {
            return Err(Error::Full);
        }
        self.records[self.len] = Some(device);
        self.len += 1;
        Ok(())
    }
    pub fn render(&self, out: &mut impl Write) -> fmt::Result {
        for (bus, name) in [
            (Bus::Acpi, "acpi"),
            (Bus::Boot, "boot"),
            (Bus::Legacy, "legacy"),
            (Bus::Pci, "pci"),
        ] {
            writeln!(out, "{}/", name)?;
            for device in self.records[..self.len]
                .iter()
                .flatten()
                .filter(|d| d.bus == bus)
            {
                write!(
                    out,
                    "  {}{} driver={}",
                    device.name, device.instance, device.driver
                )?;
                match device.details {
                    Details::None => {}
                    Details::Cpu { apic_id, bsp } => write!(
                        out,
                        " apic={} role={}",
                        apic_id,
                        if bsp { "bsp" } else { "ap" }
                    )?,
                    Details::Framebuffer { width, height } => {
                        write!(out, " pixels={}x{}", width, height)?
                    }
                    Details::Block {
                        partition_lba,
                        sectors,
                    } => write!(out, " partition_lba={} sectors={}", partition_lba, sectors)?,
                    Details::Ethernet { mac } => write!(
                        out,
                        " mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
                    )?,
                }
                out.write_char('\n')?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    use super::*;
    use alloc::string::String;
    fn cpu(instance: u8) -> Device {
        Device {
            name: "cpu",
            instance,
            bus: Bus::Acpi,
            driver: "x86-64",
            details: Details::Cpu {
                apic_id: instance * 2,
                bsp: instance == 0,
            },
        }
    }
    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn registers_unique_devices_rejects_duplicates_and_preserves_records_when_full() {
        let mut registry = Registry::new();
        registry.register(cpu(0)).unwrap();
        assert_eq!(registry.register(cpu(0)), Err(Error::Duplicate));
        for i in 1..CAPACITY {
            registry.register(cpu(i as u8)).unwrap();
        }
        assert_eq!(registry.register(cpu(64)), Err(Error::Full));
        let mut out = String::new();
        registry.render(&mut out).unwrap();
        assert_eq!(
            out.lines()
                .filter(|line| line.contains("driver=x86-64"))
                .count(),
            CAPACITY
        );
        assert!(out.contains("cpu0 driver=x86-64 apic=0 role=bsp"));
        assert!(out.contains("cpu63 driver=x86-64 apic=126 role=ap"));
    }
    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn groups_discovery_sources_without_claiming_unregistered_hardware() {
        let mut registry = Registry::new();
        let mut out = String::new();
        registry.render(&mut out).unwrap();
        assert!(!out.contains("eth0"));
        registry.register(cpu(0)).unwrap();
        registry
            .register(Device {
                name: "eth",
                instance: 0,
                bus: Bus::Pci,
                driver: "e1000",
                details: Details::Ethernet {
                    mac: [2, 0, 0, 0, 0, 1],
                },
            })
            .unwrap();
        registry.render(&mut out).unwrap();
        assert!(out.contains("acpi/\n  cpu0"));
        assert!(out.contains("pci/\n  eth0 driver=e1000 mac=02:00:00:00:00:01"));
        assert!(!out.contains("disk0"));
    }
}
