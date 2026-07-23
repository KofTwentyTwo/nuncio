//! Local storage, full-text search indexing, and credential security for Nuncio.

pub mod db;
pub mod search;

pub use db::{DatabaseEngine, DatabaseError};
pub use search::{SearchEngine, SearchHit};
