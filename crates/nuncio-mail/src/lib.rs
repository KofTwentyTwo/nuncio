//! Protocol engines for IMAP4rev1, JMAP (RFC 8620/8621), and SMTP.

pub mod backend;
pub mod imap;
pub mod jmap;
pub mod parser;
pub mod smtp;

pub use backend::MailBackend;
pub use imap::{IdleSocketState, ImapDualSocketManager, ImapEngine};
pub use jmap::JmapEngine;
pub use parser::{MailError, MimeParserAdapter};
pub use smtp::SmtpTransportEngine;
