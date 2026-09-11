use super::*;

#[tokio::test]
async fn calendar_permissions_and_nonorganizer_shared_flags_are_enforced_independently() {
    let mock = MockGoogle::start(Seed::TwoAccounts).await.unwrap();
    let auth = login(&mock, "alpha@example.test").await;
    let token = auth["access_token"].as_str().unwrap();
    let calendar = "team-alpha@example.test";
    let event = serde_json::json!({"id":"permissions01","summary":"Unchanged","start":{"date":"2026-10-01"},"end":{"date":"2026-10-02"},"organizer":{"email":"organizer@example.test","self":false},"attendees":[{"email":"alpha@example.test","self":true,"responseStatus":"needsAction","x-provider":{"retain":true}},{"email":"other@example.test","responseStatus":"accepted"}],"extendedProperties":{"private":{"unknown":"retain"}}});
    mock.control()
        .put_event("alpha@example.test", calendar, event)
        .await
        .unwrap();
    let path = format!("/calendar/v3/calendars/{calendar}/events/permissions01");
    let original = json_ok(get(&mock, token, &path).await).await;
    let before = mock.control().snapshot().await.calendars["alpha@example.test"][calendar].clone();
    for property in [
        "guestsCanInviteOthers",
        "guestsCanModify",
        "guestsCanSeeOtherGuests",
    ] {
        let denied = client()
            .patch(format!("{}{path}?sendUpdates=all", mock.base_url()))
            .bearer_auth(token)
            .header("If-Match", original["etag"].as_str().unwrap())
            .json(&serde_json::json!({property:false}))
            .send()
            .await
            .unwrap();
        assert_eq!(
            denied.status(),
            403,
            "non-organizer must not set shared guest permissions"
        );
        assert_eq!(
            denied.json::<Value>().await.unwrap()["error"]["errors"][0]["reason"],
            "forbiddenForNonOrganizer"
        );
    }
    let after = mock.control().snapshot().await.calendars["alpha@example.test"][calendar].clone();
    assert_eq!(after.events, before.events);
    assert_eq!(after.version, before.version);
    assert_eq!(after.notifications.len(), before.notifications.len());
    // Attendee copies may change local shared content; do not invent a blanket
    // organizer-only restriction stronger than Google's documented behavior.
    let private_copy = client()
        .patch(format!("{}{path}?sendUpdates=none", mock.base_url()))
        .bearer_auth(token)
        .header("If-Match", original["etag"].as_str().unwrap())
        .json(&serde_json::json!({"description":"My attendee copy"}))
        .send()
        .await
        .unwrap();
    let updated = json_ok(private_copy).await;
    for (body, status) in [
        (
            serde_json::json!({"attendeesOmitted":true,"attendees":[]}),
            400,
        ),
        (
            serde_json::json!({"attendeesOmitted":true,"attendees":[{"email":"other@example.test","responseStatus":"declined"}]}),
            403,
        ),
        (
            serde_json::json!({"attendeesOmitted":true,"attendees":[{"email":"missing@example.test","responseStatus":"accepted"}]}),
            400,
        ),
        (
            serde_json::json!({"attendeesOmitted":true,"attendees":[{"email":"alpha@example.test","responseStatus":"maybe"}]}),
            400,
        ),
        (
            serde_json::json!({"attendeesOmitted":true,"attendees":[{"email":"alpha@example.test","displayName":"Overwrite"}]}),
            400,
        ),
        (
            serde_json::json!({"attendees":[{"email":"alpha@example.test","x-request":"must reject"}]}),
            400,
        ),
    ] {
        let invalid = client()
            .patch(format!("{}{path}?sendUpdates=all", mock.base_url()))
            .bearer_auth(token)
            .header("If-Match", updated["etag"].as_str().unwrap())
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(invalid.status(), status, "accepted invalid patch {body}");
        let observed =
            mock.control().snapshot().await.calendars["alpha@example.test"][calendar].clone();
        assert_eq!(observed.events["permissions01"], updated);
        assert_eq!(observed.notifications.len(), before.notifications.len());
    }
    let response=client().patch(format!("{}{path}?sendUpdates=none",mock.base_url())).bearer_auth(token).header("If-Match",updated["etag"].as_str().unwrap()).json(&serde_json::json!({"attendeesOmitted":true,"attendees":[{"email":"alpha@example.test","responseStatus":"accepted","comment":"Attending"}]})).send().await.unwrap();
    let responded = json_ok(response).await;
    assert_eq!(responded["attendees"][0]["responseStatus"], "accepted");
    assert_eq!(responded["attendees"][1], original["attendees"][1]);
    assert_eq!(
        responded["attendees"][0]["x-provider"],
        original["attendees"][0]["x-provider"]
    );
    assert_eq!(
        responded["extendedProperties"],
        original["extendedProperties"]
    );
    mock.control()
        .set_calendar_role("alpha@example.test", calendar, "reader")
        .await
        .unwrap();
    let list = json_ok(get(&mock, token, "/calendar/v3/users/me/calendarList").await).await;
    assert_eq!(
        list["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == calendar)
            .unwrap()["accessRole"],
        "reader"
    );
    assert_eq!(get(&mock, token, &path).await.status(), 200);
    for method in [
        reqwest::Method::POST,
        reqwest::Method::PATCH,
        reqwest::Method::DELETE,
    ] {
        let route = if method == reqwest::Method::POST {
            format!("/calendar/v3/calendars/{calendar}/events")
        } else {
            path.clone()
        };
        let mut request = client()
            .request(
                method.clone(),
                format!("{}{route}?sendUpdates=all", mock.base_url()),
            )
            .bearer_auth(token);
        if method != reqwest::Method::DELETE {
            request=request.json(&serde_json::json!({"id":"denied001","start":{"date":"2026-10-01"},"end":{"date":"2026-10-02"}}));
        }
        assert_eq!(request.send().await.unwrap().status(), 403);
    }
    let final_state =
        mock.control().snapshot().await.calendars["alpha@example.test"][calendar].clone();
    assert_eq!(final_state.events["permissions01"], responded);
    assert!(!final_state.events.contains_key("denied001"));
    assert_eq!(final_state.notifications.len(), before.notifications.len());
    mock.shutdown().await.unwrap();
}
