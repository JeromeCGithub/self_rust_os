//! Bounded formatting before a single console write.
use core::fmt;
pub fn format<'a>(output: &'a mut [u8], args: fmt::Arguments<'_>) -> Result<&'a [u8], fmt::Error> {
    struct Buffer<'a> {
        bytes: &'a mut [u8],
        len: usize,
    }
    impl fmt::Write for Buffer<'_> {
        fn write_str(&mut self, value: &str) -> fmt::Result {
            let end = self.len.checked_add(value.len()).ok_or(fmt::Error)?;
            let destination = self.bytes.get_mut(self.len..end).ok_or(fmt::Error)?;
            destination.copy_from_slice(value.as_bytes());
            self.len = end;
            Ok(())
        }
    }
    let mut buffer = Buffer {
        bytes: output,
        len: 0,
    };
    fmt::write(&mut buffer, args)?;
    let length = buffer.len;
    Ok(&output[..length])
}
#[cfg(test)]
mod tests {
    #[test]
    fn complete_record_preserves_values_and_newline() {
        let mut bytes = [0; 64];
        assert_eq!(
            super::format(&mut bytes, format_args!("[unit] run={} pid={}\n", 12, 3)),
            Ok(&b"[unit] run=12 pid=3\n"[..])
        );
    }
    #[test]
    fn exact_capacity_succeeds_and_overflow_is_explicit() {
        let mut bytes = [0; 4];
        assert_eq!(
            super::format(&mut bytes, format_args!("{}", "four")),
            Ok(&b"four"[..])
        );
        assert!(super::format(&mut bytes, format_args!("{}", "oversize")).is_err());
    }
}
