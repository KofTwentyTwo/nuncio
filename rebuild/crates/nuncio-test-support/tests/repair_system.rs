#![allow(clippy::unwrap_used)]
#[allow(dead_code)]
mod support;
use nuncio_proto::v2::*;
use nuncio_test_support::google::{Fault, FaultAction, Phase, Seed};
use serde_json::json;
use support::{
    auth::{begin, finish},
    system::SystemHarness,
};
fn window() -> AgendaWindow {
    AgendaWindow {
        from: "2026-03-01".into(),
        to: "2026-03-20".into(),
    }
}
fn request(account: &str, scope: ProjectionScope, dry_run: bool) -> RepairProjectionRequest {
    RepairProjectionRequest {
        account_id: account.into(),
        scope: scope.into(),
        dry_run,
        window: (scope == ProjectionScope::Calendar).then(window),
    }
}
async fn settled(h: &SystemHarness, mut run: SyncRun) -> SyncRun {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while matches!(run.state.as_str(), "queued" | "running") {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            run = h
                .authenticated()
                .get_sync_run(SyncRunRequest {
                    account_id: run.account_id.clone(),
                    run_id: run.id,
                })
                .await
                .unwrap()
                .into_inner();
        }
        run
    })
    .await
    .unwrap()
}
async fn agenda(h: &SystemHarness, account: &str) -> ListAgendaResponse {
    h.calendar()
        .list_agenda(ListAgendaRequest {
            account_id: account.into(),
            window: Some(window()),
            page_size: 100,
            page_token: None,
        })
        .await
        .unwrap()
        .into_inner()
}

