use crate::{
    drafts::{file_error, regular_file},
    output::AppError,
};
use nuncio_proto::v2::{
    draft_upload_chunk::Part, DraftUploadChunk, DraftUploadData, DraftUploadHeader,
};
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

const CHUNK: usize = 256 * 1024;
const LIMIT: u64 = 64 * 1024 * 1024;

pub(crate) fn prepare(
    path: &Path,
    mut header: DraftUploadHeader,
) -> Result<
    (
        impl futures_util::Stream<Item = DraftUploadChunk> + Send + 'static,
        Arc<AtomicBool>,
    ),
    AppError,
> {
    let mut file = regular_file(path)?;
    let length = file.metadata().map_err(|_| file_error())?.len();
    if length > LIMIT {
        return Err(AppError::invalid());
    }
    let mut remaining = length;
    let mut digest = Sha256::new();
    let mut buffer = vec![0; CHUNK];
    while remaining > 0 {
        let count = remaining.min(CHUNK as u64) as usize;
        file.read_exact(&mut buffer[..count])
            .map_err(|_| file_error())?;
        digest.update(&buffer[..count]);
        remaining -= count as u64;
    }
    if file.read(&mut buffer[..1]).map_err(|_| file_error())? != 0 {
        return Err(file_error());
    }
    file.rewind().map_err(|_| file_error())?;
    header.byte_length = length;
    header.sha256 = hex::encode(digest.finalize());
    let failed = Arc::new(AtomicBool::new(false));
    let state = (
        Some(header),
        tokio::fs::File::from_std(file),
        0_u64,
        failed.clone(),
    );
    // Pull-based reads apply HTTP/2 backpressure without an unowned producer.
    // A changed or unreadable file sends an invalid frame, so EOF cannot commit
    // a successfully read prefix as a complete attachment.
    let stream = futures_util::stream::unfold(
        state,
        move |(header, mut file, offset, failed)| async move {
            if failed.load(Ordering::Relaxed) {
                return None;
            }
            if let Some(header) = header {
                return Some((
                    DraftUploadChunk {
                        part: Some(Part::Header(header)),
                    },
                    (None, file, offset, failed),
                ));
            }
            let count = (length - offset).min(CHUNK as u64) as usize;
            let mut data = vec![0; count.max(1)];
            if count == 0 {
                if matches!(file.read(&mut data).await, Ok(0)) {
                    return None;
                }
            } else if file.read_exact(&mut data[..count]).await.is_ok() {
                return Some((
                    DraftUploadChunk {
                        part: Some(Part::Data(DraftUploadData { offset, data })),
                    },
                    (None, file, offset + count as u64, failed),
                ));
            }
            failed.store(true, Ordering::Relaxed);
            Some((
                DraftUploadChunk { part: None },
                (None, file, offset, failed),
            ))
        },
    );
    Ok((stream, failed))
}
