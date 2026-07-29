//! vCard (RFC 6350) parser adapter converting raw vCard payloads into Nuncio [`Contact`]
//! domain entities -- the read half of the round trip whose write half is
//! [`Contact::to_vcard`].

use chrono::Utc;

use crate::carddav::CardDavError;
use crate::models::{Contact, ContactEmail, ContactPhone};

/// vCard parser adapter converting raw `.vcf`/CardDAV `address-data` payloads into Nuncio
/// [`Contact`] domain entities.
pub struct VCardParserAdapter;

impl VCardParserAdapter {
    /// Maximum allowed single-vCard payload size (1MB -- generous for a single contact
    /// record, small enough to reject a runaway/malicious response body outright).
    pub const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;

    /// Parse a raw RFC 6350 vCard string into a [`Contact`] entity.
    ///
    /// `id` is assigned by the caller (CardDAV responses carry no stable contact ID of their
    /// own outside the enclosing WebDAV `href`); `account_id` tags which Nuncio account this
    /// contact was fetched for. Never fabricates a placeholder name or email: a vCard with
    /// neither `FN` nor any `EMAIL` is a parse failure, not an empty/default contact.
    pub fn parse_vcard(
        id: &str,
        account_id: &str,
        raw_vcard: &str,
    ) -> Result<Contact, CardDavError> {
        if raw_vcard.len() > Self::MAX_PAYLOAD_BYTES {
            return Err(CardDavError::ParseFailed(
                "vCard payload exceeds maximum allowed limit of 1MB".to_string(),
            ));
        }
        if !raw_vcard.to_ascii_uppercase().contains("BEGIN:VCARD") {
            return Err(CardDavError::ParseFailed(
                "vCard payload is missing BEGIN:VCARD".to_string(),
            ));
        }

        let unfolded = unfold_lines(raw_vcard);

        let mut display_name: Option<String> = None;
        let mut given_name: Option<String> = None;
        let mut family_name: Option<String> = None;
        let mut organization: Option<String> = None;
        let mut job_title: Option<String> = None;
        let mut emails: Vec<ContactEmail> = Vec::new();
        let mut phones: Vec<ContactPhone> = Vec::new();

        for line in unfolded.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let upper = line.to_ascii_uppercase();
            if upper == "BEGIN:VCARD" || upper == "END:VCARD" || upper.starts_with("VERSION:") {
                continue;
            }

            let Some((prop, value)) = line.split_once(':') else {
                continue;
            };
            let mut segments = prop.split(';');
            let name = segments.next().unwrap_or("").to_ascii_uppercase();
            let params: Vec<&str> = segments.collect();

            match name.as_str() {
                "FN" => display_name = Some(value.trim().to_string()),
                "N" => {
                    let comps: Vec<&str> = value.split(';').collect();
                    if let Some(family) = comps.first().map(|s| s.trim()).filter(|s| !s.is_empty())
                    {
                        family_name = Some(family.to_string());
                    }
                    if let Some(given) = comps.get(1).map(|s| s.trim()).filter(|s| !s.is_empty()) {
                        given_name = Some(given.to_string());
                    }
                }
                "ORG" => organization = Some(value.trim().to_string()),
                "TITLE" => job_title = Some(value.trim().to_string()),
                "EMAIL" => {
                    let label = extract_type(&params).unwrap_or_else(|| "other".to_string());
                    let is_primary = emails.is_empty();
                    emails.push(ContactEmail {
                        email: value.trim().to_string(),
                        label: label.to_ascii_lowercase(),
                        is_primary,
                    });
                }
                "TEL" => {
                    let label = extract_type(&params).unwrap_or_else(|| "other".to_string());
                    let is_primary = phones.is_empty();
                    phones.push(ContactPhone {
                        phone: value.trim().to_string(),
                        label: label.to_ascii_lowercase(),
                        is_primary,
                    });
                }
                _ => {}
            }
        }

        let display_name = display_name
            .or_else(|| emails.first().map(|e| e.email.clone()))
            .ok_or_else(|| {
                CardDavError::ParseFailed(
                    "vCard has neither FN nor EMAIL to derive a display name".to_string(),
                )
            })?;

        let now = Utc::now();
        Ok(Contact {
            id: id.to_string(),
            account_id: Some(account_id.to_string()),
            display_name,
            given_name,
            family_name,
            organization,
            job_title,
            notes: None,
            avatar_url: None,
            emails,
            phones,
            is_favorite: false,
            interaction_count: 0,
            last_interacted_at: None,
            created_at: now,
            updated_at: now,
        })
    }
}

/// Extract a `TYPE=...` parameter value from a vCard property's parameter list.
fn extract_type(params: &[&str]) -> Option<String> {
    params.iter().find_map(|param| {
        param
            .to_ascii_uppercase()
            .strip_prefix("TYPE=")
            .map(|v| v.to_string())
    })
}

