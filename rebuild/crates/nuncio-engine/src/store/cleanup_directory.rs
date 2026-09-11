use crate::store::{DirectoryIdentity, OwnedDirectory, StoreError};
use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags};
use std::{
    ffi::CString,
    fs::File,
    path::{Path, PathBuf},
};

pub(crate) struct CleanupDirectory {
    parent: File,
    directory: File,
    path: PathBuf,
    identity: DirectoryIdentity,
    parent_identity: DirectoryIdentity,
    entries: Vec<CString>,
}
impl CleanupDirectory {
    pub fn prepare(
        parent: &Path,
        parent_identity: DirectoryIdentity,
        owned: &OwnedDirectory,
        allowed: &[&str],
    ) -> Result<Option<Self>, StoreError> {
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let parent_file = File::from(
            rustix::fs::open(parent, flags, Mode::empty()).map_err(std::io::Error::from)?,
        );
        if DirectoryIdentity::file(&parent_file)? != parent_identity {
            return Err(StoreError::InvalidPath);
        }
        let fd = match rustix::fs::openat(&parent_file, owned.name.as_str(), flags, Mode::empty()) {
            Ok(fd) => fd,
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(e) => return Err(std::io::Error::from(e).into()),
        };
        let directory = File::from(fd);
        if DirectoryIdentity::file(&directory)? != owned.identity {
            return Err(StoreError::InvalidPath);
        }
        let mut entries = Vec::new();
        for entry in Dir::read_from(&directory).map_err(std::io::Error::from)? {
            let entry = entry.map_err(std::io::Error::from)?;
            let name = entry.file_name();
            if matches!(name.to_bytes(), b"." | b"..") {
                continue;
            }
            if entries.len() >= allowed.len()
                || !name.to_str().ok().is_some_and(|n| allowed.contains(&n))
            {
                return Err(StoreError::InvalidPath);
            }
            let stat = rustix::fs::statat(&directory, name, AtFlags::SYMLINK_NOFOLLOW)
                .map_err(std::io::Error::from)?;
            if !matches!(
                FileType::from_raw_mode(stat.st_mode),
                FileType::RegularFile | FileType::Symlink
            ) {
                return Err(StoreError::InvalidPath);
            }
            entries.push(name.to_owned());
        }
        let prepared = Self {
            parent: parent_file,
            directory,
            path: parent.join(&owned.name),
            identity: owned.identity,
            parent_identity,
            entries,
        };
        prepared.check_path()?;
        Ok(Some(prepared))
    }
    fn check_path(&self) -> Result<(), StoreError> {
        if !self.identity.matches(&self.path)?
            || !self
                .parent_identity
                .matches(self.path.parent().ok_or(StoreError::InvalidPath)?)?
        {
            return Err(StoreError::InvalidPath);
        }
        Ok(())
    }
    pub fn remove(self) -> Result<(), StoreError> {
        self.check_path()?;
        // Reads and unlinks use the retained directory handle. A pathname swap
        // cannot redirect these operations into a replacement directory.
        for entry in &self.entries {
            match rustix::fs::unlinkat(&self.directory, entry.as_c_str(), AtFlags::empty()) {
                Ok(()) | Err(rustix::io::Errno::NOENT) => {}
                Err(e) => return Err(std::io::Error::from(e).into()),
            }
        }
        self.directory.sync_all()?;
        self.check_path()?;
        rustix::fs::unlinkat(
            &self.parent,
            self.path.file_name().ok_or(StoreError::InvalidPath)?,
            AtFlags::REMOVEDIR,
        )
        .map_err(std::io::Error::from)?;
        self.parent.sync_all()?;
        Ok(())
    }
}
