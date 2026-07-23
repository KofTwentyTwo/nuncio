//! iCalendar (RFC 5545), CalDAV (RFC 4791), and recurrence engine for Nuncio.

#![forbid(unsafe_code)]

pub mod caldav;
pub mod mock;
pub mod parser;
pub mod rrule;

pub use caldav::CalDavClient;
pub use mock::MockCalendarBackend;
pub use parser::{CalendarError, IcalParserAdapter};
pub use rrule::RecurrenceEngine;
