use super::ChangeError;
use serde_json::Value;
use std::collections::BTreeSet;

pub(super) fn validate(value: &Value) -> Result<(), ChangeError> {
    let fields = value
        .as_object()
        .filter(|v| !v.is_empty())
        .ok_or(ChangeError::Invalid)?;
    for (key, value) in fields {
        let valid = match key.as_str() {
            "summary" | "description" | "location" => text(value, 65536) || value.is_null(),
            "colorId" => text(value, 32) || value.is_null(),
            "start" | "end" => time(value),
            "transparency" => ["opaque", "transparent"].iter().any(|s| value == s),
            "visibility" => ["default", "public", "private", "confidential"]
                .iter()
                .any(|s| value == s),
            "guestsCanModify" | "guestsCanInviteOthers" | "guestsCanSeeOtherGuests" => {
                value.is_boolean()
            }
            "recurrence" => {
                value.is_null()
                    || value.as_array().is_some_and(|a| {
                        !a.is_empty()
                            && a.len() <= 128
                            && a.iter().all(|v| {
                                v.as_str().is_some_and(|s| {
                                    s.len() <= 8192
                                        && !s.chars().any(char::is_control)
                                        && [
                                            "RRULE:", "EXRULE:", "RDATE:", "RDATE;", "EXDATE:",
                                            "EXDATE;",
                                        ]
                                        .iter()
                                        .any(|prefix| s.starts_with(prefix))
                                })
                            })
                    })
            }
            "attendees" => attendees(value),
            "reminders" => reminders(value),
            _ => false,
        };
        if !valid {
            return Err(ChangeError::Invalid);
        }
    }
    Ok(())
}
fn text(value: &Value, max: usize) -> bool {
    value
        .as_str()
        .is_some_and(|s| s.len() <= max && !s.contains('\0'))
}
fn time(value: &Value) -> bool {
    value.as_object().is_some_and(|o| {
        !o.is_empty()
            && o.iter().all(|(k, v)| match k.as_str() {
                // Explicit null removes the old representation when changing date/time kind.
                "date" | "dateTime" => v.is_null() || text(v, 64),
                "timeZone" => {
                    v.is_null()
                        || v.as_str()
                            .is_some_and(|s| s.parse::<chrono_tz::Tz>().is_ok())
                }
                _ => false,
            })
    })
}
fn attendees(value: &Value) -> bool {
    let Some(attendees) = value.as_array().filter(|a| a.len() <= 1000) else {
        return false;
    };
    let mut emails = BTreeSet::new();
    attendees.iter().all(|a| {
        let Some(o) = a.as_object() else {
            return false;
        };
        let Some(email) = o.get("email").and_then(Value::as_str).filter(|s| {
            s.len() <= 320
                && email_address::EmailAddress::is_valid(s)
                && !s.chars().any(char::is_control)
        }) else {
            return false;
        };
        emails.insert(email.to_ascii_lowercase())
            && o.iter().all(|(k, v)| match k.as_str() {
                "email" => true,
                "displayName" => text(v, 65536),
                "optional" | "resource" => v.is_boolean(),
                // Responses belong to the explicit selected-account RSVP action.
                "responseStatus" => v == "needsAction",
                _ => false,
            })
    })
}
fn reminders(value: &Value) -> bool {
    if value.is_null() {
        return true;
    }
    let Some(o) = value.as_object() else {
        return false;
    };
    o.iter().all(|(k, v)| match k.as_str() {
        "useDefault" => v.is_boolean(),
        "overrides" => v.as_array().is_some_and(|a| {
            a.len() <= 5
                && a.iter().all(|v| {
                    v.as_object().is_some_and(|o| o.len() == 2)
                        && (v["method"] == "email" || v["method"] == "popup")
                        && v["minutes"].as_u64().is_some_and(|m| m <= 40320)
                })
        }),
        _ => false,
    }) && !(value["useDefault"] == true && value.get("overrides").is_some())
}
