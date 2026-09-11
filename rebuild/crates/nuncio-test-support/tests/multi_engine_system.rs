#![allow(clippy::unwrap_used)]
pub mod support;

use nuncio_proto::v2::*;
use nuncio_test_support::{
    google::{MockGoogle, Seed},
    TestError,
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};
use support::{
    auth::{begin, finish},
    system::SystemHarness,
};

const ADDRESS: &str = "alpha@example.test";
fn window() -> AgendaWindow {
    AgendaWindow {
        from: "2026-03-01".into(),
        to: "2026-03-20".into(),
    }
}
async fn complete(h: &SystemHarness, mut run: SyncRun) {
    tokio::time::timeout(Duration::from_secs(15), async {
        while matches!(run.state.as_str(), "queued" | "running") {
            tokio::time::sleep(Duration::from_millis(10)).await;
            run = h
                .authenticated()
                .get_sync_run(SyncRunRequest {
                    account_id: run.account_id.clone(),
                    run_id: run.id.clone(),
                })
                .await
                .unwrap()
                .into_inner();
        }
    })
    .await
    .unwrap();
    assert_eq!(run.state, "succeeded", "{:?}", run.error_code);
}
async fn refresh(h: &SystemHarness, account: &str) {
    let mail = h
        .authenticated()
        .start_sync(StartSyncRequest {
            account_id: account.into(),
            full: false,
            fetch_message_id: None,
        })
        .await
        .unwrap()
        .into_inner();
    complete(h, mail).await;
    let calendar = h
        .calendar()
        .refresh_agenda(RefreshAgendaRequest {
            account_id: account.into(),
            window: Some(window()),
        })
        .await
        .unwrap()
        .into_inner();
    complete(h, calendar).await;
}
async fn mail_projection(h: &SystemHarness, account: &str) -> BTreeMap<String, Value> {
    let mut rows = BTreeMap::new();
    let mut page = None;
    loop {
        let result = h
            .mail()
            .list_messages(ListMailRequest {
                account_id: account.into(),
                collection_id: None,
                query: None,
                page_size: 2,
                page_token: page,
            })
            .await
            .unwrap()
            .into_inner();
        assert_eq!(result.coverage.unwrap().state, "current");
        for row in result.items {
            let labels: BTreeSet<_> = row.collections.into_iter().map(|v| v.provider_id).collect();
            let value = json!({"subject": row.subject, "thread": row.thread_id, "labels": labels});
            assert!(
                rows.insert(row.provider_id, value).is_none(),
                "duplicate mail across pages"
            );
        }
        page = result.next_page_token;
        if page.is_none() {
            return rows;
        }
    }
}
async fn calendar_projection(h: &SystemHarness, account: &str) -> BTreeMap<String, CalendarEvent> {
    let calendars = h
        .calendar()
        .list_calendars(ListCalendarsRequest {
            account_id: account.into(),
            page_size: 100,
            page_token: None,
        })
        .await
        .unwrap()
        .into_inner();
    assert!(calendars.next_page_token.is_none());
    let ids: BTreeMap<_, _> = calendars
        .items
        .into_iter()
        .map(|v| (v.id, v.provider_id))
        .collect();
    let mut rows = BTreeMap::new();
    let mut page = None;
    loop {
        let result = h
            .calendar()
            .list_agenda(ListAgendaRequest {
                account_id: account.into(),
                window: Some(window()),
                page_size: 2,
                page_token: page,
            })
            .await
            .unwrap()
            .into_inner();
        assert!(result.coverage.iter().all(|v| v.state == "current"));
        for mut row in result.items {
            let key = format!("{}:{}", ids[&row.calendar_id], row.provider_id);
            row.calendar_id = ids[&row.calendar_id].clone();
            // These UUIDs identify local rows; provider identities and all
            // observable event fields must agree across independent profiles.
            row.id.clear();
            row.account_id.clear();
            row.recurring_event_id = row.recurring_event_id.map(|_| String::new());
            assert!(
                rows.insert(key, row).is_none(),
                "duplicate event across pages"
            );
        }
        page = result.next_page_token;
        if page.is_none() {
            return rows;
        }
    }
}
fn event(id: &str, summary: &str) -> Value {
    json!({"id":id,"summary":summary,"status":"confirmed",
        "start":{"dateTime":"2026-03-10T09:00:00-05:00","timeZone":"America/Chicago"},
        "end":{"dateTime":"2026-03-10T10:00:00-05:00","timeZone":"America/Chicago"}})
}
async fn add_mail(google: &MockGoogle, id: &str) -> Result<(), TestError> {
    google.control().add_message(ADDRESS, id, id,
        format!("From: outside@example.test\r\nTo: {ADDRESS}\r\nSubject: External {id}\r\nMessage-ID: <{id}@example.test>\r\n\r\nExternal body\r\n").into_bytes(),
        ["INBOX".into(), "UNREAD".into()].into_iter().collect()).await
}

