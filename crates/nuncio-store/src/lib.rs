//! Local storage, full-text search indexing, and credential security for Nuncio.

#![forbid(unsafe_code)]

pub mod cipher;
pub mod db;
pub mod search;
pub mod vault;

pub use cipher::{CipherError, PayloadCipher};
pub use db::{DatabaseEngine, DatabaseError};
pub use search::{SearchEngine, SearchHit};
pub use vault::{MockKeyring, SecretManager, SecretVault, VaultError};
