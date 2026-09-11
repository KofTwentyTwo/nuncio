use super::{
    calendar_state::{instant, overlap, Calendar},
    wire::{Reply, Result},
};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};

pub(super) fn rules(event: &Value) -> Result<rrule::RRuleSet> {
    let zone = event["start"]["timeZone"]
        .as_str()
        .unwrap_or("America/Chicago");
    let zone_parsed = zone
        .parse::<chrono_tz::Tz>()
        .map_err(|_| Reply::error(400, "invalidTimeZone"))?;
    let start = instant(&event["start"], zone)?.with_timezone(&zone_parsed);
    let lines = event["recurrence"]
        .as_array()
        .ok_or_else(|| Reply::error(400, "invalidRecurrence"))?;
    let mut text = format!("DTSTART;TZID={zone}:{}", start.format("%Y%m%dT%H%M%S"));
    for line in lines {
        let line = line
            .as_str()
            .ok_or_else(|| Reply::error(400, "invalidRecurrence"))?;
        if !["RRULE:", "EXRULE:", "RDATE", "EXDATE"]
            .iter()
            .any(|prefix| line.starts_with(prefix))
            || line.contains(['\r', '\n'])
        {
            return Err(Reply::error(400, "invalidRecurrence"));
        }
        text.push('\n');
        text.push_str(line);
    }
    text.parse()
        .map_err(|_| Reply::error(400, "invalidRecurrence"))
}

pub(super) fn expanded(
    calendar: &Calendar,
    lower: DateTime<Utc>,
    upper: DateTime<Utc>,
    master_id: Option<&str>,
    show_deleted: bool,
) -> Result<Vec<Value>> {
    let mut items = Vec::new();
    for event in calendar.events.values() {
        let master = &event.body;
        if master.get("recurringEventId").is_some()
            || master_id.is_some_and(|id| master["id"] != id)
        {
            continue;
        }
        if master["status"] == "cancelled" {
            continue;
        }
        if master.get("recurrence").is_none() {
            if master_id.is_none() && overlap(master, lower, upper)? {
                items.push(master.clone());
            }
            continue;
        }
        let start = instant(&master["start"], "America/Chicago")?;
        let duration = instant(&master["end"], "America/Chicago")? - start;
        let rule = rules(master)?
            .after((lower - duration).with_timezone(&rrule::Tz::UTC))
            .before(upper.with_timezone(&rrule::Tz::UTC));
        let result = rule.all(10_000);
        if result.limited {
            return Err(Reply::error(503, "recurrenceLimit"));
        }
        for date in result.dates {
            let master_id = master["id"]
                .as_str()
                .ok_or_else(|| Reply::error(500, "invalidFixture"))?;
            let id = format!(
                "{master_id}_{}",
                date.with_timezone(&Utc).format("%Y%m%dT%H%M%SZ")
            );
            let mut instance = master.clone();
            if let Some(object) = instance.as_object_mut() {
                object.remove("recurrence");
            }
            instance["id"] = json!(id);
            instance["recurringEventId"] = json!(master_id);
            if master["start"].get("date").is_some() {
                let begin = chrono::NaiveDate::parse_from_str(
                    master["start"]["date"].as_str().unwrap_or(""),
                    "%Y-%m-%d",
                )
                .map_err(|_| Reply::error(400, "invalidTime"))?;
                let end = chrono::NaiveDate::parse_from_str(
                    master["end"]["date"].as_str().unwrap_or(""),
                    "%Y-%m-%d",
                )
                .map_err(|_| Reply::error(400, "invalidTime"))?;
                instance["start"] = json!({"date":date.date_naive().to_string()});
                instance["end"] = json!({"date":(date.date_naive()+(end-begin)).to_string()});
            } else {
                instance["start"]["dateTime"] = json!(date.to_rfc3339());
                instance["end"]["dateTime"] = json!((date + duration).to_rfc3339());
            }
            instance["originalStartTime"] = instance["start"].clone();
            if let Some(exception) = calendar.events.get(&id) {
                instance = exception.body.clone();
            }
            if (show_deleted || instance["status"] != "cancelled")
                && overlap(&instance, lower, upper)?
            {
                items.push(instance);
            }
        }
    }
    // A moved exception may enter the window while its original occurrence is outside it.
    for event in calendar.events.values() {
        let body = &event.body;
        if body.get("recurringEventId").is_some()
            && master_id.is_none_or(|id| body["recurringEventId"] == id)
            && (show_deleted || body["status"] != "cancelled")
            && overlap(body, lower, upper)?
            && !items.iter().any(|item| item["id"] == body["id"])
        {
            let parent = body["recurringEventId"]
                .as_str()
                .and_then(|id| calendar.events.get(id));
            if parent.is_some_and(|parent| parent.body["status"] != "cancelled") {
                items.push(body.clone());
            }
        }
    }
    Ok(items)
}
