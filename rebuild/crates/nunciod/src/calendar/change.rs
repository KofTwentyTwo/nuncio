use nuncio_engine::store::EnqueueCalendarChange;
use nuncio_proto::v2::{self, change_event_request::Action};
use serde_json::{json, Value};
use tonic::Status;

pub(super) fn input(q: v2::ChangeEventRequest) -> Result<EnqueueCalendarChange, Status> {
    let scope = match v2::CalendarEditScope::try_from(q.scope) {
        Ok(v2::CalendarEditScope::Single) => "single",
        Ok(v2::CalendarEditScope::Series) => "series",
        _ => return Err(invalid()),
    };
    let notifications = match v2::CalendarNotificationPolicy::try_from(q.notification_policy) {
        Ok(v2::CalendarNotificationPolicy::None) => "none",
        Ok(v2::CalendarNotificationPolicy::All) => "all",
        Ok(v2::CalendarNotificationPolicy::ExternalOnly) => "external_only",
        _ => return Err(invalid()),
    };
    let mut action = match q.action.ok_or_else(invalid)? {
        Action::Create(q) => {
            json!({"action":"create","event":fields(q.event.ok_or_else(invalid)?,false)?})
        }
        Action::Update(q) => {
            let mut patch = fields(q.fields.unwrap_or_default(), true)?;
            let mut seen = std::collections::BTreeSet::new();
            for name in q.clear_fields {
                let key = match name.as_str() {
                    "summary" | "description" | "location" | "recurrence" | "reminders" => {
                        name.as_str()
                    }
                    "color_id" => "colorId",
                    _ => return Err(invalid()),
                };
                if !seen.insert(key.to_string()) || patch.get(key).is_some() {
                    return Err(invalid());
                }
                patch[key] = Value::Null;
            }
            json!({"action":"update","event_id":q.event_id,"expected_etag":q.expected_etag,"patch":patch})
        }
        Action::Delete(q) => {
            json!({"action":"delete","event_id":q.event_id,"expected_etag":q.expected_etag})
        }
        Action::Respond(q) => {
            let response = match v2::CalendarResponseStatus::try_from(q.response) {
                Ok(v2::CalendarResponseStatus::NeedsAction) => "needsAction",
                Ok(v2::CalendarResponseStatus::Accepted) => "accepted",
                Ok(v2::CalendarResponseStatus::Declined) => "declined",
                Ok(v2::CalendarResponseStatus::Tentative) => "tentative",
                _ => return Err(invalid()),
            };
            let mut v = json!({"action":"respond","event_id":q.event_id,"expected_etag":q.expected_etag,"response":response});
            if let Some(comment) = q.comment {
                v["comment"] = json!(comment);
            }
            v
        }
    };
    action["scope"] = json!(scope);
    action["notifications"] = json!(notifications);
    Ok(EnqueueCalendarChange {
        account_id: q.account_id,
        calendar_id: q.calendar_id,
        request_id: q.request_id,
        action,
    })
}
fn fields(f: v2::CalendarEventFields, patch: bool) -> Result<Value, Status> {
    let mut result = json!({});
    for (key, value) in [
        ("summary", f.summary),
        ("description", f.description),
        ("location", f.location),
        ("transparency", f.transparency),
        ("visibility", f.visibility),
        ("colorId", f.color_id),
    ] {
        if let Some(value) = value {
            result[key] = json!(value);
        }
    }
    for (key, value) in [
        ("guestsCanModify", f.guests_can_modify),
        ("guestsCanInviteOthers", f.guests_can_invite_others),
        ("guestsCanSeeOtherGuests", f.guests_can_see_other_guests),
    ] {
        if let Some(value) = value {
            result[key] = json!(value);
        }
    }
    for (key, value) in [("start", f.start), ("end", f.end)] {
        if let Some(value) = value {
            result[key] = time(value, patch)?;
        }
    }
    if let Some(recurrence) = f.recurrence {
        result["recurrence"] = json!(recurrence.rules);
    }
    if let Some(attendees) = f.attendees {
        result["attendees"] = Value::Array(
            attendees
                .items
                .into_iter()
                .map(|a| {
                    let mut v = json!({"email":a.email});
                    if let Some(name) = a.display_name {
                        v["displayName"] = json!(name);
                    }
                    if let Some(optional) = a.optional {
                        v["optional"] = json!(optional);
                    }
                    if let Some(resource) = a.resource {
                        v["resource"] = json!(resource);
                    }
                    v
                })
                .collect(),
        );
    }
    if let Some(reminders) = f.reminders {
        if reminders.use_default && !reminders.overrides.is_empty() {
            return Err(invalid());
        }
        let mut value = json!({"useDefault":reminders.use_default});
        if !reminders.use_default {
            value["overrides"] = Value::Array(
                reminders
                    .overrides
                    .into_iter()
                    .map(|r| json!({"method":r.method,"minutes":r.minutes}))
                    .collect(),
            );
        }
        result["reminders"] = value;
    }
    Ok(result)
}
fn time(time: v2::EventTime, patch: bool) -> Result<Value, Status> {
    Ok(match time.value.ok_or_else(invalid)? {
        v2::event_time::Value::Date(date) => {
            if patch {
                json!({"date":date,"dateTime":null,"timeZone":null})
            } else {
                json!({"date":date})
            }
        }
        v2::event_time::Value::DateTime(t) => {
            let mut value = json!({"dateTime":t.rfc3339});
            if !t.time_zone.is_empty() {
                value["timeZone"] = json!(t.time_zone);
            }
            if patch {
                value["date"] = Value::Null;
            }
            value
        }
    })
}
fn invalid() -> Status {
    Status::invalid_argument(
        "Calendar action requires supported fields, explicit scope and notification policy",
    )
}
