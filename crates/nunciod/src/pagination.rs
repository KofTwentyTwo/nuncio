//! Shared keyset ("cursor") pagination for the daemon's list/search RPCs.
//!
//! Every `nuncio.v1` list/search RPC pages results with the same convention
//! (AIP-158): a request carries `page_size` + `page_token` and a response
//! carries `next_page_token`. The `page_token` is an OPAQUE cursor, never a
//! numeric offset: it encodes the stable sort key (and id tiebreaker) of the
//! last row of the previous page so the next page resumes strictly AFTER that
//! row. Keyset paging is stable under concurrent inserts -- rows are never
//! skipped or returned twice as the underlying table changes between page
//! fetches, which offset paging cannot guarantee.
//!
//! The wire encoding is deliberately private to the daemon: it is a
//! hex-encoded JSON array of typed [`CursorField`]s. Clients must treat the
//! token as opaque and echo it back verbatim; a token this module cannot
//! decode is rejected with a typed [`ErrorReason::ValidationFailed`] error
//! (mapped to `InvalidArgument`), never silently ignored or treated as the
//! first page.

use nuncio_proto::errors;
use nuncio_proto::v1::ErrorReason;
use serde::{Deserialize, Serialize};
use tonic::Status;

/// Page size used when a request leaves `page_size` at 0 ("server default").
pub const DEFAULT_PAGE_SIZE: u32 = 50;

/// Hard upper bound on a single page, so a client cannot force the daemon to
/// materialize an unbounded result set in one call.
pub const MAX_PAGE_SIZE: u32 = 500;

/// Resolves a requested `page_size` into a concrete row count: 0 selects
/// [`DEFAULT_PAGE_SIZE`], and any value is clamped to [`MAX_PAGE_SIZE`].
#[must_use]
pub fn clamp_page_size(requested: u32) -> usize {
    let n = if requested == 0 {
        DEFAULT_PAGE_SIZE
    } else {
        requested.min(MAX_PAGE_SIZE)
    };
    n as usize
}

/// One typed component of an opaque keyset cursor. A cursor is an ordered
/// list of these, matching a list's stable sort key followed by its id
/// tiebreaker (e.g. `[I(received_at), S(id)]` for messages).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CursorField {
    /// A signed integer sort key (e.g. a unix timestamp or a priority).
    I(i64),
    /// An unsigned integer sort key (e.g. an audit sequence).
    U(u64),
    /// A floating-point sort key (e.g. an FTS5 relevance rank).
    F(f64),
    /// A textual sort key or id (e.g. a message id or display name).
    S(String),
}

/// Encodes a cursor's component list into an opaque `page_token`.
#[must_use]
pub fn encode_cursor(fields: &[CursorField]) -> String {
    // The component list is a fixed, small set of primitives, so
    // serialization cannot realistically fail; fall back to an empty payload
    // rather than panicking, keeping this crate free of `unwrap`/`expect`.
    let json = serde_json::to_vec(fields).unwrap_or_default();
    hex::encode(json)
}

fn malformed_token() -> Status {
    errors::status(
        ErrorReason::ValidationFailed,
        "malformed page_token: not a valid pagination cursor",
    )
}

/// Decodes an opaque `page_token` back into its component list, rejecting a
/// token that is not a hex-encoded cursor JSON payload with a typed
/// `VALIDATION_FAILED` error.
pub fn decode_cursor(token: &str) -> Result<Vec<CursorField>, Status> {
    let bytes = hex::decode(token).map_err(|_| malformed_token())?;
    serde_json::from_slice(&bytes).map_err(|_| malformed_token())
}

/// Decodes an optional `page_token` into a strongly-typed cursor value.
///
/// An empty token means "first page" and yields `Ok(None)`. A non-empty
/// token is decoded and handed to `shape`, which pattern-matches the expected
/// component layout for a specific list and returns `Some(value)`; a token
/// that decodes but does not match the expected layout (e.g. a cursor minted
/// for a different list) is rejected with the same typed `VALIDATION_FAILED`
/// error as an undecodable token, so a caller can never resume from a cursor
/// shaped for the wrong query.
pub fn parse_token<T>(
    token: &str,
    shape: impl FnOnce(&[CursorField]) -> Option<T>,
) -> Result<Option<T>, Status> {
    if token.is_empty() {
        return Ok(None);
    }
    let fields = decode_cursor(token)?;
    shape(&fields).map(Some).ok_or_else(malformed_token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tonic::Code;

    #[test]
    fn clamp_page_size_applies_default_and_max() {
        assert_eq!(clamp_page_size(0), DEFAULT_PAGE_SIZE as usize);
        assert_eq!(clamp_page_size(10), 10);
        assert_eq!(clamp_page_size(MAX_PAGE_SIZE + 1), MAX_PAGE_SIZE as usize);
        assert_eq!(clamp_page_size(u32::MAX), MAX_PAGE_SIZE as usize);
    }

    #[test]
    fn cursor_round_trips_through_encode_decode() {
        let fields = vec![
            CursorField::I(1_700_000_000),
            CursorField::S("msg-9".into()),
        ];
        let token = encode_cursor(&fields);
        assert!(!token.is_empty());
        assert_eq!(decode_cursor(&token).unwrap(), fields);
    }

    #[test]
    fn empty_token_parses_as_first_page() {
        let parsed = parse_token("", |f| match f {
            [CursorField::I(n)] => Some(*n),
            _ => None,
        })
        .unwrap();
        assert_eq!(parsed, None);
    }

    #[test]
    fn malformed_token_is_validation_failed() {
        // Not valid hex.
        let err = decode_cursor("zzz not-hex").unwrap_err();
        assert_eq!(err.code(), Code::InvalidArgument);
        assert_eq!(
            errors::error_reason(&err),
            Some(ErrorReason::ValidationFailed)
        );

        // Valid hex, but not a cursor JSON payload.
        let err = decode_cursor(&hex::encode(b"not json")).unwrap_err();
        assert_eq!(err.code(), Code::InvalidArgument);
    }

    #[test]
    fn wrong_shape_token_is_rejected() {
        // A cursor with a single int component handed to a shape expecting
        // (int, string) must be rejected, not silently accepted.
        let token = encode_cursor(&[CursorField::I(1)]);
        let err = parse_token(&token, |f| match f {
            [CursorField::I(n), CursorField::S(s)] => Some((*n, s.clone())),
            _ => None,
        })
        .unwrap_err();
        assert_eq!(err.code(), Code::InvalidArgument);
        assert_eq!(
            errors::error_reason(&err),
            Some(ErrorReason::ValidationFailed)
        );
    }
}
