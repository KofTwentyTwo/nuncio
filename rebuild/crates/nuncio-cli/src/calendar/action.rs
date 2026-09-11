#[cfg(test)]
mod tests {
    #[test]
    #[allow(clippy::unwrap_used)]
    fn calendar_action_files_are_versioned_strict_and_preserve_explicit_policy() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("action.json");
        for invalid in [
            r#"{"schema_version":1,"action":"create","scope":"single","notifications":"none","event":{"summary":null}}"#,
            r#"{"action":"delete","event_id":"event","expected_etag":"\"v1\"","scope":"single","notifications":"none"}"#,
            r#"{"schema_version":2,"action":"delete","event_id":"event","expected_etag":"\"v1\"","scope":"single","notifications":"none"}"#,
            r#"{"schema_version":1,"action":"delete","event_id":"event","expected_etag":"\"v1\"","scope":"following","notifications":"none"}"#,
            r#"{"schema_version":1,"action":"delete","event_id":"event","expected_etag":"\"v1\"","scope":"single"}"#,
            r#"{"schema_version":1,"action":"create","scope":"single","notifications":"none","event":{"organizer":"cannot transfer"}}"#,
            r#"{"schema_version":1,"action":"create","scope":"single","notifications":"none","event":{"start":{"date":"2026-10-01","extra":true}}}"#,
            r#"{"schema_version":1,"action":"respond","event_id":"event","expected_etag":"\"v1\"","scope":"single","notifications":"all","response":"maybe"}"#,
            r#"{"schema_version":1,"schema_version":2,"action":"delete","event_id":"event","expected_etag":"\"v1\"","scope":"single","notifications":"none"}"#,
        ] {
            std::fs::write(&path, invalid).unwrap();
            assert!(super::read(&path).is_err(), "accepted {invalid}");
        }
        std::fs::write(&path,r#"{"schema_version":1,"action":"respond","event_id":"event","expected_etag":"\"v1\"","scope":"single","notifications":"external_only","response":"declined","comment":"Unavailable"}"#).unwrap();
        let q = super::read(&path).map_err(|e| e.code).unwrap();
        assert_eq!(q.scope, nuncio_proto::v2::CalendarEditScope::Single as i32);
        assert_eq!(
            q.notification_policy,
            nuncio_proto::v2::CalendarNotificationPolicy::ExternalOnly as i32
        );
        assert!(
            matches!(q.action,Some(nuncio_proto::v2::change_event_request::Action::Respond(v)) if v.response==nuncio_proto::v2::CalendarResponseStatus::Declined as i32 && v.comment.as_deref()==Some("Unavailable"))
        );
        std::fs::write(&path, vec![b' '; 1024 * 1024 + 1]).unwrap();
        assert!(super::read(&path).is_err());
    }
}

