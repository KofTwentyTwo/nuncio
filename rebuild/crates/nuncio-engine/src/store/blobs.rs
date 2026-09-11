use super::{Store, StoreError};
use crate::domain::mail::{BLOB_CHUNK_BYTES, MAX_PAYLOAD_BYTES};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};

#[derive(Clone, Serialize)]
pub struct StoredBlob {
    pub id: String,
    pub byte_length: u64,
    pub sha256: String,
}
impl Store {
    pub async fn put_blob(
        &self,
        account: String,
        bytes: Vec<u8>,
    ) -> Result<StoredBlob, StoreError> {
        if bytes.len() > MAX_PAYLOAD_BYTES {
            return Err(StoreError::InvalidInput);
        }
        self.execute(move |connection| {
            let transaction = connection.transaction()?;
            let blob = insert(&transaction, &account, &bytes)?;
            transaction.commit()?;
            Ok(blob)
        })
        .await
    }
    pub async fn blob_chunk(
        &self,
        account: String,
        id: String,
        ordinal: u64,
    ) -> Result<Vec<u8>, StoreError> {
        self.execute(move|connection|{
            let total=connection.query_row("SELECT byte_length FROM blobs WHERE account_id=?1 AND id=?2",
                params![account,id],|row|row.get::<_,u32>(0)).optional()?.ok_or(StoreError::NotFound)?;
            let total=u64::from(total);
            let offset=ordinal.checked_mul(BLOB_CHUNK_BYTES as u64).ok_or(StoreError::InvalidInput)?;
            if offset>=total {return Err(StoreError::NotFound);}
            let ordinal=i64::try_from(ordinal).map_err(|_|StoreError::InvalidInput)?;
            let bytes:Vec<u8>=connection.query_row("SELECT data FROM blob_chunks WHERE account_id=?1 AND blob_id=?2 AND ordinal=?3",
                params![account,id,ordinal],|row|row.get(0)).optional()?.ok_or(StoreError::KeyOrCorrupt)?;
            if bytes.len() as u64!=(total-offset).min(BLOB_CHUNK_BYTES as u64) {return Err(StoreError::KeyOrCorrupt);}
            Ok(bytes)
        }).await
    }
}

/// The caller owns the encompassing transaction, including all references.
pub(super) fn insert(
    connection: &Connection,
    account: &str,
    bytes: &[u8],
) -> Result<StoredBlob, StoreError> {
    if bytes.len() > MAX_PAYLOAD_BYTES {
        return Err(StoreError::InvalidInput);
    }
    let hash = hex::encode(Sha256::digest(bytes));
    if let Some(blob) = connection
        .query_row(
            "SELECT id,byte_length,sha256 FROM blobs WHERE account_id=?1 AND sha256=?2",
            params![account, hash],
            |row| {
                Ok(StoredBlob {
                    id: row.get(0)?,
                    byte_length: u64::from(row.get::<_, u32>(1)?),
                    sha256: row.get(2)?,
                })
            },
        )
        .optional()?
    {
        if blob.byte_length != bytes.len() as u64 {
            return Err(StoreError::KeyOrCorrupt);
        }
        return Ok(blob);
    }
    let blob = StoredBlob {
        id: uuid::Uuid::new_v4().to_string(),
        byte_length: bytes.len() as u64,
        sha256: hash,
    };
    let length = i64::try_from(bytes.len()).map_err(|_| StoreError::InvalidInput)?;
    connection.execute(
        "INSERT INTO blobs(account_id,id,sha256,byte_length) VALUES (?1,?2,?3,?4)",
        params![account, blob.id, blob.sha256, length],
    )?;
    let mut statement = connection
        .prepare("INSERT INTO blob_chunks(account_id,blob_id,ordinal,data) VALUES (?1,?2,?3,?4)")?;
    for (ordinal, chunk) in bytes.chunks(BLOB_CHUNK_BYTES).enumerate() {
        let ordinal = i64::try_from(ordinal).map_err(|_| StoreError::InvalidInput)?;
        statement.execute(params![account, blob.id, ordinal, chunk])?;
    }
    Ok(blob)
}
