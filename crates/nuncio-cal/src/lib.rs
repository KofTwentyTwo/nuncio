//! iCalendar (RFC 5545), CalDAV (RFC 4791), and recurrence engine for Nuncio.

#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod backend;
pub mod caldav;
pub mod ical;
pub mod mock;
pub mod nlp;
pub mod parser;
pub mod rrule;
pub mod scheduling;

pub use backend::CalendarBackend;
pub use caldav::{CalDavAccountConfig, CalDavClient};
pub use ical::ToIcal;
pub use mock::MockCalendarBackend;
pub use nlp::{NaturalLanguageScheduler, NlpError, ParsedSchedulingIntent};
pub use parser::{CalendarError, IcalParserAdapter};
pub use rrule::RecurrenceEngine;
pub use scheduling::{SchedulingLink, SchedulingLinkGenerator};
