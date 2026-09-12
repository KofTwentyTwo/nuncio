use super::*;

#[tokio::test]
async fn limited_writer_edits_public_events_but_private_details_and_writes_are_denied() {
    let mock = MockGoogle::start(Seed::TwoAccounts).await.unwrap();
    let auth = login(&mock, "alpha@example.test").await;
    let token = auth["access_token"].as_str().unwrap();
    let calendar = "team-alpha@example.test";
    let path = format!("/calendar/v3/calendars/{calendar}/events");
    let private = serde_json::json!({"id":"private001","visibility":"private","summary":"Private title canary","description":"Private description canary","location":"Private room","start":{"date":"2026-10-01"},"end":{"date":"2026-10-02"},"recurrence":["RRULE:FREQ=DAILY;COUNT=2"],"attendees":[{"email":"secret@example.test"}],"extendedProperties":{"private":{"secret":"hidden"}}});
    mock.control()
        .put_event("alpha@example.test", calendar, private)
        .await
        .unwrap();
    let original =
        mock.control().snapshot().await.calendars["alpha@example.test"][calendar].clone();
    for role in ["writerWithoutPrivateAccess", "reader"] {
        mock.control()
            .set_calendar_role("alpha@example.test", calendar, role)
            .await
            .unwrap();
        let detail = json_ok(get(&mock, token, &format!("{path}/private001")).await).await;
        let list = json_ok(
            get(
                &mock,
                token,
                &format!("{path}?maxResults=2500&showDeleted=true"),
            )
            .await,
        )
        .await;
        let instances = json_ok(get(&mock, token, &format!("{path}/private001/instances?timeMin=2026-10-01T00:00:00Z&timeMax=2026-10-04T00:00:00Z")).await).await;
        let listed = list["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == "private001")
            .unwrap();
        assert_eq!(instances["items"].as_array().unwrap().len(), 2);
        for event in std::iter::once(&detail)
            .chain(std::iter::once(listed))
            .chain(instances["items"].as_array().unwrap())
        {
            for field in [
                "summary",
                "description",
                "location",
                "attendees",
                "extendedProperties",
            ] {
                assert!(
                    event.get(field).is_none(),
                    "{role} disclosed {field}: {event}"
                );
            }
            assert_eq!(event["visibility"], "private");
            assert!(event["start"].is_object());
            assert!(event["end"].is_object());
        }
        for method in [reqwest::Method::PATCH, reqwest::Method::DELETE] {
            let mut request = client()
                .request(
                    method.clone(),
                    format!("{}{path}/private001?sendUpdates=all", mock.base_url()),
                )
                .bearer_auth(token)
                .header("If-Match", detail["etag"].as_str().unwrap());
            if method == reqwest::Method::PATCH {
                request = request.json(&serde_json::json!({"summary":"Forbidden"}));
            }
            assert_eq!(request.send().await.unwrap().status(), 403);
        }
        let after =
            mock.control().snapshot().await.calendars["alpha@example.test"][calendar].clone();
        assert_eq!(after.events, original.events);
        assert_eq!(after.version, original.version);
        assert_eq!(after.notifications.len(), original.notifications.len());
    }
    mock.control()
        .set_calendar_role("alpha@example.test", calendar, "writerWithoutPrivateAccess")
        .await
        .unwrap();
    let create = serde_json::json!({"id":"public001","summary":"Visible","start":{"date":"2026-10-03"},"end":{"date":"2026-10-04"},"attendees":[{"email":"recipient@example.test"}]});
    let created = json_ok(
        client()
            .post(format!("{}{path}?sendUpdates=all", mock.base_url()))
            .bearer_auth(token)
            .json(&create)
            .send()
            .await
            .unwrap(),
    )
    .await;
    let edited = json_ok(
        client()
            .patch(format!(
                "{}{path}/public001?sendUpdates=all",
                mock.base_url()
            ))
            .bearer_auth(token)
            .header("If-Match", created["etag"].as_str().unwrap())
            .json(&serde_json::json!({"summary":"Edited"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(edited["summary"], "Edited");
    assert_eq!(
        client()
            .delete(format!(
                "{}{path}/public001?sendUpdates=all",
                mock.base_url()
            ))
            .bearer_auth(token)
            .header("If-Match", edited["etag"].as_str().unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    let after = mock.control().snapshot().await.calendars["alpha@example.test"][calendar].clone();
    assert_eq!(after.events["private001"], original.events["private001"]);
    assert_eq!(after.events["public001"]["status"], "cancelled");
    assert_eq!(after.notifications.len(), original.notifications.len() + 3);
    for role in ["writer", "owner"] {
        mock.control()
            .set_calendar_role("alpha@example.test", calendar, role)
            .await
            .unwrap();
        let visible = json_ok(get(&mock, token, &format!("{path}/private001")).await).await;
        assert_eq!(visible, original.events["private001"]);
    }
    mock.shutdown().await.unwrap();
}

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
