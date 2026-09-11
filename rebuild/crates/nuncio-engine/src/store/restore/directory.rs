use crate::store::StoreError;
use rustix::fs::{Mode, OFlags};
use std::{fs::File, os::unix::fs::MetadataExt, path::Path};

pub(super) struct StagingDirectory {
    temporary: tempfile::TempDir,
    parent: File,
    directory: File,
}
impl StagingDirectory {
    pub fn new(parent: &Path) -> Result<Self, StoreError> {
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let parent_file = File::from(
            rustix::fs::open(parent, flags, Mode::empty()).map_err(std::io::Error::from)?,
        );
        let mut temporary = tempfile::Builder::new()
            .prefix(".restore-")
            .tempdir_in(parent)?;
        // Until both identities are anchored, pathname cleanup is not safe.
        temporary.disable_cleanup(true);
        let directory = File::from(
            rustix::fs::openat(
                &parent_file,
                temporary
                    .path()
                    .file_name()
                    .ok_or(StoreError::InvalidPath)?,
                flags,
                Mode::empty(),
            )
            .map_err(std::io::Error::from)?,
        );
        let mut stage = Self {
            temporary,
            parent: parent_file,
            directory,
        };
        if !stage.matches()? {
            return Err(StoreError::InvalidPath);
        }
        stage.parent.sync_all()?;
        stage.temporary.disable_cleanup(false);
        Ok(stage)
    }
    pub fn path(&self) -> &Path {
        self.temporary.path()
    }
    pub(super) fn identities(
        &self,
    ) -> Result<
        (
            crate::store::DirectoryIdentity,
            crate::store::DirectoryIdentity,
        ),
        StoreError,
    > {
        Ok((
            crate::store::DirectoryIdentity::file(&self.parent)?,
            crate::store::DirectoryIdentity::file(&self.directory)?,
        ))
    }
    fn matches(&self) -> Result<bool, StoreError> {
        Ok(same_directory(
            self.path().parent().ok_or(StoreError::InvalidPath)?,
            &self.parent,
        )? && same_directory(self.path(), &self.directory)?)
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub fn activate(&mut self, target: &Path) -> Result<(), StoreError> {
        if target.parent() != self.path().parent() || !self.matches()? {
            return Err(StoreError::InvalidPath);
        }
        self.directory.sync_all()?;
        rustix::fs::renameat_with(
            &self.parent,
            self.path().file_name().ok_or(StoreError::InvalidPath)?,
            &self.parent,
            target.file_name().ok_or(StoreError::InvalidPath)?,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(std::io::Error::from)?;
        // Rename committed. Preserve the new keys even if its visible path or
        // the durability acknowledgement changes before the caller hears back.
        self.temporary.disable_cleanup(true);
        let visible = same_directory(
            target
                .parent()
                .ok_or(StoreError::RestoreActivationUncertain)?,
            &self.parent,
        )
        .and_then(|parent| Ok(parent && same_directory(target, &self.directory)?));
        if !matches!(visible, Ok(true)) {
            return Err(StoreError::RestoreActivationUncertain);
        }
        self.parent
            .sync_all()
            .map_err(|_| StoreError::RestoreActivationUncertain)?;
        Ok(())
    }
}
impl Drop for StagingDirectory {
    fn drop(&mut self) {
        // A moved stage is safer to retain than deleting an unrelated directory
        // that another process created at its old pathname.
        if !matches!(self.matches(), Ok(true)) {
            self.temporary.disable_cleanup(true);
        }
    }
}
fn same_directory(path: &Path, file: &File) -> Result<bool, StoreError> {
    let current = std::fs::symlink_metadata(path)?;
    let original = file.metadata()?;
    Ok(current.is_dir() && current.dev() == original.dev() && current.ino() == original.ino())
}
