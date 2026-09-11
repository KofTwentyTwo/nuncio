use std::path::{Path, PathBuf};

pub(crate) enum OutputPathError {
    Destination,
    Protection,
}

pub(crate) fn destination(path: &Path, protected: &[String]) -> Result<PathBuf, OutputPathError> {
    // All existing destinations are refused, including hard links and dangling
    // symlinks. Canonical parents also protect files (such as WAL) not yet present.
    match std::fs::symlink_metadata(path) {
        Ok(_) => return Err(OutputPathError::Destination),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(OutputPathError::Destination),
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = std::fs::canonicalize(parent).map_err(|_| OutputPathError::Destination)?;
    let output = parent.join(path.file_name().ok_or(OutputPathError::Destination)?);
    for path in protected {
        let path = Path::new(path);
        let name = path.file_name().ok_or(OutputPathError::Protection)?;
        let parent = path.parent().ok_or(OutputPathError::Protection)?;
        let canonical = std::fs::canonicalize(parent)
            .map_err(|_| OutputPathError::Protection)?
            .join(name);
        if canonical == output {
            return Err(OutputPathError::Destination);
        }
    }
    Ok(output)
}
