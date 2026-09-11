use super::StoreError;
use std::{
    fs::File,
    io::{Read, Write},
    path::Path,
};

pub(crate) const MAX_BACKUP_BYTES: u64 = 1 << 40;
const SPACE_MARGIN: u64 = 8 * 1024 * 1024;

fn valid_size(length: u64) -> Result<(), StoreError> {
    if length == 0 {
        return Err(StoreError::InvalidInput);
    }
    if length > MAX_BACKUP_BYTES {
        return Err(StoreError::ResultTooLarge);
    }
    Ok(())
}
fn check_space(length: u64, copies: u64, available: u64) -> Result<(), StoreError> {
    valid_size(length)?;
    let needed = length
        .checked_mul(copies)
        .and_then(|n| n.checked_add(SPACE_MARGIN))
        .ok_or(StoreError::ResultTooLarge)?;
    if available < needed {
        return Err(StoreError::Io(std::io::ErrorKind::StorageFull));
    }
    Ok(())
}
// This is a conservative preflight, not a reservation against other writers.
// Every copy/export still propagates actual write/fsync errors before activation.
pub(crate) fn preflight(parent: &Path, length: u64, copies: u64) -> Result<(), StoreError> {
    valid_size(length)?;
    let directory = File::open(parent)?;
    let space = rustix::fs::fstatvfs(&directory).map_err(std::io::Error::from)?;
    let available = space
        .f_bavail
        .checked_mul(space.f_frsize)
        .ok_or(StoreError::ResultTooLarge)?;
    check_space(length, copies, available)
}
pub(super) fn copy_bounded(
    reader: &mut impl Read,
    writer: &mut impl Write,
    length: u64,
) -> Result<(), StoreError> {
    valid_size(length)?;
    let copied = std::io::copy(&mut (&mut *reader).take(length), writer)?;
    if copied != length || reader.read(&mut [0u8; 1])? != 0 {
        return Err(StoreError::InvalidInput);
    }
    Ok(())
}

// SQLCipher returns its page-size pragmas as text, including the ordinary
// page_size spelling. Parse that documented codec value explicitly.
pub(super) fn page_size(c: &rusqlite::Connection, schema: Option<&str>) -> Result<u64, StoreError> {
    let text: String = c.pragma_query_value(schema, "cipher_page_size", |r| r.get(0))?;
    let size: u64 = text.parse().map_err(|_| StoreError::CipherUnavailable)?;
    if !(512..=65536).contains(&size) || !size.is_power_of_two() {
        return Err(StoreError::CipherUnavailable);
    }
    Ok(size)
}
pub(super) fn limit_output(
    c: &rusqlite::Connection,
    schema: &str,
    bytes: u64,
) -> Result<(), StoreError> {
    valid_size(bytes)?;
    let pages = bytes / page_size(c, Some(schema))?;
    if pages == 0 {
        return Err(StoreError::ResultTooLarge);
    }
    let pages = i64::try_from(pages).map_err(|_| StoreError::ResultTooLarge)?;
    let effective: i64 =
        c.pragma_update_and_check(Some(schema), "max_page_count", pages, |r| r.get(0))?;
    if effective != pages {
        return Err(StoreError::ResultTooLarge);
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    #[test]
    fn stable_copy_rejects_short_growing_and_oversized_sources_with_bounded_reads() {
        let mut output = Vec::new();
        copy_bounded(&mut &b"binary\0payload"[..], &mut output, 14).unwrap();
        assert_eq!(output, b"binary\0payload");
        for data in [&b"short"[..], &b"extra bytes"[..]] {
            assert!(matches!(
                copy_bounded(&mut &data[..], &mut Vec::new(), 6),
                Err(super::super::StoreError::InvalidInput)
            ));
        }
        struct Endless(u64);
        impl Read for Endless {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                buffer.fill(42);
                self.0 += buffer.len() as u64;
                Ok(buffer.len())
            }
        }
        let mut source = Endless(0);
        let mut output = Vec::new();
        assert!(matches!(
            copy_bounded(&mut source, &mut output, 5000),
            Err(super::super::StoreError::InvalidInput)
        ));
        assert_eq!(source.0, 5001);
        assert_eq!(output.len(), 5000);
        let mut source = Endless(0);
        assert!(matches!(
            copy_bounded(&mut source, &mut Vec::new(), MAX_BACKUP_BYTES + 1),
            Err(super::super::StoreError::ResultTooLarge)
        ));
        assert_eq!(source.0, 0);
    }
    #[test]
    fn disk_full_is_reported_and_space_estimates_reject_exhaustion_and_overflow() {
        struct Full;
        impl Write for Full {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::StorageFull.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        assert!(matches!(
            copy_bounded(&mut &b"data"[..], &mut Full, 4),
            Err(super::super::StoreError::Io(
                std::io::ErrorKind::StorageFull
            ))
        ));
        assert!(check_space(1024, 3, 8 * 1024 * 1024 + 3072).is_ok());
        assert!(matches!(
            check_space(1024, 3, 8 * 1024 * 1024 + 3071),
            Err(super::super::StoreError::Io(
                std::io::ErrorKind::StorageFull
            ))
        ));
        assert!(check_space(MAX_BACKUP_BYTES, u64::MAX, u64::MAX).is_err());
        assert!(check_space(0, 1, u64::MAX).is_err());
        assert!(check_space(MAX_BACKUP_BYTES + 1, 1, u64::MAX).is_err());
    }
    #[test]
    fn actual_sqlcipher_export_honors_page_limit_without_changing_source() {
        let temporary = tempfile::tempdir().unwrap();
        let mut c = rusqlite::Connection::open(temporary.path().join("source.db")).unwrap();
        c.pragma_update(None, "key", "synthetic bounds source key")
            .unwrap();
        c.execute_batch("CREATE TABLE original(payload BLOB NOT NULL); INSERT INTO original VALUES(zeroblob(200000));").unwrap();
        let before: u64 = std::fs::metadata(temporary.path().join("source.db"))
            .unwrap()
            .len();
        c.execute(
            "ATTACH DATABASE ?1 AS limited KEY ?2",
            rusqlite::params![
                temporary.path().join("limited.db").to_str().unwrap(),
                "synthetic bounds export key"
            ],
        )
        .unwrap();
        limit_output(&c, "limited", 8192).unwrap();
        let tx = c.transaction().unwrap();
        let failed = tx.query_row("SELECT sqlcipher_export('limited')", [], |_| Ok(()));
        // sqlcipher_export wraps the underlying SQLITE_FULL text using
        // sqlite3_result_error, which exposes SQLITE_ERROR at the API boundary.
        assert!(
            matches!(failed.unwrap_err(),rusqlite::Error::SqliteFailure(code,Some(message))
            if code.extended_code == 1 && message == "database or disk is full")
        );
        drop(tx);
        c.execute_batch("DETACH DATABASE limited").unwrap();
        let bytes: Vec<u8> = c
            .query_row("SELECT payload FROM original", [], |r| r.get(0))
            .unwrap();
        assert_eq!(bytes, vec![0; 200000]);
        assert_eq!(
            std::fs::metadata(temporary.path().join("source.db"))
                .unwrap()
                .len(),
            before
        );
        assert!(
            std::fs::metadata(temporary.path().join("limited.db"))
                .unwrap()
                .len()
                <= 8192
        );
    }
}
