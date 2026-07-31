//! Typed, machine-readable gRPC errors for the `nuncio.v1` contract.
//!
//! A failed RPC should carry a structured [`v1::ErrorInfo`] -- a stable
//! [`v1::ErrorReason`] discriminant, optional metadata, and a human message
//! -- in the gRPC `grpc-status-details-bin` status-details trailer, alongside
//! the canonical gRPC [`Code`] that the reason maps to. Servers build such a
//! `Status` with [`status`] / [`status_with_metadata`]; clients read the
//! reason back with [`error_reason`] / [`error_info`] and branch on it,
//! rather than string-matching the human-readable (English) message text.
//!
//! The reason-to-code mapping is centralized in [`ErrorReason::code`] so the
//! transport-level status code and the application-level reason can never
//! drift apart at a call site.

use crate::v1::{ErrorInfo, ErrorReason};
use bytes::Bytes;
use prost::Message;
use tonic::{Code, Status};

impl ErrorReason {
    /// The canonical gRPC [`Code`] this reason maps to. Both the unspecified
    /// zero value and an explicit internal error map to [`Code::Internal`],
    /// so an older client that receives a reason it does not recognize (and
    /// decodes it as `Unspecified`) still sees an internal-class failure.
    #[must_use]
    pub fn code(self) -> Code {
        match self {
            ErrorReason::Unspecified | ErrorReason::Internal => Code::Internal,
            ErrorReason::ValidationFailed => Code::InvalidArgument,
            ErrorReason::AccountNotFound
            | ErrorReason::RuleNotFound
            | ErrorReason::MessageNotFound
            | ErrorReason::EventNotFound
            | ErrorReason::ContactNotFound => Code::NotFound,
            ErrorReason::AlreadyExists => Code::AlreadyExists,
            ErrorReason::NotConfigured => Code::FailedPrecondition,
            ErrorReason::BackendUnreachable => Code::Unavailable,
            ErrorReason::AuthRequired => Code::Unauthenticated,
            ErrorReason::Unsupported => Code::Unimplemented,
        }
    }
}

/// Builds a [`Status`] for `reason` carrying `message` both as the `Status`
/// message and inside a prost-encoded [`v1::ErrorInfo`] in the
/// `grpc-status-details-bin` trailer, with the canonical code from
/// [`ErrorReason::code`].
pub fn status(reason: ErrorReason, message: impl Into<String>) -> Status {
    build(reason, message.into(), Vec::new())
}

/// Like [`status`], but also attaches structured `metadata` (e.g. the id of
/// the resource that was not found) to the encoded [`v1::ErrorInfo`].
pub fn status_with_metadata(
    reason: ErrorReason,
    message: impl Into<String>,
    metadata: impl IntoIterator<Item = (String, String)>,
) -> Status {
    build(reason, message.into(), metadata.into_iter().collect())
}

fn build(reason: ErrorReason, message: String, metadata: Vec<(String, String)>) -> Status {
    let info = ErrorInfo {
        reason: reason as i32,
        metadata: metadata.into_iter().collect(),
        message: message.clone(),
    };
    Status::with_details(reason.code(), message, Bytes::from(info.encode_to_vec()))
}

/// Decodes the [`v1::ErrorInfo`] carried in `status`'s details trailer, if
/// one is present and well-formed. Returns `None` when the status carries no
/// details (e.g. an error not built through this module) or the details are
/// not a valid `ErrorInfo`.
#[must_use]
pub fn error_info(status: &Status) -> Option<ErrorInfo> {
    let details = status.details();
    if details.is_empty() {
        return None;
    }
    ErrorInfo::decode(details).ok()
}

/// Decodes just the [`v1::ErrorReason`] from `status`'s details trailer,
/// returning `None` when no typed [`v1::ErrorInfo`] is present.
#[must_use]
pub fn error_reason(status: &Status) -> Option<ErrorReason> {
    error_info(status).map(|info| info.reason())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_reason_maps_to_its_canonical_code() {
        assert_eq!(ErrorReason::Unspecified.code(), Code::Internal);
        assert_eq!(ErrorReason::ValidationFailed.code(), Code::InvalidArgument);
        assert_eq!(ErrorReason::AccountNotFound.code(), Code::NotFound);
        assert_eq!(ErrorReason::RuleNotFound.code(), Code::NotFound);
        assert_eq!(ErrorReason::MessageNotFound.code(), Code::NotFound);
        assert_eq!(ErrorReason::EventNotFound.code(), Code::NotFound);
        assert_eq!(ErrorReason::ContactNotFound.code(), Code::NotFound);
        assert_eq!(ErrorReason::AlreadyExists.code(), Code::AlreadyExists);
        assert_eq!(ErrorReason::NotConfigured.code(), Code::FailedPrecondition);
        assert_eq!(ErrorReason::BackendUnreachable.code(), Code::Unavailable);
        assert_eq!(ErrorReason::AuthRequired.code(), Code::Unauthenticated);
        assert_eq!(ErrorReason::Unsupported.code(), Code::Unimplemented);
        assert_eq!(ErrorReason::Internal.code(), Code::Internal);
    }

    #[test]
    fn status_carries_reason_code_and_decodable_details() {
        let status = status(ErrorReason::MessageNotFound, "message 'm-1' not found");
        assert_eq!(status.code(), Code::NotFound);
        assert_eq!(status.message(), "message 'm-1' not found");

        let info = error_info(&status).expect("typed details present");
        assert_eq!(info.reason(), ErrorReason::MessageNotFound);
        assert_eq!(info.message, "message 'm-1' not found");
        assert!(info.metadata.is_empty());

        assert_eq!(
            error_reason(&status),
            Some(ErrorReason::MessageNotFound),
            "the reason round-trips through the encoded details"
        );
    }

    #[test]
    fn status_with_metadata_round_trips_context() {
        let status = status_with_metadata(
            ErrorReason::AccountNotFound,
            "account 'acct-1' not found",
            [("account_id".to_string(), "acct-1".to_string())],
        );
        assert_eq!(status.code(), Code::NotFound);

        let info = error_info(&status).expect("typed details present");
        assert_eq!(info.reason(), ErrorReason::AccountNotFound);
        assert_eq!(
            info.metadata.get("account_id").map(String::as_str),
            Some("acct-1")
        );
    }

    #[test]
    fn plain_status_without_details_decodes_to_none() {
        let status = Status::not_found("no typed details here");
        assert_eq!(error_info(&status), None);
        assert_eq!(error_reason(&status), None);
    }
}
