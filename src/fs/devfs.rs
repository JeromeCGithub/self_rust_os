//! Kernel-owned /dev namespace. Device operations never reach the backing FAT.
use core::fmt::{self, Write};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Error {
    InvalidPath,
    NotFound,
    IsDirectory,
    NotDirectory,
    ReadOnly,
    Unavailable,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Node {
    Directory,
    Null,
    Zero,
    Random,
    Inventory,
    Devices,
    Cpus,
    CpuDirectory(u8),
    CpuInfo(u8),
    DeviceDirectory([u8; 11]),
    DeviceInfo([u8; 11]),
}

/// Bounded canonical absolute path; relative paths retain the existing root base.
/// Parent traversal is rejected so mount boundaries cannot be escaped or spoofed.
pub(crate) struct Path {
    bytes: [u8; 256],
    len: usize,
}
impl Path {
    pub(crate) fn new(path: &str) -> Result<Self, Error> {
        if path.len() >= 255 || path.as_bytes().contains(&0) {
            return Err(Error::InvalidPath);
        }
        let mut result = Self {
            bytes: [0; 256],
            len: 0,
        };
        for component in path.split('/').filter(|c| !c.is_empty() && *c != ".") {
            if component == ".." {
                return Err(Error::InvalidPath);
            }
            // FAT treats an empty extension as the same short name. Route this
            // alias through the mount too, so DEV. cannot create a backing DEV.
            let component = component
                .strip_suffix('.')
                .filter(|base| !base.contains('.'))
                .unwrap_or(component);
            result.bytes[result.len] = b'/';
            result.len += 1;
            let end = result.len + component.len();
            result.bytes[result.len..end].copy_from_slice(component.as_bytes());
            result.len = end;
        }
        if result.len == 0 {
            result.bytes[0] = b'/';
            result.len = 1;
        }
        Ok(result)
    }
    pub(crate) fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).expect("copied UTF-8 path")
    }
    pub(crate) fn device(&self) -> Option<&str> {
        let path = &self.as_str()[1..];
        let (head, tail) = path.split_once('/').unwrap_or((path, ""));
        head.eq_ignore_ascii_case("dev").then_some(tail)
    }
}

fn device_name(component: &str) -> Result<[u8; 11], Error> {
    if component.is_empty()
        || component.len() > 8
        || !component
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err(Error::NotFound);
    }
    let mut name = [b' '; 11];
    for (index, byte) in component.bytes().enumerate() {
        name[index] = byte.to_ascii_uppercase();
    }
    Ok(name)
}

pub(crate) fn lookup(path: &str) -> Result<Node, Error> {
    let mut components = path.split('/');
    let first = components.next().unwrap_or("");
    let second = components.next();
    let third = components.next();
    if components.next().is_some() {
        return Err(Error::NotFound);
    }
    if first.eq_ignore_ascii_case("devices") {
        return match (second, third) {
            (None, None) => Ok(Node::Devices),
            (Some(name), None) if name.eq_ignore_ascii_case("info") => Ok(Node::Inventory),
            (Some(name), None) => Ok(Node::DeviceDirectory(device_name(name)?)),
            (Some(name), Some(info)) if info.eq_ignore_ascii_case("info") => {
                Ok(Node::DeviceInfo(device_name(name)?))
            }
            _ => Err(Error::NotFound),
        };
    }
    if first.eq_ignore_ascii_case("cpu") {
        if second.is_none() {
            return Ok(Node::Cpus);
        }
        let number = second.ok_or(Error::NotFound)?;
        if number.is_empty()
            || !number.bytes().all(|b| b.is_ascii_digit())
            || (number.len() > 1 && number.starts_with('0'))
        {
            return Err(Error::NotFound);
        }
        let index = number.parse::<u8>().map_err(|_| Error::NotFound)?;
        if usize::from(index) >= crate::arch::x86_64::acpi::MAX_CPUS {
            return Err(Error::NotFound);
        }
        return match third {
            None => Ok(Node::CpuDirectory(index)),
            Some(info) if info.eq_ignore_ascii_case("info") => Ok(Node::CpuInfo(index)),
            _ => Err(Error::NotFound),
        };
    }
    if second.is_some() {
        return Err(Error::NotFound);
    }
    match first {
        "" => Ok(Node::Directory),
        p if p.eq_ignore_ascii_case("null") => Ok(Node::Null),
        p if p.eq_ignore_ascii_case("zero") => Ok(Node::Zero),
        p if p.eq_ignore_ascii_case("random") => Ok(Node::Random),
        _ => Err(Error::NotFound),
    }
}

pub(crate) struct Prefix<'a> {
    pub(crate) bytes: &'a mut [u8],
    pub(crate) written: usize,
}
impl Write for Prefix<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let count = s.len().min(self.bytes.len() - self.written);
        self.bytes[self.written..self.written + count].copy_from_slice(&s.as_bytes()[..count]);
        self.written += count;
        Ok(())
    }
}

