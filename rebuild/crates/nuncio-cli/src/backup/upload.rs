use super::{file_error, AppError, LIMIT};
use nuncio_proto::v2::{self, backup_upload_chunk::Content};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Seek},
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tokio::io::AsyncReadExt;

pub(super) fn prepare(
    path: &Path,
    secret: v2::RecoverySecret,
    new_profile: Option<String>,
) -> Result<
    (
        impl futures_util::Stream<Item = v2::BackupUploadChunk> + Send + 'static,
        Arc<AtomicBool>,
    ),
    AppError,
> {
    let fd = rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::NONBLOCK
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(|_| file_error())?;
    let mut file = std::fs::File::from(fd);
    let metadata = file.metadata().map_err(|_| file_error())?;
    let length = metadata.len();
    if !metadata.is_file() || length == 0 || length > LIMIT {
        return Err(file_error());
    }
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut companion = path.as_os_str().to_os_string();
        companion.push(suffix);
        match std::fs::symlink_metadata(&companion) {
            Ok(_) => return Err(file_error()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(file_error()),
        }
    }
    let mut hash = Sha256::new();
    let mut remaining = length;
    let mut buffer = vec![0; 262144];
    while remaining > 0 {
        let n = remaining.min(buffer.len() as u64) as usize;
        file.read_exact(&mut buffer[..n])
            .map_err(|_| file_error())?;
        hash.update(&buffer[..n]);
        remaining -= n as u64;
    }
    if file.read(&mut buffer[..1]).map_err(|_| file_error())? != 0 {
        return Err(file_error());
    }
    file.rewind().map_err(|_| file_error())?;
    let header = v2::BackupUploadHeader {
        secret: Some(secret),
        byte_length: length,
        sha256: format!("{:x}", hash.finalize()),
        new_profile,
    };
    let failed = Arc::new(AtomicBool::new(false));
    let stream = futures_util::stream::unfold(
        (
            Some(header),
            tokio::fs::File::from_std(file),
            0_u64,
            failed.clone(),
        ),
        move |(header, mut file, offset, failed)| async move {
            if failed.load(Ordering::Relaxed) {
                return None;
            }
            if let Some(header) = header {
                return Some((
                    v2::BackupUploadChunk {
                        content: Some(Content::Header(header)),
                    },
                    (None, file, offset, failed),
                ));
            }
            let n = (length - offset).min(262144) as usize;
            let mut data = vec![0; n.max(1)];
            if n == 0 {
                if matches!(file.read(&mut data).await, Ok(0)) {
                    return None;
                }
            } else if file.read_exact(&mut data).await.is_ok() {
                return Some((
                    v2::BackupUploadChunk {
                        content: Some(Content::Data(v2::BackupData { offset, data })),
                    },
                    (None, file, offset + n as u64, failed),
                ));
            }
            failed.store(true, Ordering::Relaxed);
            Some((
                v2::BackupUploadChunk { content: None },
                (None, file, offset, failed),
            ))
        },
    );
    Ok((stream, failed))
}