use crate::output::AppError;
use nuncio_proto::v2::{self, change_event_request::Action};
use serde::Deserialize;
use std::{io::Read, path::Path};

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum ActionFile {
    Create {
        schema_version: u32,
        scope: Scope,
        notifications: Policy,
        event: Fields,
    },
    Update {
        schema_version: u32,
        scope: Scope,
        notifications: Policy,
        event_id: String,
        expected_etag: String,
        #[serde(default)]
        patch: Fields,
        #[serde(default)]
        clear_fields: Vec<String>,
    },
    Delete {
        schema_version: u32,
        scope: Scope,
        notifications: Policy,
        event_id: String,
        expected_etag: String,
    },
    Respond {
        schema_version: u32,
        scope: Scope,
        notifications: Policy,
        event_id: String,
        expected_etag: String,
        response: Response,
        #[serde(default, deserialize_with = "present")]
        comment: Option<String>,
    },
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Scope {
    Single,
    Series,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Policy {
    None,
    All,
    ExternalOnly,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Response {
    NeedsAction,
    Accepted,
    Declined,
    Tentative,
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Fields {
    #[serde(default, deserialize_with = "present")]
    summary: Option<String>,
    #[serde(default, deserialize_with = "present")]
    description: Option<String>,
    #[serde(default, deserialize_with = "present")]
    location: Option<String>,
    #[serde(default, deserialize_with = "present")]
    start: Option<Time>,
    #[serde(default, deserialize_with = "present")]
    end: Option<Time>,
    #[serde(default, deserialize_with = "present")]
    recurrence: Option<Vec<String>>,
    #[serde(default, deserialize_with = "present")]
    attendees: Option<Vec<Attendee>>,
    #[serde(default, deserialize_with = "present")]
    reminders: Option<Reminders>,
    #[serde(default, deserialize_with = "present")]
    guests_can_modify: Option<bool>,
    #[serde(default, deserialize_with = "present")]
    guests_can_invite_others: Option<bool>,
    #[serde(default, deserialize_with = "present")]
    guests_can_see_other_guests: Option<bool>,
    #[serde(default, deserialize_with = "present")]
    transparency: Option<String>,
    #[serde(default, deserialize_with = "present")]
    visibility: Option<String>,
    #[serde(default, deserialize_with = "present")]
    color_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(untagged, deny_unknown_fields)]
enum Time {
    Date { date: String },
    Timed { date_time: ZonedTime },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ZonedTime {
    rfc3339: String,
    #[serde(default)]
    time_zone: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Attendee {
    email: String,
    #[serde(default, deserialize_with = "present")]
    display_name: Option<String>,
    #[serde(default, deserialize_with = "present")]
    optional: Option<bool>,
    #[serde(default, deserialize_with = "present")]
    resource: Option<bool>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reminders {
    use_default: bool,
    #[serde(default)]
    overrides: Vec<Reminder>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reminder {
    method: String,
    minutes: u32,
}

fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

pub(super) fn read(path: &Path) -> Result<v2::ChangeEventRequest, AppError> {
    let mut bytes = Vec::new();
    crate::drafts::regular_file(path)?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| AppError::invalid())?;
    if bytes.len() > 1024 * 1024 {
        return Err(AppError::invalid());
    }
    let input: ActionFile = serde_json::from_slice(&bytes).map_err(|_| AppError::invalid())?;
    let (version, scope, policy, action) = match input {
        ActionFile::Create {
            schema_version,
            scope,
            notifications,
            event,
        } => (
            schema_version,
            scope,
            notifications,
            Action::Create(v2::CalendarCreate {
                event: Some(fields(event)),
            }),
        ),
        ActionFile::Update {
            schema_version,
            scope,
            notifications,
            event_id,
            expected_etag,
            patch,
            clear_fields,
        } => (
            schema_version,
            scope,
            notifications,
            Action::Update(v2::CalendarUpdate {
                event_id,
                expected_etag,
                fields: Some(fields(patch)),
                clear_fields,
            }),
        ),
        ActionFile::Delete {
            schema_version,
            scope,
            notifications,
            event_id,
            expected_etag,
        } => (
            schema_version,
            scope,
            notifications,
            Action::Delete(v2::CalendarDelete {
                event_id,
                expected_etag,
            }),
        ),
        ActionFile::Respond {
            schema_version,
            scope,
            notifications,
            event_id,
            expected_etag,
            response,
            comment,
        } => (
            schema_version,
            scope,
            notifications,
            Action::Respond(v2::CalendarRespond {
                event_id,
                expected_etag,
                response: match response {
                    Response::NeedsAction => v2::CalendarResponseStatus::NeedsAction,
                    Response::Accepted => v2::CalendarResponseStatus::Accepted,
                    Response::Declined => v2::CalendarResponseStatus::Declined,
                    Response::Tentative => v2::CalendarResponseStatus::Tentative,
                } as i32,
                comment,
            }),
        ),
    };
    if version != 1 {
        return Err(AppError::invalid());
    }
    Ok(v2::ChangeEventRequest {
        scope: match scope {
            Scope::Single => v2::CalendarEditScope::Single,
            Scope::Series => v2::CalendarEditScope::Series,
        } as i32,
        notification_policy: match policy {
            Policy::None => v2::CalendarNotificationPolicy::None,
            Policy::All => v2::CalendarNotificationPolicy::All,
            Policy::ExternalOnly => v2::CalendarNotificationPolicy::ExternalOnly,
        } as i32,
        action: Some(action),
        ..Default::default()
    })
}
fn fields(f: Fields) -> v2::CalendarEventFields {
    v2::CalendarEventFields {
        summary: f.summary,
        description: f.description,
        location: f.location,
        start: f.start.map(time),
        end: f.end.map(time),
        recurrence: f.recurrence.map(|rules| v2::CalendarRecurrence { rules }),
        attendees: f.attendees.map(|a| v2::CalendarAttendees {
            items: a
                .into_iter()
                .map(|a| v2::CalendarAttendee {
                    email: a.email,
                    display_name: a.display_name,
                    optional: a.optional,
                    resource: a.resource,
                })
                .collect(),
        }),
        reminders: f.reminders.map(|r| v2::CalendarReminders {
            use_default: r.use_default,
            overrides: r
                .overrides
                .into_iter()
                .map(|r| v2::CalendarReminder {
                    method: r.method,
                    minutes: r.minutes,
                })
                .collect(),
        }),
        guests_can_modify: f.guests_can_modify,
        guests_can_invite_others: f.guests_can_invite_others,
        guests_can_see_other_guests: f.guests_can_see_other_guests,
        transparency: f.transparency,
        visibility: f.visibility,
        color_id: f.color_id,
    }
}
fn time(t: Time) -> v2::EventTime {
    v2::EventTime {
        value: Some(match t {
            Time::Date { date } => v2::event_time::Value::Date(date),
            Time::Timed { date_time: t } => v2::event_time::Value::DateTime(v2::ZonedDateTime {
                rfc3339: t.rfc3339,
                time_zone: t.time_zone,
            }),
        }),
    }
}
