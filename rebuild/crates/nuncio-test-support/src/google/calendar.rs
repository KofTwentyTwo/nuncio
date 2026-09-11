use super::{
    calendar_recurrence,
    calendar_state::{instant, timestamp, Calendar, SyncCursor},
    paging,
    state::Model,
    wire::{Input, Query, Reply, Result},
};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};

pub(super) fn route(model: &mut Model, input: &Input, account: &str) -> Result<Reply> {
    if input.path == "/calendar/v3/users/me/calendarList" && input.method == "GET" {
        input.query.validate(&["maxResults", "pageToken"], &[])?;
        let size = input.query.page_size(100, 250)?;
        let scope = paging::scope(account, input);
        if let Some(page) = model.saved_page(&scope, input)? {
            return Ok(page);
        }
        let calendars = model
            .calendars
            .get(account)
            .ok_or_else(|| Reply::error(404, "notFound"))?
            .values()
            .map(Calendar::listing)
            .collect();
        return Ok(model.paginate(
            scope,
            "items",
            calendars,
            size,
            json!({"kind":"calendar#calendarList"}),
            json!({}),
        ));
    }
    if input.path == "/calendar/v3/freeBusy" && input.method == "POST" {
        return free_busy(model, input, account);
    }
    let decoded = input.segments()?;
    let segments: Vec<_> = decoded.iter().map(String::as_str).collect();
    if segments.len() < 5
        || segments[..3] != ["calendar", "v3", "calendars"]
        || segments[4] != "events"
    {
        return Err(Reply::error(404, "notFound"));
    }
    let calendar_id = if segments[3] == "primary" {
        account
    } else {
        segments[3]
    };
    match (input.method.as_str(), &segments[5..]) {
        ("GET", []) => list(model, input, account, calendar_id, None),
        ("GET", [id, "instances"]) => list(model, input, account, calendar_id, Some(id)),
        ("GET", [id]) => {
            input.query.validate(&[], &[])?;
            Ok(Reply::json(
                model.calendar(account, calendar_id)?.event(id)?,
            ))
        }
        ("POST", []) | ("PATCH", [_]) | ("DELETE", [_]) => {
            write(model, input, account, calendar_id, segments.get(5).copied())
        }
        _ => Err(Reply::error(405, "methodNotAllowed")),
    }
}
fn bounds(query: &Query) -> Result<(DateTime<Utc>, DateTime<Utc>)> {
    let lower = timestamp(query.get("timeMin").unwrap_or("2025-01-01T00:00:00Z"))?;
    let upper = timestamp(query.get("timeMax").unwrap_or("2030-01-01T00:00:00Z"))?;
    if lower >= upper {
        return Err(Reply::error(400, "invalidTimeRange"));
    }
    Ok((lower, upper))
}
fn shape(input: &Input) -> String {
    let mut fields = input.query.0.clone();
    fields.remove("pageToken");
    fields.remove("syncToken");
    format!("{fields:?}")
}
fn list(
    model: &mut Model,
    input: &Input,
    account: &str,
    calendar_id: &str,
    master: Option<&str>,
) -> Result<Reply> {
    let query = &input.query;
    if master.is_some() {
        query.validate(
            &[
                "maxResults",
                "pageToken",
                "timeMin",
                "timeMax",
                "showDeleted",
                "timeZone",
            ],
            &[],
        )?;
    } else {
        query.validate(
            &[
                "maxResults",
                "pageToken",
                "syncToken",
                "singleEvents",
                "showDeleted",
                "timeMin",
                "timeMax",
                "orderBy",
                "timeZone",
            ],
            &[],
        )?;
    }
    let size = query.page_size(250, 2500)?;
    let single = master.is_some() || query.boolean("singleEvents", false)?;
    let deleted = query.boolean("showDeleted", false)?;
    if let Some(zone) = query.get("timeZone") {
        zone.parse::<chrono_tz::Tz>()
            .map_err(|_| Reply::error(400, "invalidTimeZone"))?;
    }
    if query
        .get("orderBy")
        .is_some_and(|order| !["startTime", "updated"].contains(&order))
        || query.get("orderBy") == Some("startTime") && !single
    {
        return Err(Reply::error(400, "invalidArgument"));
    }
    let since = if let Some(token) = query.get("syncToken") {
        if !deleted
            || ["timeMin", "timeMax", "orderBy"]
                .iter()
                .any(|key| query.get(key).is_some())
        {
            return Err(Reply::error(400, "invalidArgument"));
        }
        let cursor = model
            .calendar_tokens
            .get(token)
            .ok_or_else(|| Reply::error(410, "fullSyncRequired"))?;
        if cursor.account != account || cursor.calendar != calendar_id {
            return Err(Reply::error(410, "fullSyncRequired"));
        }
        if cursor.shape != shape(input) {
            return Err(Reply::error(400, "invalidArgument"));
        }
        Some(cursor.version)
    } else {
        None
    };
    let scope = paging::scope(account, input);
    if let Some(page) = model.saved_page(&scope, input)? {
        return Ok(page);
    }
    let calendar = model.calendar(account, calendar_id)?;
    let response_role = calendar.access_role.clone();
    let version = calendar.version;
    let (lower, upper) = bounds(query)?;
    let mut items = if single {
        if master.is_some_and(|id| {
            !calendar
                .events
                .get(id)
                .is_some_and(|e| e.body.get("recurrence").is_some())
        }) {
            return Err(Reply::error(404, "notFound"));
        }
        calendar_recurrence::expanded(calendar, lower, upper, master, deleted)?
    } else {
        calendar
            .events
            .values()
            .filter(|e| since.is_none_or(|v| e.version > v))
            .filter(|e| {
                deleted
                    || e.body["status"] != "cancelled"
                    || e.body.get("recurringEventId").is_some()
            })
            .map(|e| e.body.clone())
            .collect()
    };
    if !single && (query.get("timeMin").is_some() || query.get("timeMax").is_some()) {
        return Err(Reply::error(400, "unsupportedCanonicalWindow"));
    }
    if single && since.is_some() {
        return Err(Reply::error(400, "unsupportedExpandedSync"));
    }
    if query.get("orderBy") == Some("startTime") {
        items.sort_by_key(|event| {
            instant(
                event.get("start").unwrap_or(&event["originalStartTime"]),
                "America/Chicago",
            )
            .ok()
        });
    } else if model.reverse {
        items.reverse();
    }
    let response_zone = query.get("timeZone").unwrap_or("America/Chicago");
    if query.get("timeZone").is_some() {
        let zone = response_zone
            .parse::<chrono_tz::Tz>()
            .map_err(|_| Reply::error(400, "invalidTimeZone"))?;
        for event in &mut items {
            for field in ["start", "end", "originalStartTime"] {
                if event[field].get("dateTime").is_some() {
                    event[field]["dateTime"] = json!(instant(&event[field], "America/Chicago")?
                        .with_timezone(&zone)
                        .to_rfc3339());
                }
            }
        }
    }
    let final_fields = if master.is_none() {
        let token = model.next("sync");
        model.calendar_tokens.insert(
            token.clone(),
            SyncCursor {
                account: account.into(),
                calendar: calendar_id.into(),
                shape: shape(input),
                version,
            },
        );
        json!({"nextSyncToken":token})
    } else {
        json!({})
    };
    Ok(model.paginate(scope,"items",items,size,json!({"kind":"calendar#events","timeZone":response_zone,"etag":format!("\"calendar-{version}\""),"accessRole":response_role,"defaultReminders":[]}),final_fields))
}
fn policy(input: &Input) -> Result<&str> {
    input.query.validate(&["sendUpdates"], &[])?;
    let policy = input.query.get("sendUpdates").unwrap_or("none");
    if !["none", "all", "externalOnly"].contains(&policy) {
        return Err(Reply::error(400, "invalidArgument"));
    }
    Ok(policy)
}
fn write(
    model: &mut Model,
    input: &Input,
    account: &str,
    calendar_id: &str,
    id: Option<&str>,
) -> Result<Reply> {
    let policy = policy(input)?;
    let creation = input.method == "POST";
    let mut body = if input.method == "DELETE" {
        if !input.body.is_empty() {
            return Err(Reply::error(400, "invalidArgument"));
        }
        json!({})
    } else {
        input.json::<Value>()?
    };
    if !body.is_object() {
        return Err(Reply::error(400, "invalidArgument"));
    }
    if creation && body.get("id").is_none() {
        body["id"] = json!(format!("created{:016x}", model.sequence));
        model.sequence += 1;
    }
    let id = if creation {
        body["id"]
            .as_str()
            .ok_or_else(|| Reply::error(400, "invalidArgument"))?
    } else {
        id.ok_or_else(|| Reply::error(400, "invalidArgument"))?
    }
    .to_string();
    let calendar = model.calendar_mut(account, calendar_id)?;
    if !matches!(calendar.access_role.as_str(), "owner" | "writer") {
        return Err(Reply::error(403, "forbidden"));
    }
    if creation {
        if !(5..=1024).contains(&id.len())
            || !id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'v').contains(&b))
        {
            return Err(Reply::error(400, "invalidArgument"));
        }
        if calendar.events.contains_key(&id) {
            return Err(Reply::error(409, "duplicate"));
        }
    } else {
        let existing = calendar.event(&id)?;
        let organizer_copy = existing.get("organizer").is_none_or(|organizer| {
            organizer
                .get("self")
                .and_then(Value::as_bool)
                .unwrap_or_else(|| organizer["email"].as_str() == Some(calendar_id))
        });
        if !organizer_copy
            && [
                "guestsCanInviteOthers",
                "guestsCanModify",
                "guestsCanSeeOtherGuests",
            ]
            .iter()
            .any(|field| body.get(*field).is_some())
        {
            return Err(Reply::error(403, "forbiddenForNonOrganizer"));
        }
        if let Some(expected) = input.headers.get("if-match") {
            if expected.to_str().ok() != existing["etag"].as_str() {
                return Err(Reply::error(412, "conditionNotMet"));
            }
        }
        if body.get("id").is_some_and(|value| value != &id) {
            return Err(Reply::error(400, "immutableId"));
        }
        if input.method == "DELETE" {
            body = existing.clone();
            body["status"] = json!("cancelled");
            calendar.put(body, policy, "delete")?;
            return Ok(Reply {
                status: 204,
                body: Value::Null,
                location: None,
            });
        }
        validate_patch(&body)?;
        partial_response(&mut body, &existing, account)?;
        let mut merged = existing;
        merge(&mut merged, body);
        body = merged;
    }
    if creation {
        validate_patch(&body)?;
        if body["attendeesOmitted"] == true {
            return Err(Reply::error(400, "invalidArgument"));
        }
    }
    validate_event(&body)?;
    if body.get("status").is_none() {
        body["status"] = json!("confirmed");
    }
    let body = calendar.put(body, policy, if creation { "insert" } else { "patch" })?;
    Ok(Reply::json(body))
}
// Google supports updating just the participant response without replacing the
// complete attendee array (Events.attendeesOmitted). Model that on server state.
fn partial_response(body: &mut Value, existing: &Value, account: &str) -> Result<()> {
    if body["attendeesOmitted"] != true {
        return Ok(());
    }
    let changes = body["attendees"]
        .as_array()
        .filter(|a| a.len() == 1)
        .ok_or_else(|| Reply::error(400, "unsupportedPartialAttendees"))?;
    let change = changes[0]
        .as_object()
        .ok_or_else(|| Reply::error(400, "invalidArgument"))?;
    if change
        .keys()
        .any(|k| !["email", "responseStatus", "comment"].contains(&k.as_str()))
    {
        return Err(Reply::error(400, "unsupportedPartialAttendeeField"));
    }
    let email = change
        .get("email")
        .and_then(Value::as_str)
        .ok_or_else(|| Reply::error(400, "invalidArgument"))?;
    let mut attendees = existing["attendees"]
        .as_array()
        .ok_or_else(|| Reply::error(400, "invalidAttendee"))?
        .clone();
    let position = attendees
        .iter()
        .position(|a| {
            a["email"]
                .as_str()
                .is_some_and(|s| s.eq_ignore_ascii_case(email))
        })
        .ok_or_else(|| Reply::error(400, "invalidAttendee"))?;
    if attendees[position]["self"] != true && !email.eq_ignore_ascii_case(account) {
        return Err(Reply::error(403, "forbidden"));
    }
    for key in ["responseStatus", "comment"] {
        if let Some(value) = change.get(key) {
            attendees[position][key] = value.clone();
        }
    }
    body["attendees"] = Value::Array(attendees);
    if let Some(object) = body.as_object_mut() {
        object.remove("attendeesOmitted");
    }
    Ok(())
}
fn merge(target: &mut Value, patch: Value) {
    if let (Some(target), Some(patch)) = (target.as_object_mut(), patch.as_object()) {
        for (key, value) in patch {
            if value.is_null() {
                target.remove(key);
            } else if value.is_object() {
                merge(
                    target.entry(key.clone()).or_insert(json!({})),
                    value.clone(),
                );
            } else {
                target.insert(key.clone(), value.clone());
            }
        }
    } else {
        *target = patch;
    }
}
fn validate_patch(body: &Value) -> Result<()> {
    let fields = body
        .as_object()
        .ok_or_else(|| Reply::error(400, "invalidArgument"))?;
    for (key, value) in fields {
        let valid = match key.as_str() {
            "id" | "summary" | "description" | "location" | "status" | "transparency"
            | "visibility" | "colorId" => value.is_null() || value.is_string(),
            "start" | "end" | "originalStartTime" | "reminders" | "extendedProperties" => {
                value.is_null() || value.is_object()
            }
            "recurrence" | "attendees" => value.is_null() || value.is_array(),
            "sequence" => value.is_null() || value.is_u64(),
            "attendeesOmitted"
            | "guestsCanModify"
            | "guestsCanInviteOthers"
            | "guestsCanSeeOtherGuests" => value.is_null() || value.is_boolean(),
            _ => false,
        };
        if !valid {
            return Err(Reply::error(400, "invalidArgument"));
        }
    }
    validate_nested(body, true)?;
    Ok(())
}
fn validate_event(body: &Value) -> Result<()> {
    validate_nested(body, false)?;
    for name in ["start", "end"] {
        let time = body[name]
            .as_object()
            .ok_or_else(|| Reply::error(400, "invalidTime"))?;
        if time
            .keys()
            .any(|k| !["date", "dateTime", "timeZone"].contains(&k.as_str()))
            || time.contains_key("date") == time.contains_key("dateTime")
        {
            return Err(Reply::error(400, "invalidTime"));
        }
        if let Some(zone) = time.get("timeZone") {
            zone.as_str()
                .ok_or_else(|| Reply::error(400, "invalidTimeZone"))?
                .parse::<chrono_tz::Tz>()
                .map_err(|_| Reply::error(400, "invalidTimeZone"))?;
        }
    }
    if body["start"].get("date").is_some() != body["end"].get("date").is_some()
        || instant(&body["start"], "America/Chicago")? >= instant(&body["end"], "America/Chicago")?
    {
        return Err(Reply::error(400, "invalidTimeRange"));
    }
    if body.get("recurrence").is_some() {
        let _ = calendar_recurrence::rules(body)?;
    }
    if let Some(attendees) = body.get("attendees") {
        for attendee in attendees
            .as_array()
            .ok_or_else(|| Reply::error(400, "invalidArgument"))?
        {
            if !attendee["email"]
                .as_str()
                .is_some_and(|s| s.contains('@') && !s.chars().any(char::is_control))
                || attendee.get("responseStatus").is_some_and(|s| {
                    !["needsAction", "declined", "tentative", "accepted"]
                        .iter()
                        .any(|allowed| s == allowed)
                })
            {
                return Err(Reply::error(400, "invalidArgument"));
            }
        }
    }
    if body.get("status").is_some_and(|s| {
        !["confirmed", "tentative", "cancelled"]
            .iter()
            .any(|valid| s == valid)
    }) {
        return Err(Reply::error(400, "invalidArgument"));
    }
    Ok(())
}
fn validate_nested(body: &Value, request: bool) -> Result<()> {
    if let Some(reminders) = body.get("reminders").filter(|v| !request || !v.is_null()) {
        let object = reminders
            .as_object()
            .ok_or_else(|| Reply::error(400, "invalidArgument"))?;
        if object
            .keys()
            .any(|key| !["useDefault", "overrides"].contains(&key.as_str()))
            || object
                .get("useDefault")
                .is_some_and(|value| !value.is_boolean())
        {
            return Err(Reply::error(400, "invalidArgument"));
        }
        if let Some(overrides) = object.get("overrides") {
            let overrides = overrides
                .as_array()
                .filter(|v| v.len() <= 5)
                .ok_or_else(|| Reply::error(400, "invalidArgument"))?;
            for value in overrides {
                if value.as_object().is_none_or(|v| v.len() != 2)
                    || !["email", "popup"]
                        .iter()
                        .any(|method| value["method"] == *method)
                    || !value["minutes"].as_u64().is_some_and(|n| n <= 40320)
                {
                    return Err(Reply::error(400, "invalidArgument"));
                }
            }
        }
    }
    if let Some(properties) = body
        .get("extendedProperties")
        .filter(|v| !request || !v.is_null())
    {
        let object = properties
            .as_object()
            .ok_or_else(|| Reply::error(400, "invalidArgument"))?;
        if object.iter().any(|(key, value)| {
            !["private", "shared"].contains(&key.as_str())
                || value
                    .as_object()
                    .is_none_or(|v| v.values().any(|item| !item.is_string()))
        }) {
            return Err(Reply::error(400, "invalidArgument"));
        }
    }
    if let Some(attendees) = body.get("attendees").filter(|v| !request || !v.is_null()) {
        for attendee in attendees
            .as_array()
            .ok_or_else(|| Reply::error(400, "invalidArgument"))?
        {
            let object = attendee
                .as_object()
                .ok_or_else(|| Reply::error(400, "invalidArgument"))?;
            if object.iter().any(|(key, value)| match key.as_str() {
                "email" | "displayName" | "responseStatus" | "comment" => !value.is_string(),
                "optional" | "resource" | "self" | "organizer" => !value.is_boolean(),
                "additionalGuests" => !value.is_u64(),
                _ => request,
            }) {
                return Err(Reply::error(400, "invalidArgument"));
            }
        }
    }
    Ok(())
}
fn free_busy(model: &Model, input: &Input, account: &str) -> Result<Reply> {
    input.query.validate(&[], &[])?;
    let body: Value = input.json()?;
    let object = body
        .as_object()
        .ok_or_else(|| Reply::error(400, "invalidArgument"))?;
    if object
        .keys()
        .any(|k| !["timeMin", "timeMax", "timeZone", "items"].contains(&k.as_str()))
    {
        return Err(Reply::error(400, "invalidArgument"));
    }
    let lower = timestamp(
        body["timeMin"]
            .as_str()
            .ok_or_else(|| Reply::error(400, "invalidTime"))?,
    )?;
    let upper = timestamp(
        body["timeMax"]
            .as_str()
            .ok_or_else(|| Reply::error(400, "invalidTime"))?,
    )?;
    if lower >= upper {
        return Err(Reply::error(400, "invalidTimeRange"));
    }
    let items = body["items"]
        .as_array()
        .filter(|items| items.len() <= 50)
        .ok_or_else(|| Reply::error(400, "invalidArgument"))?;
    let zone = body
        .get("timeZone")
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| Reply::error(400, "invalidTimeZone"))
        })
        .transpose()?
        .unwrap_or("UTC")
        .parse::<chrono_tz::Tz>()
        .map_err(|_| Reply::error(400, "invalidTimeZone"))?;
    let mut calendars = json!({});
    for item in items {
        if item.as_object().is_none_or(|o| o.len() != 1) {
            return Err(Reply::error(400, "invalidArgument"));
        }
        let id = item["id"]
            .as_str()
            .ok_or_else(|| Reply::error(400, "invalidArgument"))?;
        let resolved = if id == "primary" { account } else { id };
        match model.calendar(account, resolved) {
            Ok(calendar) => {
                let busy=calendar_recurrence::expanded(calendar,lower,upper,None,false)?.into_iter()
                    .filter(|event|event["transparency"]!="transparent")
                    .map(|event|Ok(json!({"start":instant(&event["start"],"America/Chicago")?.max(lower).with_timezone(&zone).to_rfc3339(),"end":instant(&event["end"],"America/Chicago")?.min(upper).with_timezone(&zone).to_rfc3339()})))
                    .collect::<Result<Vec<_>>>()?;
                calendars[id] = json!({"busy":busy});
            }
            Err(_) => calendars[id] = json!({"errors":[{"domain":"global","reason":"notFound"}]}),
        }
    }
    Ok(Reply::json(
        json!({"kind":"calendar#freeBusy","timeMin":body["timeMin"],"timeMax":body["timeMax"],"calendars":calendars}),
    ))
}