/// Unfold RFC 6350 line continuations (a line starting with a space/tab is a continuation of
/// the previous line) into single logical lines, tolerating both CRLF and bare LF input.
fn unfold_lines(raw: &str) -> String {
    let mut result = String::new();
    for line in raw.split("\r\n").flat_map(|l| l.split('\n')) {
        if (line.starts_with(' ') || line.starts_with('\t')) && !result.is_empty() {
            result.push_str(line.trim_start());
        } else {
            if !result.is_empty() {
                result.push('\n');
            }
            result.push_str(line);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_vcard_with_all_supported_fields() {
        let vcard = "BEGIN:VCARD\r\n\
                     VERSION:4.0\r\n\
                     FN:James Maes\r\n\
                     N:Maes;James;;;\r\n\
                     ORG:KofTwentyTwo\r\n\
                     TITLE:Engineer\r\n\
                     EMAIL;TYPE=WORK:james.maes@kof22.com\r\n\
                     TEL;TYPE=MOBILE:+1-555-0199\r\n\
                     END:VCARD";

        let contact =
            VCardParserAdapter::parse_vcard("c-1", "acct-1", vcard).expect("parse succeeds");

        assert_eq!(contact.display_name, "James Maes");
        assert_eq!(contact.given_name.as_deref(), Some("James"));
        assert_eq!(contact.family_name.as_deref(), Some("Maes"));
        assert_eq!(contact.organization.as_deref(), Some("KofTwentyTwo"));
        assert_eq!(contact.job_title.as_deref(), Some("Engineer"));
        assert_eq!(contact.emails.len(), 1);
        assert_eq!(contact.emails[0].email, "james.maes@kof22.com");
        assert_eq!(contact.emails[0].label, "work");
        assert!(contact.emails[0].is_primary);
        assert_eq!(contact.phones.len(), 1);
        assert_eq!(contact.phones[0].phone, "+1-555-0199");
        assert_eq!(contact.phones[0].label, "mobile");
    }

    #[test]
    fn parse_vcard_without_fn_falls_back_to_primary_email_as_display_name() {
        let vcard = "BEGIN:VCARD\r\nVERSION:4.0\r\nEMAIL:alice@nuncio.mx\r\nEND:VCARD";
        let contact =
            VCardParserAdapter::parse_vcard("c-2", "acct-1", vcard).expect("parse succeeds");
        assert_eq!(contact.display_name, "alice@nuncio.mx");
    }

    #[test]
    fn parse_vcard_missing_begin_marker_is_a_real_error() {
        let err = VCardParserAdapter::parse_vcard("c-3", "acct-1", "FN:No Envelope")
            .expect_err("must reject a payload with no BEGIN:VCARD");
        assert!(matches!(err, CardDavError::ParseFailed(_)));
    }

    #[test]
    fn parse_vcard_with_no_name_or_email_is_a_real_error_not_a_fabricated_contact() {
        let vcard = "BEGIN:VCARD\r\nVERSION:4.0\r\nORG:Ghost Corp\r\nEND:VCARD";
        let err = VCardParserAdapter::parse_vcard("c-4", "acct-1", vcard)
            .expect_err("a vCard with no FN and no EMAIL must never become a fabricated contact");
        assert!(matches!(err, CardDavError::ParseFailed(_)));
    }

    /// Proves the parser is the true inverse of [`Contact::to_vcard`]: generating a vCard from
    /// a rich domain [`Contact`] and parsing it back must reproduce the same
    /// FN/N/ORG/TITLE/primary-EMAIL/primary-TEL fields.
    #[test]
    fn vcard_round_trip_preserves_core_fields() {
        let mut original = Contact::new("Alice Developer", "alice@kof22.com");
        original.given_name = Some("Alice".to_string());
        original.family_name = Some("Developer".to_string());
        original.organization = Some("KofTwentyTwo".to_string());
        original.job_title = Some("Staff Engineer".to_string());
        original.phones.push(ContactPhone {
            phone: "+1-555-0100".to_string(),
            label: "mobile".to_string(),
            is_primary: true,
        });

        let vcard = original.to_vcard();
        let round_tripped = VCardParserAdapter::parse_vcard("c-rt-1", "acct-1", &vcard)
            .expect("round-trip parse succeeds");

        assert_eq!(round_tripped.display_name, original.display_name);
        assert_eq!(round_tripped.given_name, original.given_name);
        assert_eq!(round_tripped.family_name, original.family_name);
        assert_eq!(round_tripped.organization, original.organization);
        assert_eq!(round_tripped.job_title, original.job_title);
        assert_eq!(round_tripped.emails[0].email, original.emails[0].email);
        assert_eq!(round_tripped.emails[0].label, original.emails[0].label);
        assert_eq!(round_tripped.phones[0].phone, original.phones[0].phone);
        assert_eq!(round_tripped.phones[0].label, original.phones[0].label);
    }
}
