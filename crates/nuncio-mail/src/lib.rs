//! Protocol engines for IMAP4rev1, JMAP (RFC 8620/8621), and SMTP.

#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod backend;
pub mod imap;
pub mod imap_raw;
pub mod jmap;
pub mod mock;
pub mod parser;
pub mod smtp;

// A real loopback IMAP server for offline tests. Behind a feature so it never
// enters a production build, but visible to this crate's own tests and to
// dependents that opt in (integration suites in `nunciod`).
#[cfg(any(test, feature = "test-support"))]
pub mod test_server;

pub use backend::{
    FolderChanges, MailBackend, MessageSender, MutationOutcome, OutboundMessage, PlacedMessage,
    RemoteMutationKind, RemoteMutationSpec,
};
pub use imap::{IdleSocketState, ImapDualSocketManager, ImapEngine};
pub use jmap::{EmailChangesPage, JmapEngine};
pub use mock::{MockMailBackend, MockMessageSender};
pub use parser::{MailError, MimeParserAdapter};
pub use smtp::SmtpTransportEngine;
