#![allow(clippy::unwrap_used)]
use nuncio_engine::domain::calendar_change::CalendarAction;
use serde_json::{json, Value};

fn event() -> Value {
    json!({"id":"event01","etag":"\"version1\"","summary":"Keep title","start":{"dateTime":"2026-10-01T10:00:00-05:00","timeZone":"America/Chicago"},"end":{"dateTime":"2026-10-01T11:00:00-05:00","timeZone":"America/Chicago"},"organizer":{"email":"organizer@example.test","self":false},"attendees":[{"email":"ALPHA@example.test","self":true,"responseStatus":"needsAction","displayName":"Alpha","x-provider":"retain"},{"email":"other@example.test","responseStatus":"accepted","comment":"Retain me"}],"extendedProperties":{"private":{"unknown":"retain"}},"unknown_provider":{"keep":true}})
}
#[test]
fn limited_writer_can_write_visible_events_but_cannot_modify_private_events() {
    let base = event();
    let mut private = base.clone();
    private["visibility"] = json!("private");
    for action in [
        json!({"action":"update","event_id":"local","expected_etag":"\"version1\"","scope":"single","notifications":"all","patch":{"summary":"New"}}),
        json!({"action":"delete","event_id":"local","expected_etag":"\"version1\"","scope":"single","notifications":"all"}),
        json!({"action":"respond","event_id":"local","expected_etag":"\"version1\"","scope":"single","notifications":"all","response":"accepted"}),
    ] {
        let action = CalendarAction::from_value(action, "America/Chicago").unwrap();
        assert!(action
            .provider_patch(
                Some(&base),
                "America/Chicago",
                "alpha@example.test",
                "writerWithoutPrivateAccess"
            )
            .is_ok());
        assert!(matches!(
            action.provider_patch(
                Some(&private),
                "America/Chicago",
                "alpha@example.test",
                "writerWithoutPrivateAccess"
            ),
            Err(nuncio_engine::domain::calendar_change::ChangeError::Permission)
        ));
        assert!(action
            .provider_patch(
                Some(&private),
                "America/Chicago",
                "alpha@example.test",
                "writer"
            )
            .is_ok());
    }
    let create = CalendarAction::from_value(json!({"action":"create","scope":"single","notifications":"none","event":{"summary":"New","start":{"date":"2026-10-01"},"end":{"date":"2026-10-02"}}}), "America/Chicago").unwrap();
    assert!(create
        .provider_patch(
            None,
            "America/Chicago",
            "alpha@example.test",
            "writerWithoutPrivateAccess"
        )
        .is_ok());
}
#[test]
fn calendar_actions_require_scope_notification_policy_and_bounded_named_fields() {
    for value in [
        json!({"action":"update","event_id":"local","expected_etag":"\"v1\"","patch":{"summary":"New"},"notifications":"none"}),
        json!({"action":"update","event_id":"local","expected_etag":"\"v1\"","patch":{"summary":"New"},"scope":"single"}),
        json!({"action":"delete","event_id":"local","expected_etag":"\"v1\"","scope":"following","notifications":"none"}),
        json!({"action":"update","event_id":"local","expected_etag":"\"v1\"","scope":"single","notifications":"none","patch":{"organizer":{"email":"intruder@example.test"}}}),
        json!({"action":"update","event_id":"local","expected_etag":"\"v1\"","scope":"single","notifications":"none","patch":{}}),
        json!({"action":"delete","event_id":"local","expected_etag":"\"v1\"\r\nInjected: yes","scope":"single","notifications":"all"}),
        json!({"action":"respond","event_id":"local","expected_etag":"\"v1\"","scope":"single","notifications":"all","response":"maybe"}),
        json!({"action":"create","scope":"single","notifications":"none","event":{"summary":"A","start":{"date":"2026-10-01"},"end":{"date":"2026-10-01"}}}),
        json!({"action":"create","scope":"single","notifications":"none","event":{"id":"not-client-owned","start":{"date":"2026-10-01"},"end":{"date":"2026-10-02"}}}),
    ] {
        assert!(
            CalendarAction::from_value(value.clone(), "America/Chicago").is_err(),
            "accepted {value}"
        );
    }
    let action=CalendarAction::from_value(json!({"action":"update","event_id":"local","expected_etag":"\"version1\"","scope":"single","notifications":"external_only","patch":{"summary":"New"}}),"America/Chicago").unwrap();
    let patch = action
        .provider_patch(
            Some(&event()),
            "America/Chicago",
            "alpha@example.test",
            "owner",
        )
        .unwrap();
    assert_eq!(patch, json!({"summary":"New"}));
    assert_eq!(action.notification_parameter(), "externalOnly");
    assert!(action
        .provider_patch(
            Some(&event()),
            "America/Chicago",
            "alpha@example.test",
            "reader"
        )
        .is_err());
    let oversized = "a".repeat(65537);
    assert!(CalendarAction::from_value(json!({"action":"update","event_id":"local","expected_etag":"\"v1\"","scope":"single","notifications":"none","patch":{"summary":oversized}}),"America/Chicago").is_err());
}
#[test]
fn rsvp_uses_partial_response_and_recurrence_scope_is_explicit() {
    let response=CalendarAction::from_value(json!({"action":"respond","event_id":"local","expected_etag":"\"version1\"","scope":"single","notifications":"all","response":"accepted","comment":"I will attend"}),"America/Chicago").unwrap();
    let original = event();
    let patch = response
        .provider_patch(
            Some(&original),
            "America/Chicago",
            "alpha@example.test",
            "owner",
        )
        .unwrap();
    assert_eq!(
        patch,
        json!({"attendeesOmitted":true,"attendees":[{"email":"ALPHA@example.test","responseStatus":"accepted","comment":"I will attend"}]})
    );
    assert!(response
        .provider_patch(
            Some(&original),
            "America/Chicago",
            "missing@example.test",
            "owner"
        )
        .is_err());
    let mut omitted = original.clone();
    omitted["attendeesOmitted"] = json!(true);
    assert_eq!(
        response
            .provider_patch(
                Some(&omitted),
                "America/Chicago",
                "alpha@example.test",
                "owner"
            )
            .unwrap(),
        patch
    );
    let mut ambiguous = original.clone();
    ambiguous["attendees"]
        .as_array_mut()
        .unwrap()
        .push(original["attendees"][0].clone());
    assert!(response
        .provider_patch(
            Some(&ambiguous),
            "America/Chicago",
            "alpha@example.test",
            "owner"
        )
        .is_err());
    let mut master = original.clone();
    master["recurrence"] = json!(["RRULE:FREQ=WEEKLY;COUNT=3"]);
    assert!(
        response
            .provider_patch(
                Some(&master),
                "America/Chicago",
                "alpha@example.test",
                "owner"
            )
            .is_err(),
        "single action must not silently change a recurring master"
    );
    let series=CalendarAction::from_value(json!({"action":"update","event_id":"master-local","expected_etag":"\"version1\"","scope":"series","notifications":"none","patch":{"summary":"All occurrences"}}),"America/Chicago").unwrap();
    assert_eq!(
        series
            .provider_patch(
                Some(&master),
                "America/Chicago",
                "alpha@example.test",
                "owner"
            )
            .unwrap(),
        json!({"summary":"All occurrences"})
    );
    assert!(series
        .provider_patch(
            Some(&original),
            "America/Chicago",
            "alpha@example.test",
            "owner"
        )
        .is_err());
    let mut instance = original.clone();
    instance["recurringEventId"] = json!("master01");
    instance["originalStartTime"] = instance["start"].clone();
    assert!(response
        .provider_patch(
            Some(&instance),
            "America/Chicago",
            "alpha@example.test",
            "owner"
        )
        .is_ok());
    let mixed=CalendarAction::from_value(json!({"action":"update","event_id":"local","expected_etag":"\"version1\"","scope":"single","notifications":"none","patch":{"start":{"date":"2026-10-01"}}}),"America/Chicago").unwrap();
    assert!(mixed
        .provider_patch(
            Some(&original),
            "America/Chicago",
            "alpha@example.test",
            "owner"
        )
        .is_err());
}

#[test]
fn explicit_nonorganizer_flag_is_authoritative_for_guest_controls() {
    let mut base = event();
    base["organizer"]["email"] = json!("alpha@example.test");
    let action=CalendarAction::from_value(json!({"action":"update","event_id":"local","expected_etag":"\"version1\"","scope":"single","notifications":"all","patch":{"guestsCanModify":true}}),"America/Chicago").unwrap();
    assert!(action
        .provider_patch(
            Some(&base),
            "America/Chicago",
            "alpha@example.test",
            "owner"
        )
        .is_err());
}
