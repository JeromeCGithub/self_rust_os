//! PCI configuration mechanism #1. Discovery is independent of hardware access.
/// A PCI function address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Function {
    /// PCI bus number.
    pub bus: u8,
    /// Device on bus (0..32).
    pub device: u8,
    /// Function (0..8).
    pub function: u8,
}
impl Function {
    /// Aligned mechanism #1 configuration address.
    pub fn address(self, offset: u8) -> u32 {
        0x8000_0000
            | (u32::from(self.bus) << 16)
            | (u32::from(self.device) << 11)
            | (u32::from(self.function) << 8)
            | u32::from(offset & 0xfc)
    }
}

/// Find the supported Intel 82540EM among PCI functions.
pub fn discover(mut read: impl FnMut(Function, u8) -> u32) -> Option<Function> {
    for bus in 0..=255 {
        for device in 0..32 {
            let first = Function {
                bus,
                device,
                function: 0,
            };
            if read(first, 0) & 0xffff == 0xffff {
                continue;
            }
            let functions = if read(first, 0x0c) & (1 << 23) != 0 {
                8
            } else {
                1
            };
            for function in 0..functions {
                let address = Function {
                    bus,
                    device,
                    function,
                };
                if read(address, 0) == 0x100e_8086 {
                    return Some(address);
                }
            }
        }
    }
    None
}

/// Decode BAR0 for the supported device (32-bit memory BAR only).
pub fn memory_bar(raw: u32) -> Option<u64> {
    if raw == u32::MAX || raw & 7 != 0 || raw & !15 == 0 {
        None
    } else {
        Some(u64::from(raw & !15))
    }
}

/// Read a configuration dword. Boot-only callers serialize the address/data pair.
#[cfg(target_os = "none")]
pub fn read(function: Function, offset: u8) -> u32 {
    use x86_64::instructions::port::Port;
    // SAFETY: x86 PC configuration mechanism #1; Ring 0 boot runs on one CPU.
    // The aligned address selects only a PCI configuration register. No IRQ uses it.
    unsafe {
        Port::<u32>::new(0xcf8).write(function.address(offset));
        Port::<u32>::new(0xcfc).read()
    }
}

/// Enable memory decoding and DMA, preserving the adjacent W1C status register.
#[cfg(target_os = "none")]
pub fn enable_bus_master(function: Function) {
    use x86_64::instructions::port::Port;
    let command = read(function, 4) as u16;
    // SAFETY: Boot-only serialized configuration pair. A 16-bit write targets
    // the command register without writing PCI status bits in the upper half.
    unsafe {
        Port::<u32>::new(0xcf8).write(function.address(4));
        Port::<u16>::new(0xcfc).write(command | 6);
    }
}

/// Enable MMIO but prevent firmware DMA from running while rings are reset.
#[cfg(target_os = "none")]
pub fn enable_memory(function: Function) {
    use x86_64::instructions::port::Port;
    let command = read(function, 4) as u16;
    // SAFETY: Boot-only serialized PCI command access. The halfword write
    // preserves W1C status; disabling bus mastering stops access to old rings.
    unsafe {
        Port::<u32>::new(0xcf8).write(function.address(4));
        Port::<u16>::new(0xcfc).write((command | 2) & !4);
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;
    #[test]
    fn discovers_multifunction_and_absent_device() {
        let wanted = Function {
            bus: 0,
            device: 4,
            function: 2,
        };
        let found = discover(|f, offset| {
            if f == wanted && offset == 0 {
                0x100e_8086
            } else if f.bus == 0 && f.device == 4 && f.function == 0 {
                if offset == 0x0c {
                    0x0080_0000
                } else {
                    0x1111_8086
                }
            } else {
                u32::MAX
            }
        });
        assert_eq!(found, Some(wanted));
        assert_eq!(discover(|_, _| u32::MAX), None);
    }
    #[test]
    fn validates_memory_bar_and_config_address() {
        assert_eq!(memory_bar(0xfebc0000), Some(0xfebc0000));
        for raw in [0, u32::MAX, 0x1001, 0x1004] {
            assert_eq!(memory_bar(raw), None);
        }
        assert_eq!(
            Function {
                bus: 2,
                device: 3,
                function: 4
            }
            .address(0x13),
            0x8002_1c10
        );
    }
}
