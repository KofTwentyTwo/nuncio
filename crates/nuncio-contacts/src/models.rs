use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Contact {
    pub id: String,
    pub account_id: Option<String>,
    pub display_name: String,
    pub given_name: Option<String>,
    pub family_name: Option<String>,
    pub organization: Option<String>,
    pub job_title: Option<String>,
    pub notes: Option<String>,
    pub avatar_url: Option<String>,
    pub emails: Vec<ContactEmail>,
    pub phones: Vec<ContactPhone>,
    pub is_favorite: bool,
    pub interaction_count: u64,
    pub last_interacted_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContactEmail {
    pub email: String,
    pub label: String, // e.g. "work", "home", "other"
    pub is_primary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContactPhone {
    pub phone: String,
    pub label: String, // e.g. "mobile", "work", "home"
    pub is_primary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContactGroup {
    pub id: String,
    pub name: String,
    pub member_ids: Vec<String>,
}

impl Contact {
    pub fn new(display_name: impl Into<String>, primary_email: impl Into<String>) -> Self {
        let now = Utc::now();
        let email_str = primary_email.into();
        let id_str = format!("ct_{}_{}", now.timestamp_millis(), std::process::id());
        Self {
            id: id_str,
            account_id: None,
            display_name: display_name.into(),
            given_name: None,
            family_name: None,
            organization: None,
            job_title: None,
            notes: None,
            avatar_url: None,
            emails: vec![ContactEmail {
                email: email_str,
                label: "work".to_string(),
                is_primary: true,
            }],
            phones: Vec::new(),
            is_favorite: false,
            interaction_count: 0,
            last_interacted_at: None,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn to_vcard(&self) -> String {
        let mut vcard = String::new();
        vcard.push_str("BEGIN:VCARD\r\n");
        vcard.push_str("VERSION:4.0\r\n");
        vcard.push_str(&format!("FN:{}\r\n", self.display_name));
        if let (Some(g), Some(f)) = (&self.given_name, &self.family_name) {
            vcard.push_str(&format!("N:{};{};;;\r\n", f, g));
        }
        if let Some(org) = &self.organization {
            vcard.push_str(&format!("ORG:{}\r\n", org));
        }
        if let Some(title) = &self.job_title {
            vcard.push_str(&format!("TITLE:{}\r\n", title));
        }
        for email in &self.emails {
            vcard.push_str(&format!("EMAIL;TYPE={}:{}\r\n", email.label.to_uppercase(), email.email));
        }
        for phone in &self.phones {
            vcard.push_str(&format!("TEL;TYPE={}:{}\r\n", phone.label.to_uppercase(), phone.phone));
        }
        vcard.push_str("END:VCARD\r\n");
        vcard
    }
}
