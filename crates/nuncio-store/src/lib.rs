//! Local storage, full-text search indexing, and credential security for Nuncio.

pub mod db;

pub use db::{DatabaseEngine, DatabaseError};
