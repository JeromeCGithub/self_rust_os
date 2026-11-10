//! Kernel-owned directory grants for the single immutable boot mount.
use crate::fat32::{BlockDevice, DirectoryKey, Fat32, FatError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Directory-grant upper bounds, intersected with syscall capability and file
/// access-mode checks. These are native rights, not POSIX mode bits.
pub struct Rights(u8);
impl Rights {
    /// Read file contents, enumerate directories, and load executable bytes.
    pub const READ: Self = Self(1);
    /// Create directories/files and modify file contents.
    pub const WRITE: Self = Self(2);
    /// Combined read and write rights.
    pub const READ_WRITE: Self = Self(3);
    pub(crate) const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Grant {
    root: DirectoryKey,
    rights: Rights,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Authority {
    System,
    Restricted([Option<Grant>; 8]),
}
impl Authority {
    pub(crate) const fn none() -> Self {
        Self::Restricted([None; 8])
    }
    pub(crate) fn narrow<B: BlockDevice>(
        &self,
        fs: &mut Fat32<B>,
        requests: &[(&str, Rights)],
    ) -> Result<Self, FatError> {
        if requests.len() > 8 {
            return Err(FatError::AccessDenied);
        }
        let mut result = [None; 8];
        for (index, &(path, rights)) in requests.iter().enumerate() {
            let root = fs.directory_key(path)?;
            self.check(fs, root, rights)?;
            result[index] = Some(Grant { root, rights });
        }
        Ok(Self::Restricted(result))
    }
    fn check<B: BlockDevice>(
        &self,
        fs: &mut Fat32<B>,
        target: DirectoryKey,
        rights: Rights,
    ) -> Result<(), FatError> {
        let Self::Restricted(grants) = self else {
            return Ok(());
        };
        let mut allowed = 0;
        for grant in grants.iter().flatten() {
            if fs.directory_contains(grant.root, target)? {
                allowed |= grant.rights.0;
            }
        }
        if Rights(allowed).contains(rights) {
            Ok(())
        } else {
            Err(FatError::AccessDenied)
        }
    }
    pub(crate) fn file<B: BlockDevice>(
        &self,
        fs: &mut Fat32<B>,
        path: &str,
        rights: Rights,
    ) -> Result<(), FatError> {
        if matches!(self, Self::System) {
            return Ok(());
        }
        let parent = fs.file_parent_key(path)?;
        self.check(fs, parent, rights)
    }
    pub(crate) fn directory<B: BlockDevice>(
        &self,
        fs: &mut Fat32<B>,
        path: &str,
        rights: Rights,
    ) -> Result<(), FatError> {
        if matches!(self, Self::System) {
            return Ok(());
        }
        let directory = fs.directory_key(path)?;
        self.check(fs, directory, rights)
    }
    pub(crate) fn recursive_create<B: BlockDevice>(
        &self,
        fs: &mut Fat32<B>,
        path: &str,
    ) -> Result<(), FatError> {
        if matches!(self, Self::System) {
            return Ok(());
        }
        let directory = fs.creation_parent_key(path)?;
        self.check(fs, directory, Rights::WRITE)
    }
    pub(crate) fn search<B: BlockDevice>(
        &self,
        fs: &mut Fat32<B>,
        path: &str,
    ) -> Result<(), FatError> {
        let Self::Restricted(grants) = self else {
            return fs.check_directory(path);
        };
        let directory = fs.directory_key(path)?;
        for grant in grants.iter().flatten() {
            if fs.directory_contains(grant.root, directory)?
                || fs.directory_contains(directory, grant.root)?
            {
                return Ok(());
            }
        }
        Err(FatError::AccessDenied)
    }
}
