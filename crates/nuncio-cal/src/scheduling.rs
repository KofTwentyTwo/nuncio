//! Natural language 1-on-1 scheduling link and availability generator.

use serde::{Deserialize, Serialize};

/// 1-on-1 scheduling booking link descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchedulingLink {
    /// Host organizer email.
    pub host_email: String,
    /// Meeting title.
    pub title: String,
    /// Duration in minutes (e.g. 15, 30, 60).
    pub duration_mins: u32,
    /// Generated booking URL.
    pub booking_url: String,
}

/// Natural language scheduling generator.
pub struct SchedulingLinkGenerator;

impl SchedulingLinkGenerator {
    /// Generate a 1-on-1 natural language scheduling URL link.
    pub fn generate_1on1_link(host_email: &str, duration_mins: u32, title: &str) -> SchedulingLink {
        let clean_email = host_email.trim().to_lowercase();
        let slug_title = title.to_lowercase().replace(' ', "-");
        let booking_url = format!(
            "https://nuncio.mx/meet/{}/{}?dur={}",
            clean_email, slug_title, duration_mins
        );

        SchedulingLink {
            host_email: clean_email,
            title: title.to_string(),
            duration_mins,
            booking_url,
        }
    }

    /// Parse natural language duration string (e.g., "30m", "1h", "45 mins") into minutes.
    pub fn parse_duration_minutes(input: &str) -> u32 {
        let clean = input.trim().to_lowercase();
        if clean.ends_with('h') || clean.ends_with("hour") || clean.ends_with("hours") {
            let num: u32 = clean
                .trim_end_matches("hours")
                .trim_end_matches("hour")
                .trim_end_matches('h')
                .trim()
                .parse()
                .unwrap_or(1);
            num * 60
        } else {
            let num: u32 = clean
                .trim_end_matches("mins")
                .trim_end_matches("min")
                .trim_end_matches('m')
                .trim()
                .parse()
                .unwrap_or(30);
            num
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_1on1_link_format() {
        let link = SchedulingLinkGenerator::generate_1on1_link("james.maes@kof22.com", 30, "Architecture Review");
        assert_eq!(link.host_email, "james.maes@kof22.com");
        assert_eq!(link.duration_mins, 30);
        assert_eq!(link.booking_url, "https://nuncio.mx/meet/james.maes@kof22.com/architecture-review?dur=30");
    }

    #[test]
    fn parse_duration_minutes_variations() {
        assert_eq!(SchedulingLinkGenerator::parse_duration_minutes("30m"), 30);
        assert_eq!(SchedulingLinkGenerator::parse_duration_minutes("1h"), 60);
        assert_eq!(SchedulingLinkGenerator::parse_duration_minutes("45 mins"), 45);
        assert_eq!(SchedulingLinkGenerator::parse_duration_minutes("2 hours"), 120);
    }
}
