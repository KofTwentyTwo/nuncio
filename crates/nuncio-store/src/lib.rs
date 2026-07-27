//! Local storage, full-text search indexing, and credential security for Nuncio.

#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod cipher;
pub mod db;
pub mod recovery;
pub mod search;
pub mod vault;

pub use cipher::{CipherError, PayloadCipher};
pub use db::{DatabaseEngine, DatabaseError, WormChainReport};
pub use recovery::{
    is_sqlite_corruption_error, CorruptedBackupManager, RecoverySummary, SqliteRecoveryEngine,
};
pub use search::{SearchEngine, SearchHit};
pub use vault::{
    MockKeyring, OsKeyring, SecretManager, SecretVault, VaultError, KEYRING_SERVICE,
    LEDGER_KEY_ACCOUNT, STORAGE_KEY_ACCOUNT, WORM_KEY_ACCOUNT,
};