#[tokio::test]
async fn calendar_repair_admission_cancellation_and_missing_permissions_preserve_honest_coverage() {
    let mut h = SystemHarness::start(Seed::TwoAccounts).await.unwrap();
    let account = finish(&h, &begin(&h, "alpha@example.test", None).await, 200)
        .await
        .account_id
        .unwrap();
    let q = request(&account, ProjectionScope::Calendar, false);
    let initial = h
        .maintenance()
        .repair_projection(q.clone())
        .await
        .unwrap()
        .into_inner()
        .run
        .unwrap();
    assert_eq!(settled(&h, initial).await.state, "succeeded");
    let before = agenda(&h, &account).await;
    h.arm("calendar-repair-before-promotion").unwrap();
    let run = h
        .maintenance()
        .repair_projection(q.clone())
        .await
        .unwrap()
        .into_inner()
        .run
        .unwrap();
    h.wait("calendar-repair-before-promotion").await.unwrap();
    let joined = h
        .maintenance()
        .repair_projection(q.clone())
        .await
        .unwrap()
        .into_inner()
        .run
        .unwrap();
    assert_eq!(joined.id, run.id);
    let mut other = q.clone();
    other.window.as_mut().unwrap().to = "2026-03-21".into();
    assert_eq!(
        h.maintenance()
            .repair_projection(other)
            .await
            .unwrap_err()
            .code(),
        tonic::Code::FailedPrecondition
    );
    let cancelled = h
        .authenticated()
        .cancel_sync(SyncRunRequest {
            account_id: account.clone(),
            run_id: run.id,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(cancelled.state, "cancelled");
    let after = agenda(&h, &account).await;
    assert_eq!(after.items, before.items);
    assert_eq!(after.coverage, before.coverage);
    h.google
        .control()
        .inject(Fault {
            method: "GET".into(),
            path: "/calendar/v3/users/me/calendarList".into(),
            account: Some("alpha@example.test".into()),
            call: None,
            phase: Phase::After,
            action: FaultAction::Withhold {
                barrier: "ordinary-calendar-list".into(),
            },
        })
        .await;
    let ordinary = h
        .calendar()
        .refresh_agenda(RefreshAgendaRequest {
            account_id: account.clone(),
            window: Some(window()),
        })
        .await
        .unwrap()
        .into_inner();
    h.google
        .control()
        .wait_for_barrier("ordinary-calendar-list")
        .await
        .unwrap();
    assert_eq!(
        h.maintenance()
            .repair_projection(q.clone())
            .await
            .unwrap_err()
            .code(),
        tonic::Code::FailedPrecondition
    );
    assert_eq!(
        h.authenticated()
            .cancel_sync(SyncRunRequest {
                account_id: account.clone(),
                run_id: ordinary.id
            })
            .await
            .unwrap()
            .into_inner()
            .state,
        "cancelled"
    );
    h.google
        .control()
        .release_barrier("ordinary-calendar-list")
        .await;
    h.google
        .control()
        .omit_calendar_fields(
            "alpha@example.test",
            "team-alpha@example.test",
            &["accessRole"],
        )
        .await
        .unwrap();
    let remote = h.google.control().snapshot().await;
    let repaired = h
        .maintenance()
        .repair_projection(q.clone())
        .await
        .unwrap()
        .into_inner()
        .run
        .unwrap();
    assert_eq!(settled(&h, repaired).await.state, "succeeded");
    let catalogue = h
        .calendar()
        .list_calendars(ListCalendarsRequest {
            account_id: account.clone(),
            page_size: 100,
            page_token: None,
        })
        .await
        .unwrap()
        .into_inner();
    let team = catalogue
        .items
        .iter()
        .find(|c| c.provider_id == "team-alpha@example.test")
        .unwrap();
    assert_eq!(team.access_role, "none");
    let after = agenda(&h, &account).await;
    assert_eq!(
        after
            .coverage
            .iter()
            .find(|c| c.calendar_id == team.id)
            .unwrap()
            .state,
        "unavailable"
    );
    assert_eq!(
        after
            .items
            .iter()
            .filter(|e| e.calendar_id == team.id)
            .collect::<Vec<_>>(),
        before
            .items
            .iter()
            .filter(|e| e.calendar_id == team.id)
            .collect::<Vec<_>>()
    );
    let now = h.google.control().snapshot().await;
    assert_eq!(
        serde_json::to_value(&now.calendars).unwrap(),
        serde_json::to_value(&remote.calendars).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&now.mail).unwrap(),
        serde_json::to_value(&remote.mail).unwrap()
    );
    h.google
        .control()
        .omit_calendar_fields("alpha@example.test", "team-alpha@example.test", &[])
        .await
        .unwrap();
    let repaired = h
        .maintenance()
        .repair_projection(q)
        .await
        .unwrap()
        .into_inner()
        .run
        .unwrap();
    assert_eq!(settled(&h, repaired).await.state, "succeeded");
    let after = agenda(&h, &account).await;
    assert!(after.coverage.iter().all(|c| c.state == "current"));
    assert!(h
        .google
        .control()
        .accepted_sends("alpha@example.test")
        .await
        .is_empty());
    h.shutdown().await.unwrap();
}

#[tokio::test]
async fn repair_preview_is_local_authenticated_and_calendar_failure_preserves_the_complete_projection(
) {
    let mut h = SystemHarness::start(Seed::TwoAccounts).await.unwrap();
    h.google.control().set_page_cap(1).await.unwrap();
    assert_eq!(
        h.anonymous_maintenance()
            .repair_projection(request("missing", ProjectionScope::Mail, true))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    let account = finish(&h, &begin(&h, "alpha@example.test", None).await, 200)
        .await
        .account_id
        .unwrap();
    for scope in [ProjectionScope::Mail, ProjectionScope::Calendar] {
        let response = h
            .maintenance()
            .repair_projection(request(&account, scope, false))
            .await
            .unwrap()
            .into_inner();
        assert!(!response.dry_run);
        assert_eq!(response.scope, scope as i32);
        assert_eq!(settled(&h, response.run.unwrap()).await.state, "succeeded");
    }
    let draft = h
        .mail()
        .save_draft(SaveDraftRequest {
            account_id: account.clone(),
            draft_id: None,
            expected_version: None,
            content: Some(DraftContent {
                subject: "Repair must preserve local intent".into(),
                text: Some("Saved draft body".into()),
                ..Default::default()
            }),
        })
        .await
        .unwrap()
        .into_inner();
    let before = agenda(&h, &account).await;
    let requests = serde_json::to_value(h.google.control().snapshot().await.requests).unwrap();
    let revision = h
        .authenticated()
        .get_status(GetStatusRequest {})
        .await
        .unwrap()
        .into_inner()
        .storage
        .unwrap()
        .revision;
    for scope in [ProjectionScope::Mail, ProjectionScope::Calendar] {
        let preview = h
            .maintenance()
            .repair_projection(request(&account, scope, true))
            .await
            .unwrap()
            .into_inner();
        assert!(preview.dry_run);
        assert!(preview.run.is_none());
        assert_eq!(preview.revision, revision);
        assert_eq!(preview.preserved.unwrap().drafts, 1);
        let counts = preview.projection.unwrap();
        if scope == ProjectionScope::Mail {
            assert_eq!(counts.messages, 3);
            assert_eq!(counts.calendars, 0);
        } else {
            assert_eq!(counts.calendars, 2);
            assert!(counts.events > 0);
            assert_eq!(counts.messages, 0);
        }
    }
    assert_eq!(
        serde_json::to_value(h.google.control().snapshot().await.requests).unwrap(),
        requests
    );
    assert_eq!(
        h.authenticated()
            .get_status(GetStatusRequest {})
            .await
            .unwrap()
            .into_inner()
            .storage
            .unwrap()
            .revision,
        revision
    );
    h.google.control().put_event("alpha@example.test","primary",json!({"id":"alldate01","summary":"Remote repair edit","status":"confirmed","start":{"date":"2026-03-10"},"end":{"date":"2026-03-11"}})).await.unwrap();
    let remote = h.google.control().snapshot().await;
    h.google
        .control()
        .inject(Fault {
            method: "GET".into(),
            path: "/calendar/v3/calendars/team-alpha@example.test/events".into(),
            account: Some("alpha@example.test".into()),
            call: None,
            phase: Phase::After,
            action: FaultAction::MalformedJson,
        })
        .await;
    let response = h
        .maintenance()
        .repair_projection(request(&account, ProjectionScope::Calendar, false))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(settled(&h, response.run.unwrap()).await.state, "failed");
    let after = agenda(&h, &account).await;
    assert_eq!(after.items, before.items);
    assert_eq!(after.coverage, before.coverage);
    let response = h
        .maintenance()
        .repair_projection(request(&account, ProjectionScope::Calendar, false))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(settled(&h, response.run.unwrap()).await.state, "succeeded");
    let after = agenda(&h, &account).await;
    assert_eq!(
        after
            .items
            .iter()
            .find(|v| v.provider_id == "alldate01")
            .unwrap()
            .summary
            .as_deref(),
        Some("Remote repair edit")
    );
    assert_eq!(
        h.mail()
            .get_draft(DraftRequest {
                account_id: account.clone(),
                draft_id: draft.id.clone()
            })
            .await
            .unwrap()
            .into_inner(),
        draft
    );
    let after_remote = h.google.control().snapshot().await;
    assert_eq!(
        serde_json::to_value(after_remote.calendars).unwrap(),
        serde_json::to_value(remote.calendars).unwrap()
    );
    assert_eq!(
        serde_json::to_value(after_remote.mail).unwrap(),
        serde_json::to_value(remote.mail).unwrap()
    );
    assert!(h
        .google
        .control()
        .accepted_sends("alpha@example.test")
        .await
        .is_empty());
    h.shutdown().await.unwrap();
}
