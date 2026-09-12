use super::calendar::CalendarObject;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

mod fields;

#[derive(Debug, thiserror::Error)]
pub enum ChangeError {
    #[error("Calendar action has invalid, unsupported, or oversized fields")]
    Invalid,
    #[error("Calendar action is not permitted for this account or event")]
    Permission,
    #[error("Calendar action scope does not match the event")]
    Scope,
    #[error("The selected account must match exactly one event attendee")]
    Attendee,
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Single,
    Series,
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Notifications {
    None,
    All,
    ExternalOnly,
}
impl Notifications {
    pub fn parameter(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::All => "all",
            Self::ExternalOnly => "externalOnly",
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum CalendarAction {
    Create {
        scope: Scope,
        notifications: Notifications,
        event: Value,
    },
    Update {
        event_id: String,
        expected_etag: String,
        scope: Scope,
        notifications: Notifications,
        patch: Value,
    },
    Delete {
        event_id: String,
        expected_etag: String,
        scope: Scope,
        notifications: Notifications,
    },
    Respond {
        event_id: String,
        expected_etag: String,
        scope: Scope,
        notifications: Notifications,
        response: Response,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        comment: Option<String>,
    },
}
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Response {
    NeedsAction,
    Accepted,
    Declined,
    Tentative,
}
impl CalendarAction {
    pub fn from_value(value: Value, zone: &str) -> Result<Self, ChangeError> {
        if serde_json::to_vec(&value)
            .map_err(|_| ChangeError::Invalid)?
            .len()
            > 1024 * 1024
        {
            return Err(ChangeError::Invalid);
        }
        let action: Self = serde_json::from_value(value).map_err(|_| ChangeError::Invalid)?;
        if let Some((event, etag)) = action.identity() {
            if event.is_empty()
                || event.len() > 128
                || event.chars().any(char::is_control)
                || etag.len() > 2048
                || etag.len() < 3
                || !etag.starts_with('"')
                || !etag.ends_with('"')
                || !etag.bytes().all(|b| (32..=126).contains(&b))
                || etag[1..etag.len() - 1].contains('"')
            {
                return Err(ChangeError::Invalid);
            }
        }
        match &action {
            Self::Create { event, .. } => {
                fields::validate(event)?;
                validate_event(event.clone(), zone)?;
                action.check_scope(event)?;
            }
            Self::Update { patch, .. } => fields::validate(patch)?,
            Self::Respond { comment, .. }
                if comment
                    .as_ref()
                    .is_some_and(|v| v.len() > 65536 || v.contains('\0')) =>
            {
                return Err(ChangeError::Invalid)
            }
            _ => (),
        }
        Ok(action)
    }
    pub fn identity(&self) -> Option<(&str, &str)> {
        match self {
            Self::Create { .. } => None,
            Self::Update {
                event_id,
                expected_etag,
                ..
            }
            | Self::Delete {
                event_id,
                expected_etag,
                ..
            }
            | Self::Respond {
                event_id,
                expected_etag,
                ..
            } => Some((event_id, expected_etag)),
        }
    }
    pub fn notification_parameter(&self) -> &'static str {
        match self {
            Self::Create { notifications, .. }
            | Self::Update { notifications, .. }
            | Self::Delete { notifications, .. }
            | Self::Respond { notifications, .. } => notifications.parameter(),
        }
    }
    pub fn provider_patch(
        &self,
        base: Option<&Value>,
        zone: &str,
        account_email: &str,
        role: &str,
    ) -> Result<Value, ChangeError> {
        if !matches!(role, "owner" | "writer" | "writerWithoutPrivateAccess") {
            return Err(ChangeError::Permission);
        }
        if let Self::Create { event, .. } = self {
            return Ok(event.clone());
        }
        let base = base.ok_or(ChangeError::Invalid)?;
        if role == "writerWithoutPrivateAccess" && base["visibility"] == "private"
            || base["status"] == "cancelled"
            || base["locked"] == true
            || base.get("eventType").is_some_and(|v| v != "default")
        {
            return Err(ChangeError::Permission);
        }
        self.check_scope(base)?;
        match self {
            Self::Update { patch, .. } => {
                let organizer = base.get("organizer").is_none_or(|o| {
                    o.get("self").and_then(Value::as_bool).unwrap_or_else(|| {
                        o["email"]
                            .as_str()
                            .is_some_and(|e| e.eq_ignore_ascii_case(account_email))
                    })
                });
                if !organizer
                    && [
                        "attendees",
                        "guestsCanModify",
                        "guestsCanInviteOthers",
                        "guestsCanSeeOtherGuests",
                    ]
                    .iter()
                    .any(|k| patch.get(*k).is_some())
                {
                    return Err(ChangeError::Permission);
                }
                let mut merged = base.clone();
                merge(&mut merged, patch);
                validate_event(merged, zone)?;
                Ok(patch.clone())
            }
            Self::Respond {
                response, comment, ..
            } => {
                let attendees = base["attendees"].as_array().ok_or(ChangeError::Attendee)?;
                let matches: Vec<_> = attendees
                    .iter()
                    .filter(|a| {
                        a["email"]
                            .as_str()
                            .is_some_and(|e| e.eq_ignore_ascii_case(account_email))
                    })
                    .collect();
                if matches.len() != 1 {
                    return Err(ChangeError::Attendee);
                }
                let mut attendee = json!({"email":matches[0]["email"],"responseStatus":response});
                if let Some(comment) = comment {
                    attendee["comment"] = json!(comment);
                }
                // Native partial response avoids replacing other attendees or copying
                // provider-owned fields back into a write request.
                Ok(json!({"attendeesOmitted":true,"attendees":[attendee]}))
            }
            Self::Delete { .. } => Ok(Value::Null),
            Self::Create { .. } => Err(ChangeError::Invalid),
        }
    }
    fn check_scope(&self, base: &Value) -> Result<(), ChangeError> {
        let scope = match self {
            Self::Create { scope, .. }
            | Self::Update { scope, .. }
            | Self::Delete { scope, .. }
            | Self::Respond { scope, .. } => *scope,
        };
        let master = base
            .get("recurrence")
            .is_some_and(|v| v.as_array().is_some_and(|a| !a.is_empty()))
            && base.get("recurringEventId").is_none();
        if (scope == Scope::Series) != master {
            return Err(ChangeError::Scope);
        }
        Ok(())
    }
}
fn validate_event(mut value: Value, zone: &str) -> Result<(), ChangeError> {
    if value.get("id").is_none() {
        value["id"] = json!("validation");
    }
    CalendarObject::from_google(value.clone(), zone).map_err(|_| ChangeError::Invalid)?;
    if let Some(rules) = value.get("recurrence") {
        let rules = rules.as_array().ok_or(ChangeError::Invalid)?;
        if rules.is_empty() {
            return Err(ChangeError::Invalid);
        }
        if value["start"].get("dateTime").is_some()
            && (value["start"].get("timeZone").is_none() || value["end"].get("timeZone").is_none())
        {
            return Err(ChangeError::Invalid);
        }
    }
    Ok(())
}
pub(crate) fn merge(target: &mut Value, patch: &Value) {
    if let (Some(target), Some(patch)) = (target.as_object_mut(), patch.as_object()) {
        for (key, value) in patch {
            if value.is_null() {
                target.remove(key);
            } else if value.is_object() {
                merge(target.entry(key.clone()).or_insert(json!({})), value);
            } else {
                target.insert(key.clone(), value.clone());
            }
        }
    } else {
        *target = patch.clone();
    }
}
