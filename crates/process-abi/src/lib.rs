//! Bounded UTF-8 argv request, shared by the kernel and Ring 3 SDK.
#![no_std]
/// Process-wide irreversible worker rights and heap budget.
pub mod isolation;
/// Owned process identity and child event records.
pub mod supervision;
/// Native argument-bearing spawn: path pointer, argument record pointer, flags.
pub const SYS_SPAWN_ARGS: u64 = 29;
/// Complete readable request size, including the 16-byte header.
pub const SIZE: usize = 2048;
/// Maximum argument count, including argv[0].
pub const MAX_ARGS: usize = 32;
/// Maximum combined string storage, including each NUL terminator.
pub const DATA_SIZE: usize = SIZE - 16;
/// Unsupported version, excessive arguments/bytes, embedded NUL or invalid UTF-8.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidArguments;
/// Initial C-call entry registers and private-stack addresses.
pub struct Startup {
    /// Stack pointer, 8 modulo 16 for the native extern-C entry function.
    pub stack_pointer: u64,
    /// Number of argument pointers before the null sentinel.
    pub argc: u64,
    /// User virtual address of the first argv pointer.
    pub argv_pointer: u64,
    /// User virtual address of the null-terminated environment pointer vector.
    pub env_pointer: u64,
}
/// Little-endian version/count header followed by concatenated NUL strings.
#[repr(transparent)]
pub struct Arguments([u8; SIZE]);
impl Arguments {
    /// Encode an explicit argv, including caller-selected argv[0].
    pub fn new(argv: &[&str]) -> Result<Self, InvalidArguments> {
        if argv.len() > MAX_ARGS {
            return Err(InvalidArguments);
        }
        let mut bytes = [0; SIZE];
        bytes[..8].copy_from_slice(&1u64.to_le_bytes());
        bytes[8..16].copy_from_slice(&(argv.len() as u64).to_le_bytes());
        let mut offset: usize = 16;
        for arg in argv {
            let end = offset
                .checked_add(arg.len())
                .filter(|&n| n < SIZE)
                .ok_or(InvalidArguments)?;
            if arg.as_bytes().contains(&0) {
                return Err(InvalidArguments);
            }
            bytes[offset..end].copy_from_slice(arg.as_bytes());
            offset = end + 1;
        }
        Ok(Self(bytes))
    }
    /// Validate a kernel-owned snapshot, retaining no user pointers.
    pub fn from_bytes(bytes: [u8; SIZE]) -> Result<Self, InvalidArguments> {
        let version = u64::from_le_bytes(bytes[..8].try_into().map_err(|_| InvalidArguments)?);
        let count = u64::from_le_bytes(bytes[8..16].try_into().map_err(|_| InvalidArguments)?);
        if version != 1 || count > MAX_ARGS as u64 {
            return Err(InvalidArguments);
        }
        let mut rest = &bytes[16..];
        for _ in 0..count {
            let end = rest.iter().position(|&b| b == 0).ok_or(InvalidArguments)?;
            core::str::from_utf8(&rest[..end]).map_err(|_| InvalidArguments)?;
            rest = &rest[end + 1..];
        }
        Ok(Self(bytes))
    }
    /// Build a private final stack page with argc, argv, null sentinels and strings.
    pub fn stack_page(
        &self,
        stack_top: u64,
        page: &mut [u8; 4096],
    ) -> Result<Startup, InvalidArguments> {
        if stack_top < 8192 || stack_top > 1 << 47 || stack_top % 4096 != 0 {
            return Err(InvalidArguments);
        }
        let base = stack_top - 4096;
        page.fill(0);
        page[..8].copy_from_slice(&(self.len() as u64).to_le_bytes());
        let mut offset = 512;
        for index in 0..self.len() {
            let string = self.get(index).ok_or(InvalidArguments)?;
            let end = offset + string.len();
            if end >= page.len() {
                return Err(InvalidArguments);
            }
            page[8 + index * 8..16 + index * 8]
                .copy_from_slice(&(base + offset as u64).to_le_bytes());
            page[offset..end].copy_from_slice(string.as_bytes());
            offset = end + 1;
        }
        Ok(Startup {
            stack_pointer: base - 8,
            argc: self.len() as u64,
            argv_pointer: base + 8,
            env_pointer: base + 8 + (self.len() as u64 + 1) * 8,
        })
    }
    /// Number of strings.
    pub fn len(&self) -> usize {
        self.0[8] as usize
    }
    /// Indexed UTF-8 string.
    pub fn get(&self, index: usize) -> Option<&str> {
        if index >= self.len() {
            return None;
        }
        core::str::from_utf8(self.0[16..].split(|&b| b == 0).nth(index)?).ok()
    }
    /// Fixed-size wire record.
    pub fn as_bytes(&self) -> &[u8; SIZE] {
        &self.0
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn startup_stack_has_private_pointers_null_sentinel_and_c_alignment() {
        let arguments = Arguments::new(&["app", "", "two words"]).unwrap();
        let mut page = [255; 4096];
        let top = 0x3000000u64;
        let startup = arguments.stack_page(top, &mut page).unwrap();
        assert_eq!(startup.stack_pointer % 16, 8);
        assert_eq!(startup.argc, 3);
        assert_eq!(startup.argv_pointer, top - 4096 + 8);
        assert_eq!(u64::from_le_bytes(page[..8].try_into().unwrap()), 3);
        for (index, expected) in ["app", "", "two words"].iter().enumerate() {
            let pointer =
                u64::from_le_bytes(page[8 + index * 8..16 + index * 8].try_into().unwrap());
            assert!((top - 4096 + 512..top).contains(&pointer));
            let offset = (pointer - (top - 4096)) as usize;
            assert_eq!(&page[offset..offset + expected.len()], expected.as_bytes());
            assert_eq!(page[offset + expected.len()], 0);
        }
        assert_eq!(&page[32..48], &[0; 16], "argv terminator and empty envp");
        assert!(arguments.stack_page(0, &mut page).is_err());
        assert!(arguments.stack_page(top + 1, &mut page).is_err());
        assert!(arguments.stack_page(u64::MAX, &mut page).is_err());
    }
    #[test]
    fn round_trip_preserves_argv0_empty_spaces_and_unicode() {
        let input = ["app", "", "two words", "café", "\\quote\""];
        let encoded = Arguments::new(&input).unwrap();
        let decoded = Arguments::from_bytes(*encoded.as_bytes()).unwrap();
        assert_eq!(decoded.len(), input.len());
        for (i, expected) in input.iter().enumerate() {
            assert_eq!(decoded.get(i), Some(*expected));
        }
        assert_eq!(decoded.get(input.len()), None);
        assert_eq!(Arguments::new(&[]).unwrap().len(), 0);
    }
    #[test]
    fn accepts_exact_limits_rejects_overflow_and_embedded_nul() {
        let bytes = [b'a'; DATA_SIZE];
        let maximum = core::str::from_utf8(&bytes[..DATA_SIZE - 1]).unwrap();
        assert!(Arguments::new(&[maximum]).is_ok());
        assert!(Arguments::new(&[core::str::from_utf8(&bytes).unwrap()]).is_err());
        assert!(Arguments::new(&[""; MAX_ARGS]).is_ok());
        assert!(Arguments::new(&[""; MAX_ARGS + 1]).is_err());
        assert!(Arguments::new(&["bad\0argument"]).is_err());
    }
    #[test]
    fn rejects_version_count_missing_nul_and_invalid_utf8() {
        let mut bytes = [0; SIZE];
        assert!(Arguments::from_bytes(bytes).is_err());
        bytes[..8].copy_from_slice(&1u64.to_le_bytes());
        bytes[8..16].copy_from_slice(&33u64.to_le_bytes());
        assert!(Arguments::from_bytes(bytes).is_err());
        bytes[8..16].copy_from_slice(&1u64.to_le_bytes());
        bytes[16..].fill(b'x');
        assert!(Arguments::from_bytes(bytes).is_err());
        bytes[16] = 255;
        bytes[17] = 0;
        assert!(Arguments::from_bytes(bytes).is_err());
    }
}

/// General argv/envp spawn: path pointer, request pointer, request byte length.
pub const SYS_SPAWN_ENV: u64 = 30;
/// Maximum entries in either startup vector.
pub const MAX_VECTOR: usize = 4096;
/// Combined UTF-8 string bytes including NUL terminators (128 KiB).
pub const MAX_STARTUP_BYTES: usize = 128 * 1024;
/// Explicit length avoids scanning untrusted pointers for string terminators.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct StringRef {
    pub pointer: u64,
    pub length: u64,
}
/// Versioned, padding-free general launch request. flags 0/1 select ordinary or connected I/O.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SpawnRequest {
    pub version: u64,
    pub flags: u64,
    pub argv: u64,
    pub argc: u64,
    pub envp: u64,
    pub envc: u64,
}
/// Validate vectors and return their page-rounded initial stack storage.
pub fn startup_size(argv: &[&str], envp: &[&str]) -> Result<usize, InvalidArguments> {
    if argv.len() > MAX_VECTOR || envp.len() > MAX_VECTOR {
        return Err(InvalidArguments);
    }
    let mut strings = 0usize;
    for string in argv.iter().chain(envp.iter()) {
        strings = strings
            .checked_add(string.len() + 1)
            .filter(|&n| n <= MAX_STARTUP_BYTES)
            .ok_or(InvalidArguments)?;
        if string.as_bytes().contains(&0) {
            return Err(InvalidArguments);
        }
    }
    let bytes = 8 + (argv.len() + envp.len() + 2) * 8 + strings;
    Ok((bytes + 4095) & !4095)
}
/// Serialize argument/environment pointer tables and strings into owned pages.
pub fn write_startup(
    top: u64,
    argv: &[&str],
    envp: &[&str],
    bytes: &mut [u8],
) -> Result<Startup, InvalidArguments> {
    let size = startup_size(argv, envp)?;
    if bytes.len() != size || top <= size as u64 + 4096 || top > 1 << 47 || top % 4096 != 0 {
        return Err(InvalidArguments);
    }
    let base = top - size as u64;
    bytes.fill(0);
    bytes[..8].copy_from_slice(&(argv.len() as u64).to_le_bytes());
    let mut offset = 8 + (argv.len() + envp.len() + 2) * 8;
    for (index, string) in argv.iter().chain(envp.iter()).enumerate() {
        let slot = if index < argv.len() {
            index + 1
        } else {
            index + 2
        };
        bytes[slot * 8..slot * 8 + 8].copy_from_slice(&(base + offset as u64).to_le_bytes());
        bytes[offset..offset + string.len()].copy_from_slice(string.as_bytes());
        offset += string.len() + 1;
    }
    Ok(Startup {
        stack_pointer: base - 8,
        argc: argv.len() as u64,
        argv_pointer: base + 8,
        env_pointer: base + 8 + (argv.len() as u64 + 1) * 8,
    })
}
#[cfg(test)]
mod environment_tests {
    use super::*;
    #[test]
    fn startup_environment_preserves_strings_and_null_sentinels() {
        let argv = ["program", "two words", ""];
        let env = ["KEY=value", "UNICODE=café"];
        let mut bytes = [255; 4096];
        assert_eq!(startup_size(&argv, &env), Ok(4096));
        let startup = write_startup(0x400000000, &argv, &env, &mut bytes).unwrap();
        assert_eq!(startup.argc, 3);
        assert_eq!(startup.stack_pointer % 16, 8);
        let base = 0x400000000 - 4096;
        for (index, expected) in argv.iter().chain(env.iter()).enumerate() {
            let slot = if index < argv.len() {
                index + 1
            } else {
                index + 2
            };
            let pointer = u64::from_le_bytes(bytes[slot * 8..slot * 8 + 8].try_into().unwrap());
            let offset = (pointer - base) as usize;
            assert_eq!(&bytes[offset..offset + expected.len()], expected.as_bytes());
            assert_eq!(bytes[offset + expected.len()], 0);
        }
        assert_eq!(&bytes[32..40], &[0; 8]);
        assert_eq!(&bytes[56..64], &[0; 8]);
        assert!(startup_size(&["bad\0data"], &[]).is_err());
        assert!(startup_size(&[], &[""; MAX_VECTOR + 1]).is_err());
        assert!(write_startup(4096, &argv, &env, &mut bytes).is_err());
        assert_eq!(core::mem::size_of::<SpawnRequest>(), 48);
        assert_eq!(core::mem::size_of::<StringRef>(), 16);
    }
}

/// Descriptive native identity/capability ABI; authority remains kernel-owned.
pub mod principal;
