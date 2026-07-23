//! Protocol engines for IMAP4rev1, JMAP (RFC 8620/8621), and SMTP.

pub mod parser;

pub use parser::{MailError, MimeParserAdapter};
