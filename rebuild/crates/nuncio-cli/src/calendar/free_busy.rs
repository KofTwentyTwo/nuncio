use crate::output::AppError;
use nuncio_proto::v2::FreeBusyRequest;
use serde::Deserialize;
use std::{io::Read, path::Path};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Query {
    schema_version: u32,
    from: String,
    to: String,
    time_zone: String,
    provider_calendar_ids: Vec<String>,
}
pub(super) fn read(path: &Path) -> Result<FreeBusyRequest, AppError> {
    let file = std::fs::File::open(path).map_err(|_| AppError::invalid())?;
    let meta = file.metadata().map_err(|_| AppError::invalid())?;
    const MAX: u64 = 128 * 1024;
    if !meta.is_file() || meta.len() > MAX {
        return Err(AppError::invalid());
    }
    let mut bytes = Vec::new();
    file.take(MAX + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| AppError::invalid())?;
    if bytes.len() > MAX as usize {
        return Err(AppError::invalid());
    }
    let q: Query = serde_json::from_slice(&bytes).map_err(|_| AppError::invalid())?;
    if q.schema_version != 1 {
        return Err(AppError::invalid());
    }
    Ok(FreeBusyRequest {
        account_id: String::new(),
        from: q.from,
        to: q.to,
        time_zone: q.time_zone,
        provider_calendar_ids: q.provider_calendar_ids,
    })
}
