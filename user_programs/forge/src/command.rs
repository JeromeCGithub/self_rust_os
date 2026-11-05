// Shell tokenization: bounded words with single/double quotes and backslash.
pub const MAX_WORDS: usize = 32;
pub struct Command {
    bytes: [u8; 512],
    starts: [usize; MAX_WORDS],
    lengths: [usize; MAX_WORDS],
    pub count: usize,
}
impl Command {
    pub fn parse(input: &[u8]) -> Result<Self, &'static str> {
        if input.len() > 512 || input.contains(&0) {
            return Err("invalid command length or NUL");
        }
        let mut result = Self {
            bytes: [0; 512],
            starts: [0; MAX_WORDS],
            lengths: [0; MAX_WORDS],
            count: 0,
        };
        let mut quote = None;
        let mut escaped = false;
        let mut active = false;
        let mut written = 0;
        for &byte in input {
            if !active && !byte.is_ascii_whitespace() {
                if result.count == MAX_WORDS {
                    return Err("too many arguments");
                }
                result.starts[result.count] = written;
                active = true;
            }
            let emit = if escaped {
                escaped = false;
                true
            } else if byte == b'\\' && quote != Some(b'\'') {
                escaped = true;
                false
            } else if let Some(delimiter) = quote {
                if byte == delimiter {
                    quote = None;
                    false
                } else {
                    true
                }
            } else if byte == b'\'' || byte == b'"' {
                quote = Some(byte);
                false
            } else if byte.is_ascii_whitespace() {
                if active {
                    result.count += 1;
                    active = false;
                }
                false
            } else {
                true
            };
            if emit {
                result.bytes[written] = byte;
                written += 1;
                result.lengths[result.count] += 1;
            }
        }
        if quote.is_some() || escaped {
            return Err("unterminated quote or escape");
        }
        if active {
            result.count += 1;
        }
        if core::str::from_utf8(&result.bytes[..written]).is_err() {
            return Err("invalid UTF-8");
        }
        Ok(result)
    }
    pub fn word(&self, index: usize) -> &str {
        core::str::from_utf8(
            &self.bytes[self.starts[index]..self.starts[index] + self.lengths[index]],
        )
        .unwrap()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_empty_arguments_whitespace_quotes_and_escapes() {
        let c = Command::parse(br#"./hello  alpha "two words" '' 'single quoted' a\ b \"quote\""#)
            .unwrap();
        assert_eq!(c.count, 7);
        for (i, expected) in [
            "./hello",
            "alpha",
            "two words",
            "",
            "single quoted",
            "a b",
            "\"quote\"",
        ]
        .iter()
        .enumerate()
        {
            assert_eq!(c.word(i), *expected);
        }
        assert_eq!(Command::parse(b"  \t ").unwrap().count, 0);
    }
    #[test]
    fn rejects_incomplete_or_excessive_commands() {
        for input in [
            b"app 'unterminated".as_slice(),
            b"app trailing\\",
            b"app \"bad",
        ] {
            assert!(Command::parse(input).is_err());
        }
        let mut many = [b' '; 100];
        for i in (0..many.len()).step_by(2) {
            many[i] = b'x';
        }
        assert!(Command::parse(&many).is_err());
        assert!(Command::parse(&[b'x'; 513]).is_err());
        assert!(Command::parse(b"app a\0b").is_err());
    }
}
