//! Bounded PC CMOS UTC clock, with wire-format conversion independently tested.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Raw RTC register sample, including format flags.
pub struct Snapshot {
    /// Raw second register.
    pub second: u8,
    /// Raw minute register.
    pub minute: u8,
    /// Raw hour register.
    pub hour: u8,
    /// Raw day register.
    pub day: u8,
    /// Raw month register.
    pub month: u8,
    /// Raw year register.
    pub year: u8,
    /// Raw century register.
    pub century: u8,
    /// Raw status b register.
    pub status_b: u8,
}
/// Decode a calendar-valid UTC sample into Unix seconds.
pub fn unix_seconds(s: Snapshot) -> Option<u64> {
    fn decode(n: u8, binary: bool) -> Option<u64> {
        if binary {
            Some(u64::from(n))
        } else if n & 15 <= 9 && n >> 4 <= 9 {
            Some(u64::from(n >> 4) * 10 + u64::from(n & 15))
        } else {
            None
        }
    }
    fn leap(y: u64) -> bool {
        y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
    }
    if s.status_b & 0x80 != 0 {
        return None;
    }
    let binary = s.status_b & 4 != 0;
    let year = decode(s.century, binary)? * 100 + decode(s.year, binary)?;
    let month = decode(s.month, binary)?;
    let day = decode(s.day, binary)?;
    let minute = decode(s.minute, binary)?;
    let second = decode(s.second, binary)?;
    let mut hour = decode(s.hour & 0x7f, binary)?;
    if s.status_b & 2 == 0 {
        if !(1..=12).contains(&hour) {
            return None;
        }
        hour = hour % 12 + if s.hour & 0x80 != 0 { 12 } else { 0 };
    } else if s.hour & 0x80 != 0 {
        return None;
    }
    if !(1970..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    let months = [
        31,
        if leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if day == 0 || day > months[month as usize - 1] {
        return None;
    }
    let mut days = 0;
    for y in 1970..year {
        days += if leap(y) { 366 } else { 365 };
    }
    days += months[..month as usize - 1].iter().sum::<u64>() + day - 1;
    Some(days * 86400 + hour * 3600 + minute * 60 + second)
}
/// Read two matching samples with bounded update-in-progress retries.
pub fn stable(mut read: impl FnMut(u8) -> u8) -> Option<Snapshot> {
    let mut previous = None;
    for _ in 0..8 {
        if read(0x0a) & 0x80 != 0 {
            previous = None;
            continue;
        }
        let s = Snapshot {
            second: read(0),
            minute: read(2),
            hour: read(4),
            day: read(7),
            month: read(8),
            year: read(9),
            century: read(0x32),
            status_b: read(0x0b),
        };
        if read(0x0a) & 0x80 != 0 {
            previous = None;
            continue;
        }
        if previous == Some(s) {
            return Some(s);
        }
        previous = Some(s);
    }
    None
}
#[cfg(target_os = "none")]
/// Read validated UTC seconds from the QEMU PC CMOS clock.
pub fn now() -> Option<u64> {
    use x86_64::instructions::port::Port;
    static CMOS: spin::Mutex<()> = spin::Mutex::new(());
    x86_64::instructions::interrupts::without_interrupts(|| {
        let _guard = CMOS.lock();
        let snapshot = stable(|reg| {
            // SAFETY: Kernel entry serializes PC CMOS index/data access. Only the
            // fixed time/status registers above are read; no RTC interrupt/status
            // acknowledgement registers are touched. QEMU's write-only index port
            // cannot preserve a selector by readback. Select with NMI enabled,
            // matching the kernel's normal boot policy, and never change PIC masks.
            unsafe {
                Port::<u8>::new(0x70).write(reg);
                Port::<u8>::new(0x71).read()
            }
        })?;
        unix_seconds(snapshot)
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> Snapshot {
        Snapshot {
            second: 0,
            minute: 0,
            hour: 0,
            day: 1,
            month: 1,
            year: 0,
            century: 20,
            status_b: 6,
        }
    }
    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn binary_bcd_and_twelve_hour_dates() {
        assert_eq!(unix_seconds(sample()), Some(946684800));
        let mut s = sample();
        s.status_b = 0;
        s.century = 0x20;
        s.hour = 0x12;
        assert_eq!(unix_seconds(s), Some(946684800));
        s.hour = 0x92;
        assert_eq!(unix_seconds(s), Some(946728000));
    }
    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn validates_calendar_leap_year_and_malformed_bcd() {
        let mut s = sample();
        s.month = 2;
        s.day = 29;
        assert_eq!(unix_seconds(s), Some(951782400));
        s.year = 1;
        assert_eq!(unix_seconds(s), None);
        s = sample();
        s.status_b = 2;
        s.century = 0x2a;
        assert_eq!(unix_seconds(s), None);
        s = sample();
        s.hour = 24;
        assert_eq!(unix_seconds(s), None);
        s = sample();
        s.month = 0;
        assert_eq!(unix_seconds(s), None);
    }
    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn unstable_clock_is_bounded() {
        let mut calls = 0;
        assert!(stable(|_| {
            calls += 1;
            0x80
        })
        .is_none());
        assert!(calls <= 128);
    }
    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn stable_snapshot_preserves_format() {
        assert_eq!(
            stable(|reg| match reg {
                0x0a => 0,
                0x0b => 6,
                7 | 8 => 1,
                0x32 => 20,
                _ => 0,
            }),
            Some(sample())
        );
    }
}
