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
impl Device {
    /// Directory identity in the existing short-name ABI.
    pub fn file_name(&self) -> Option<[u8; 11]> {
        let mut digits = [0u8; 3];
        let count = if self.instance >= 100 {
            3
        } else if self.instance >= 10 {
            2
        } else {
            1
        };
        let mut value = self.instance;
        for index in (0..count).rev() {
            digits[index] = b'0' + value % 10;
            value /= 10;
        }
        if self.name.is_empty()
            || self.name.len() + count > 8
            || !self
                .name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_')
        {
            return None;
        }
        let mut result = [b' '; 11];
        for (index, byte) in self.name.bytes().enumerate() {
            result[index] = byte.to_ascii_uppercase();
        }
        result[self.name.len()..self.name.len() + count].copy_from_slice(&digits[..count]);
        Some(result)
    }
    /// One immutable public driver/CPU description, shared by directory aliases.
    pub fn write_info(&self, out: &mut impl Write) -> fmt::Result {
        write!(out, "{}{} driver={}", self.name, self.instance, self.driver)?;
        match self.details {
            Details::None => {}
            Details::Cpu { apic_id, bsp } => write!(
                out,
                " apic={} role={}",
                apic_id,
                if bsp { "bsp" } else { "ap" }
            )?,
            Details::Framebuffer { width, height } => write!(out, " pixels={}x{}", width, height)?,
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
        Ok(())
    }
}

/// Directory metadata copied out under the inventory lock, without allocation.
#[derive(Clone, Copy)]
pub struct Entry {
    pub name: [u8; 11],
    pub cpu: Option<u8>,
}

/// Registration is boot-only, bounded, and rejects ambiguous identities.
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    InvalidName,
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
        if device.file_name().is_none() {
            return Err(Error::InvalidName);
        }
        if self.records[..self.len]
            .iter()
            .flatten()
            .any(|d| d.file_name() == device.file_name())
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
    pub fn entries(&self) -> [Option<Entry>; CAPACITY] {
        let mut entries = [None; CAPACITY];
        for (slot, device) in entries
            .iter_mut()
            .zip(self.records[..self.len].iter().flatten())
        {
            *slot = Some(Entry {
                name: device.file_name().expect("validated registered name"),
                cpu: if matches!(device.details, Details::Cpu { .. }) {
                    Some(device.instance)
                } else {
                    None
                },
            });
        }
        entries
    }
    pub fn find(&self, name: &[u8; 11]) -> Option<Device> {
        self.records[..self.len]
            .iter()
            .flatten()
            .find(|d| d.file_name().as_ref() == Some(name))
            .copied()
    }
    pub fn cpu(&self, index: u8) -> Option<Device> {
        self.records[..self.len]
            .iter()
            .flatten()
            .find(|d| d.instance == index && matches!(d.details, Details::Cpu { .. }))
            .copied()
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
                out.write_str("  ")?;
                device.write_info(out)?;
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
    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn device_filenames_are_valid_unique_short_names() {
        let mut registry = Registry::new();
        for name in ["", "keyboard", "a/b", "a.b", "bad name"] {
            let mut device = cpu(0);
            device.name = name;
            assert_eq!(registry.register(device), Err(Error::InvalidName));
        }
        registry.register(cpu(0)).unwrap();
        let mut alias = cpu(0);
        alias.name = "CPU";
        assert_eq!(registry.register(alias), Err(Error::Duplicate));
        registry.register(cpu(10)).unwrap();
        let mut collision = cpu(0);
        collision.name = "cpu1";
        assert_eq!(registry.register(collision), Err(Error::Duplicate));
    }
}
