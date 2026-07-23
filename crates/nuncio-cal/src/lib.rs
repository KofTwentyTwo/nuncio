//! iCalendar (RFC 5545), CalDAV (RFC 4791), and recurrence engine for Nuncio.

#![forbid(unsafe_code)]

pub mod caldav;
pub mod carddav;
pub mod mock;
pub mod nlp;
pub mod parser;
pub mod rrule;
pub mod scheduling;

pub use caldav::CalDavClient;
pub use carddav::{CardDavClient, Contact};
pub use mock::MockCalendarBackend;
pub use nlp::{NaturalLanguageScheduler, NlpError, ParsedSchedulingIntent};
pub use parser::{CalendarError, IcalParserAdapter};
pub use rrule::RecurrenceEngine;
pub use scheduling::{SchedulingLink, SchedulingLinkGenerator};