#[tokio::test]
async fn three_independent_profiles_converge_after_external_changes_and_staggered_restarts(
) -> Result<(), TestError> {
    let google = Arc::new(MockGoogle::start(Seed::TwoAccounts).await?);
    google.control().set_page_cap(2).await?;
    google
        .control()
        .put_event(ADDRESS, "primary", event("multi-keep", "Initial meeting"))
        .await?;
    google
        .control()
        .put_event(ADDRESS, "primary", event("multi-delete", "Removed meeting"))
        .await?;
    let mut engines = Vec::new();
    let mut accounts = Vec::new();
    let mut profile_ids = BTreeSet::new();
    for _ in 0..3 {
        let h = SystemHarness::with_google(google.clone(), 64 * 1024 * 1024, None).await?;
        let account = finish(&h, &begin(&h, ADDRESS, None).await, 200)
            .await
            .account_id
            .unwrap();
        profile_ids.insert(
            h.authenticated()
                .get_status(GetStatusRequest {})
                .await?
                .into_inner()
                .profile_id,
        );
        refresh(&h, &account).await;
        accounts.push(account);
        engines.push(h);
    }
    assert_eq!(profile_ids.len(), 3);
    assert_eq!(accounts.iter().collect::<BTreeSet<_>>().len(), 3);
    let saved = engines[0]
        .mail()
        .save_draft(SaveDraftRequest {
            account_id: accounts[0].clone(),
            draft_id: None,
            expected_version: None,
            content: Some(serde_json::from_value(
                json!({"to":[{"address":"recipient@example.test"}],
            "subject":"Local to profile zero","text":"Must never synchronize or send"}),
            )?),
        })
        .await?
        .into_inner();

    engines[1].shutdown().await?;
    add_mail(&google, "multi-first").await?;
    google
        .control()
        .change_labels(ADDRESS, "m-001", &["STARRED".into()], &["UNREAD".into()])
        .await?;
    google.control().delete_message(ADDRESS, "m-002").await?;
    refresh(&engines[0], &accounts[0]).await;
    engines[0].shutdown().await?;
    engines[1].restart().await?;
    add_mail(&google, "multi-second").await?;
    google
        .control()
        .put_event(
            ADDRESS,
            "primary",
            event("multi-keep", "Final external meeting"),
        )
        .await?;
    google
        .control()
        .delete_event(ADDRESS, "primary", "multi-delete")
        .await?;
    refresh(&engines[1], &accounts[1]).await;
    engines[2].shutdown().await?;
    engines[2].restart().await?;
    refresh(&engines[2], &accounts[2]).await;
    engines[0].restart().await?;
    refresh(&engines[0], &accounts[0]).await;

    let remote = google.control().snapshot().await;
    let expected_mail: BTreeMap<_, _> = remote.mail[ADDRESS]
        .messages
        .iter()
        .map(|(id, row)| {
            (
                id.clone(),
                json!({"subject":row.subject,"thread":row.thread_id,"labels":row.labels}),
            )
        })
        .collect();
    let baseline_calendar = calendar_projection(&engines[0], &accounts[0]).await;
    let event_key = format!("{ADDRESS}:multi-keep");
    assert_eq!(
        baseline_calendar[&event_key].summary.as_deref(),
        Some("Final external meeting")
    );
    let provider_json: Value = serde_json::from_str(&baseline_calendar[&event_key].provider_json)?;
    assert_eq!(
        provider_json,
        remote.calendars[ADDRESS][ADDRESS].events["multi-keep"]
    );
    assert!(!baseline_calendar.contains_key(&format!("{ADDRESS}:multi-delete")));
    for (i, h) in engines.iter().enumerate() {
        assert_eq!(mail_projection(h, &accounts[i]).await, expected_mail);
        assert_eq!(
            calendar_projection(h, &accounts[i]).await,
            baseline_calendar
        );
        let drafts = h
            .mail()
            .list_drafts(ListDraftsRequest {
                account_id: accounts[i].clone(),
                page_size: 100,
                page_token: None,
            })
            .await?
            .into_inner();
        assert!(drafts.next_page_token.is_none());
        assert_eq!(drafts.items.len(), usize::from(i == 0));
        if i == 0 {
            assert_eq!(drafts.items[0].id, saved.id);
        }
    }
    for mailbox in remote.mail.values() {
        assert_eq!(mailbox.accepted_sends.len(), 0, "sync submitted mail");
        assert_eq!(mailbox.message_copies, 0, "sync created a remote copy");
    }
    for calendars in remote.calendars.values() {
        for calendar in calendars.values() {
            assert_eq!(calendar.notifications.len(), 0, "sync sent invitations");
        }
    }
    for h in &mut engines {
        h.shutdown().await?;
    }
    let final_remote = google.control().snapshot().await;
    assert_eq!(
        serde_json::to_value(final_remote.mail)?,
        serde_json::to_value(remote.mail)?
    );
    assert_eq!(
        serde_json::to_value(final_remote.calendars)?,
        serde_json::to_value(remote.calendars)?
    );
    Ok(())
}
