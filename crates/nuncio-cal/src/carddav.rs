//! CardDAV (vCard RFC 6350) contact sync and XML query parser.

use serde::{Deserialize, Serialize};
use crate::parser::CalendarError;

/// Contact entity representation synced over CardDAV.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contact {
    pub id: String,
    pub name: String,
    pub email: String,
    pub phone: Option<String>,
    pub organization: Option<String>,
}

/// CardDAV client protocol engine managing vCard address book queries.
pub struct CardDavClient {
    account_id: String,
}

impl CardDavClient {
    /// Create a new `CardDavClient` bound to an account ID.
    pub fn new(account_id: &str) -> Self {
        Self {
            account_id: account_id.to_string(),
        }
    }

    /// Construct a standard CardDAV `<addressbook-query>` XML payload.
    pub fn build_addressbook_query() -> String {
        r#"<?xml version="1.0" encoding="utf-8" ?>
<card:addressbook-query xmlns:d="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav">
    <d:prop>
        <d:getetag />
        <card:address-data />
    </d:prop>
</card:addressbook-query>"#.to_string()
    }

    /// Parse raw vCard string into domain [`Contact`] entity.
    pub fn parse_vcard(id: &str, raw_vcard: &str) -> Result<Contact, CalendarError> {
        let mut name = String::new();
        let mut email = String::new();
        let mut phone = None;
        let mut organization = None;

        for line in raw_vcard.lines() {
            let line = line.trim();
            if let Some(val) = line.strip_prefix("FN:") {
                name = val.trim().to_string();
            } else if let Some(val) = line.strip_prefix("EMAIL:") {
                email = val.trim().to_string();
            } else if line.starts_with("EMAIL;") {
                if let Some((_, val)) = line.split_once(':') {
                    email = val.trim().to_string();
                }
            } else if let Some(val) = line.strip_prefix("TEL:") {
                phone = Some(val.trim().to_string());
            } else if line.starts_with("TEL;") {
                if let Some((_, val)) = line.split_once(':') {
                    phone = Some(val.trim().to_string());
                }
            } else if let Some(val) = line.strip_prefix("ORG:") {
                organization = Some(val.trim().to_string());
            }
        }

        if name.is_empty() && email.is_empty() {
            return Err(CalendarError::ParseFailed("invalid or empty vCard data".to_string()));
        }

        if name.is_empty() {
            name = email.clone();
        }

        Ok(Contact {
            id: id.to_string(),
            name,
            email,
            phone,
            organization,
        })
    }

    /// Parse CardDAV WebDAV XML `<multistatus>` response containing embedded vCard data.
    pub fn parse_multistatus_response(&self, raw_xml: &str) -> Result<Vec<Contact>, CalendarError> {
        let mut contacts = Vec::new();

        for block in raw_xml.split("<card:address-data>") {
            if let Some((vcard_data, _)) = block.split_once("</card:address-data>") {
                let clean_vcard = vcard_data.trim();
                if !clean_vcard.is_empty() {
                    let contact_id = format!("{}-contact-{}", self.account_id, contacts.len() + 1);
                    if let Ok(contact) = Self::parse_vcard(&contact_id, clean_vcard) {
                        contacts.push(contact);
                    }
                }
            }
        }

        Ok(contacts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_vcard_valid_structure() {
        let vcard = r#"BEGIN:VCARD
VERSION:4.0
FN:James Maes
EMAIL:james.maes@kof22.com
TEL:+1-555-0199
ORG:Kof22 Labs
END:VCARD"#;

        let contact = CardDavClient::parse_vcard("c-1", vcard).expect("parse vcard");
        assert_eq!(contact.name, "James Maes");
        assert_eq!(contact.email, "james.maes@kof22.com");
        assert_eq!(contact.phone.as_deref(), Some("+1-555-0199"));
        assert_eq!(contact.organization.as_deref(), Some("Kof22 Labs"));
    }

    #[test]
    fn parse_multistatus_carddav_xml() {
        let client = CardDavClient::new("acct-1");
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
        <d:multistatus xmlns:d="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav">
            <d:response>
                <card:address-data>BEGIN:VCARD
FN:Alice Dev
EMAIL:alice@nuncio.mx
END:VCARD</card:address-data>
            </d:response>
        </d:multistatus>"#;

        let contacts = client.parse_multistatus_response(xml).expect("parse multistatus");
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].name, "Alice Dev");
    }
}
