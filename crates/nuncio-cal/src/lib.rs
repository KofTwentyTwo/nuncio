//! iCalendar (RFC 5545), CalDAV (RFC 4791), and recurrence engine for Nuncio.

pub mod parser;
pub mod rrule;

pub use parser::{CalendarError, IcalParserAdapter};
pub use rrule::RecurrenceEngine;