pub(crate) fn read(
    node: Node,
    out: &mut [u8],
    inventory: impl FnOnce(&mut Prefix<'_>) -> fmt::Result,
    random: impl FnOnce(&mut [u8]) -> bool,
) -> Result<usize, Error> {
    match node {
        Node::Directory
        | Node::Devices
        | Node::Cpus
        | Node::CpuDirectory(_)
        | Node::DeviceDirectory(_) => Err(Error::IsDirectory),
        Node::Null => Ok(0),
        Node::Zero => {
            out.fill(0);
            Ok(out.len())
        }
        Node::Inventory | Node::CpuInfo(_) | Node::DeviceInfo(_) => {
            let mut prefix = Prefix {
                bytes: out,
                written: 0,
            };
            inventory(&mut prefix).map_err(|_| Error::Unavailable)?;
            Ok(prefix.written)
        }
        Node::Random => {
            let count = out.len().min(rustos_platform_abi::MAX_RANDOM_BYTES);
            if count == 0 || random(&mut out[..count]) {
                Ok(count)
            } else {
                Err(Error::Unavailable)
            }
        }
    }
}
pub(crate) fn write(node: Node, bytes: &[u8]) -> Result<usize, Error> {
    match node {
        Node::Null | Node::Zero => Ok(bytes.len()),
        Node::Directory
        | Node::Devices
        | Node::Cpus
        | Node::CpuDirectory(_)
        | Node::DeviceDirectory(_) => Err(Error::IsDirectory),
        _ => Err(Error::ReadOnly),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test_case]
    fn device_paths_cannot_alias_disk_or_escape_the_mount() {
        for raw in ["/dev", "//DeV/./", "dev/"] {
            let path = Path::new(raw).unwrap();
            assert_eq!(path.device(), Some(""));
        }
        assert_eq!(Path::new("/dev/zero/").unwrap().device(), Some("zero"));
        assert_eq!(
            Path::new("/DEV./ZERO.").unwrap().device(),
            Some("ZERO"),
            "FAT empty-extension aliases must stay inside the device mount"
        );
        for raw in ["/device", "/DEVIL.TXT", "/tmp/dev"] {
            assert_eq!(Path::new(raw).unwrap().device(), None);
        }
        for raw in [
            "/dev/../readme.txt",
            "/./dev/../dev/null",
            "/dev/zero\0.txt",
        ] {
            assert!(Path::new(raw).is_err());
        }
        assert!(Path::new(&"a".repeat(255)).is_err());
        assert_eq!(Path::new(&"a".repeat(254)).unwrap().as_str().len(), 255);
        assert_eq!(lookup("missing"), Err(Error::NotFound));
    }
    #[test_case]
    fn null_zero_inventory_and_random_have_bounded_observable_io() {
        let mut guarded = [0xa5; 18];
        let result = read(Node::Null, &mut guarded[1..17], |_| Ok(()), |_| true);
        assert_eq!(result, Ok(0));
        assert_eq!(guarded, [0xa5; 18]);
        assert_eq!(
            read(Node::Zero, &mut guarded[1..17], |_| Ok(()), |_| true),
            Ok(16)
        );
        assert_eq!(guarded[0], 0xa5);
        assert_eq!(guarded[17], 0xa5);
        assert_eq!(&guarded[1..17], &[0; 16]);
        for node in [Node::Null, Node::Zero] {
            assert_eq!(write(node, b"abc"), Ok(3));
        }
        for node in [Node::Directory, Node::Inventory, Node::Random] {
            assert!(write(node, b"abc").is_err());
        }
        assert_eq!(
            read(Node::Directory, &mut [], |_| Ok(()), |_| true),
            Err(Error::IsDirectory)
        );
        assert_eq!(
            read(
                Node::Inventory,
                &mut guarded[1..5],
                |out| out.write_str("inventory"),
                |_| true
            ),
            Ok(4)
        );
        assert_eq!(&guarded[1..5], b"inve");
        assert_eq!(
            read(Node::Random, &mut guarded[1..5], |_| Ok(()), |_| false),
            Err(Error::Unavailable)
        );
        assert_eq!(read(Node::Random, &mut [], |_| Ok(()), |_| false), Ok(0));
        assert_eq!(
            read(
                Node::Random,
                &mut guarded[1..5],
                |_| Ok(()),
                |out| {
                    out.fill(7);
                    true
                }
            ),
            Ok(4)
        );
        assert_eq!(&guarded[1..5], &[7; 4]);
        let mut bounded = [0xa5; 258];
        assert_eq!(
            read(
                Node::Random,
                &mut bounded[1..],
                |_| Ok(()),
                |out| {
                    assert_eq!(out.len(), 256, "one entropy call has a fixed work bound");
                    out.fill(9);
                    true
                }
            ),
            Ok(256)
        );
        assert_eq!(bounded[0], 0xa5);
        assert_eq!(bounded[257], 0xa5);
        assert_eq!(
            read(
                Node::Inventory,
                &mut [],
                |out| out.write_str("inventory"),
                |_| true
            ),
            Ok(0)
        );
    }
    #[test_case]
    fn directory_routes_distinguish_cpu_and_device_information() {
        assert_eq!(lookup("devices"), Ok(Node::Devices));
        assert_eq!(lookup("devices/INFO"), Ok(Node::Inventory));
        assert_eq!(lookup("cpu"), Ok(Node::Cpus));
        assert_eq!(lookup("CPU/0/INFO"), Ok(Node::CpuInfo(0)));
        assert_eq!(lookup("cpu/31"), Ok(Node::CpuDirectory(31)));
        assert_eq!(
            lookup("devices/cpu31/info"),
            Ok(Node::DeviceInfo(*b"CPU31      "))
        );
        for path in [
            "cpu/32",
            "cpu/256/info",
            "cpu/00/info",
            "cpu/-1",
            "devices/cpu0/info/extra",
            "devices/a.b/info",
        ] {
            assert_eq!(lookup(path), Err(Error::NotFound));
        }
        for node in [
            Node::Devices,
            Node::Cpus,
            Node::CpuDirectory(0),
            Node::DeviceDirectory(*b"CPU0       "),
        ] {
            assert_eq!(
                read(node, &mut [], |_| Ok(()), |_| true),
                Err(Error::IsDirectory)
            );
            assert!(write(node, b"no").is_err());
        }
    }
}
