#![allow(clippy::unwrap_used)]
use nuncio_engine::{
    domain::{
        identity::AccountId,
        imap::{ImapFlags, ImapMailboxState, ImapPlacement, MailboxName},
        mail::decode_mime,
    },
    store::{AccountRecord, MailQuery, StagedMail, Store},
};
use zeroize::Zeroizing;
#[path = "imap_projection/flags.rs"]
mod flags;
#[path = "imap_projection/transfers.rs"]
mod transfers;

fn mailbox(name: &str, validity: u32, next: u32) -> ImapMailboxState {
    ImapMailboxState {
        name: MailboxName::from_unicode(name).unwrap(),
        delimiter: Some('/'),
        attributes: vec![],
        uid_validity: Some(validity),
        uid_next: Some(next),
        highest_mod_seq: None,
    }
}
fn mail(placement: ImapPlacement) -> StagedMail {
    let raw=b"From: sender@example.test\r\nTo: alpha@example.test\r\nMessage-ID: <same-rfc-id@example.test>\r\nSubject: Identical copies\r\n\r\nIndependent placements\r\n".to_vec();
    StagedMail {
        provider_id: placement.provider_id().unwrap(),
        thread_id: None,
        history_id: None,
        internal_date_ms: Some(1700000000000),
        provider_json: "{}".into(),
        subject: None,
        headers: vec![],
        labels: vec![],
        decoded: Some(decode_mime(&raw, 64 * 1024 * 1024).unwrap()),
        raw: Some(raw),
        availability: "available".into(),
    }
}
async fn begin(store: &Store, account: AccountId) -> String {
    let run = store
        .start_mail_run(account.to_string(), "full".into(), 10)
        .await
        .unwrap();
    assert_eq!(run.scope, "imap");
    store
        .begin_sync_run(account.to_string(), run.id.clone(), None)
        .await
        .unwrap();
    run.id
}
fn query(account: AccountId) -> MailQuery {
    MailQuery {
        account_id: account.to_string(),
        collection_id: None,
        query: None,
        page_size: 100,
        page_token: None,
    }
}
#[tokio::test]
async fn staged_imap_placements_publish_atomically_and_reset_uid_epochs_without_guessing_identity()
{
    let temp = tempfile::tempdir().unwrap();
    let key = Zeroizing::new(vec![0x46; 32]);
    let store = Store::open(temp.path(), key.clone()).await.unwrap();
    let account = AccountId::generate();
    store
        .add_account(AccountRecord {
            id: account.to_string(),
            provider: "imap".into(),
            address: "alpha@example.test".into(),
        })
        .await
        .unwrap();
    let run = begin(&store, account).await;
    let inbox = store
        .stage_imap_mailbox(account.to_string(), run.clone(), mailbox("INBOX", 9001, 2))
        .await
        .unwrap();
    let archive = store
        .stage_imap_mailbox(
            account.to_string(),
            run.clone(),
            mailbox("Archive", 9010, 2),
        )
        .await
        .unwrap();
    for (folder, epoch) in [(inbox, 9001), (archive, 9010)] {
        let placement = ImapPlacement::new(account, folder, epoch, 1).unwrap();
        store
            .stage_imap_mail(
                run.clone(),
                placement,
                ImapFlags::new(vec!["\\Seen".into()]).unwrap(),
                mail(placement),
            )
            .await
            .unwrap();
    }
    assert!(store
        .query_mail(query(account))
        .await
        .unwrap()
        .items
        .is_empty());
    assert!(store
        .imap_mailboxes(account.to_string())
        .await
        .unwrap()
        .is_empty());
    store
        .promote_mail(account.to_string(), run, None, 20)
        .await
        .unwrap();
    let first = store.query_mail(query(account)).await.unwrap();
    assert_eq!(first.items.len(), 2);
    assert_eq!(first.coverage.state, "current");
    assert_ne!(first.items[0].id, first.items[1].id);
    assert_ne!(first.items[0].provider_id, first.items[1].provider_id);
    let original_inbox = first
        .items
        .iter()
        .find(|m| {
            ImapPlacement::from_provider_id(account, &m.provider_id)
                .unwrap()
                .mailbox_id
                == inbox
        })
        .unwrap()
        .id
        .clone();
    let original_archive = first
        .items
        .iter()
        .find(|m| {
            ImapPlacement::from_provider_id(account, &m.provider_id)
                .unwrap()
                .mailbox_id
                == archive
        })
        .unwrap()
        .id
        .clone();
    let run = begin(&store, account).await;
    assert_eq!(
        store
            .stage_imap_mailbox(account.to_string(), run.clone(), mailbox("inbox", 9002, 2))
            .await
            .unwrap(),
        inbox
    );
    let placement = ImapPlacement::new(account, inbox, 9002, 1).unwrap();
    store
        .stage_imap_mail(
            run.clone(),
            placement,
            ImapFlags::new(vec![]).unwrap(),
            mail(placement),
        )
        .await
        .unwrap();
    let before = store.query_mail(query(account)).await.unwrap();
    assert_eq!(before.items.len(), 2);
    assert_eq!(before.coverage.cursor, first.coverage.cursor);
    store
        .promote_mail(account.to_string(), run, None, 30)
        .await
        .unwrap();
    let replaced = store.query_mail(query(account)).await.unwrap();
    assert_eq!(replaced.items.len(), 1);
    assert_ne!(replaced.items[0].id, original_inbox);
    assert_ne!(replaced.coverage.cursor, first.coverage.cursor);
    assert!(store
        .get_mail(account.to_string(), original_archive)
        .await
        .is_err());
    assert!(store
        .imap_mailboxes(account.to_string())
        .await
        .unwrap()
        .iter()
        .any(|m| m.id == archive && m.retired));
    let run = begin(&store, account).await;
    let recreated = store
        .stage_imap_mailbox(
            account.to_string(),
            run.clone(),
            mailbox("Archive", 9020, 1),
        )
        .await
        .unwrap();
    assert_ne!(recreated, archive);
    store
        .finish_sync_run_error(
            account.to_string(),
            run,
            "failed".into(),
            "fixture_failure".into(),
            40,
        )
        .await
        .unwrap();
    assert_eq!(
        store.query_mail(query(account)).await.unwrap().items[0].id,
        replaced.items[0].id
    );
    store.close().await.unwrap();
    let reopened = Store::open(temp.path(), key).await.unwrap();
    assert_eq!(
        reopened.query_mail(query(account)).await.unwrap().items[0].id,
        replaced.items[0].id
    );
    reopened.close().await.unwrap();
}

async fn setup() -> (tempfile::TempDir, Store, AccountId) {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(temp.path(), Zeroizing::new(vec![0x61; 32]))
        .await
        .unwrap();
    let account = AccountId::generate();
    store
        .add_account(AccountRecord {
            id: account.to_string(),
            provider: "imap".into(),
            address: "alpha@example.test".into(),
        })
        .await
        .unwrap();
    (temp, store, account)
}
async fn delta(store: &Store, account: AccountId) -> String {
    let run = store
        .start_mail_run(account.to_string(), "delta".into(), 40)
        .await
        .unwrap();
    store
        .begin_sync_run(account.to_string(), run.id.clone(), None)
        .await
        .unwrap();
    run.id
}
#[tokio::test]
async fn quiet_delta_retains_cursor_and_identity_but_noselect_removes_obsolete_placements() {
    let (_temp, store, account) = setup().await;
    let run = begin(&store, account).await;
    let inbox = store
        .stage_imap_mailbox(account.to_string(), run.clone(), mailbox("INBOX", 1, 2))
        .await
        .unwrap();
    let placement = ImapPlacement::new(account, inbox, 1, 1).unwrap();
    store
        .stage_imap_mail(
            run.clone(),
            placement,
            ImapFlags::new(vec![]).unwrap(),
            mail(placement),
        )
        .await
        .unwrap();
    store
        .promote_mail(account.to_string(), run, None, 20)
        .await
        .unwrap();
    let first = store.query_mail(query(account)).await.unwrap();
    let run = delta(&store, account).await;
    store
        .stage_imap_mailbox(account.to_string(), run.clone(), mailbox("INBOX", 1, 2))
        .await
        .unwrap();
    store
        .promote_mail(account.to_string(), run, None, 50)
        .await
        .unwrap();
    let quiet = store.query_mail(query(account)).await.unwrap();
    assert_eq!(quiet.coverage.cursor, first.coverage.cursor);
    assert_eq!(quiet.items[0].id, first.items[0].id);
    let run = delta(&store, account).await;
    let mut folder = mailbox("INBOX", 1, 2);
    folder.attributes = vec!["\\Noselect".into()];
    folder.uid_validity = None;
    folder.uid_next = None;
    store
        .stage_imap_mailbox(account.to_string(), run.clone(), folder)
        .await
        .unwrap();
    store
        .promote_mail(account.to_string(), run, None, 60)
        .await
        .unwrap();
    let after = store.query_mail(query(account)).await.unwrap();
    assert!(
        after.items.is_empty(),
        "a now nonselectable mailbox cannot retain an authoritative message placement"
    );
    assert_ne!(after.coverage.cursor, quiet.coverage.cursor);
    let mut search = query(account);
    search.query = Some("Independent placements".into());
    assert!(store.query_mail(search).await.unwrap().items.is_empty());
    store.close().await.unwrap();
}
#[tokio::test]
async fn imap_promotion_revalidates_staged_membership_and_catalog_before_any_publication() {
    let (_temp, store, account) = setup().await;
    let run = begin(&store, account).await;
    let inbox = store
        .stage_imap_mailbox(account.to_string(), run.clone(), mailbox("INBOX", 1, 2))
        .await
        .unwrap();
    let placement = ImapPlacement::new(account, inbox, 1, 1).unwrap();
    let bad_epoch = ImapPlacement::new(account, inbox, 2, 1).unwrap();
    assert!(store
        .stage_imap_mail(
            run.clone(),
            bad_epoch,
            ImapFlags::new(vec![]).unwrap(),
            mail(bad_epoch)
        )
        .await
        .is_err());
    let beyond_next = ImapPlacement::new(account, inbox, 1, 2).unwrap();
    assert!(store
        .stage_imap_mail(
            run.clone(),
            beyond_next,
            ImapFlags::new(vec![]).unwrap(),
            mail(beyond_next)
        )
        .await
        .is_err());
    let mut wrong_membership = mail(placement);
    wrong_membership.provider_json =
        serde_json::to_string(&nuncio_engine::domain::imap::ImapMessageState {
            placement,
            flags: ImapFlags::new(vec![]).unwrap(),
        })
        .unwrap();
    wrong_membership.labels = vec!["unrecognized".into()];
    store
        .stage_mail(account.to_string(), run.clone(), wrong_membership)
        .await
        .unwrap();
    assert!(store
        .promote_mail(account.to_string(), run.clone(), None, 20)
        .await
        .is_err());
    assert!(store
        .imap_mailboxes(account.to_string())
        .await
        .unwrap()
        .is_empty());
    assert!(store
        .query_mail(query(account))
        .await
        .unwrap()
        .items
        .is_empty());
    store
        .stage_imap_mail(
            run.clone(),
            placement,
            ImapFlags::new(vec![]).unwrap(),
            mail(placement),
        )
        .await
        .unwrap();
    store
        .stage_mail_collection(
            account.to_string(),
            run.clone(),
            "foreign-collection".into(),
            Some("Ghost".into()),
            "user".into(),
        )
        .await
        .unwrap();
    assert!(
        store
            .promote_mail(account.to_string(), run, None, 30)
            .await
            .is_err(),
        "IMAP collections require a discovered mailbox identity"
    );
    assert!(store
        .imap_mailboxes(account.to_string())
        .await
        .unwrap()
        .is_empty());
    assert!(store
        .mail_coverage(account.to_string())
        .await
        .unwrap()
        .cursor
        .is_none());
    store.recover_sync_runs(40).await.unwrap();
    let run = begin(&store, account).await;
    store
        .stage_imap_mailbox(account.to_string(), run.clone(), mailbox("INBOX", 3, 1))
        .await
        .unwrap();
    store
        .promote_mail(account.to_string(), run, None, 50)
        .await
        .unwrap();
    assert_eq!(
        store
            .imap_mailboxes(account.to_string())
            .await
            .unwrap()
            .len(),
        1
    );
    store.close().await.unwrap();
}

#[tokio::test]
async fn fetch_promotion_cannot_introduce_unobserved_placement_or_replace_mailbox_epoch() {
    let (_temp, store, account) = setup().await;
    let run = begin(&store, account).await;
    let inbox = store
        .stage_imap_mailbox(account.to_string(), run.clone(), mailbox("INBOX", 1, 2))
        .await
        .unwrap();
    let existing = ImapPlacement::new(account, inbox, 1, 1).unwrap();
    store
        .stage_imap_mail(
            run.clone(),
            existing,
            ImapFlags::new(vec![]).unwrap(),
            mail(existing),
        )
        .await
        .unwrap();
    store
        .promote_mail(account.to_string(), run, None, 20)
        .await
        .unwrap();
    let before = store.query_mail(query(account)).await.unwrap();
    for (epoch, uid, next) in [(1, 2, 3), (2, 1, 2)] {
        let run = store
            .start_mail_run(account.to_string(), "fetch".into(), 30)
            .await
            .unwrap()
            .id;
        store
            .begin_sync_run(account.to_string(), run.clone(), None)
            .await
            .unwrap();
        store
            .stage_imap_mailbox(
                account.to_string(),
                run.clone(),
                mailbox("INBOX", epoch, next),
            )
            .await
            .unwrap();
        let placement = ImapPlacement::new(account, inbox, epoch, uid).unwrap();
        store
            .stage_imap_mail(
                run.clone(),
                placement,
                ImapFlags::new(vec![]).unwrap(),
                mail(placement),
            )
            .await
            .unwrap();
        assert!(store
            .promote_mail(account.to_string(), run.clone(), None, 40)
            .await
            .is_err());
        let after = store.query_mail(query(account)).await.unwrap();
        assert_eq!(after.items.len(), 1);
        assert_eq!(after.items[0].id, before.items[0].id);
        assert_eq!(after.coverage.cursor, before.coverage.cursor);
        store
            .finish_sync_run_error(
                account.to_string(),
                run,
                "failed".into(),
                "invalid_input".into(),
                41,
            )
            .await
            .unwrap();
    }
    store.close().await.unwrap();
}

#[tokio::test]
async fn imap_coverage_ignores_optional_interest_hints_but_retains_state_and_real_changes() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(temp.path(), Zeroizing::new(vec![0x68; 32]))
        .await
        .unwrap();
    let account = AccountId::generate();
    store
        .add_account(AccountRecord {
            id: account.to_string(),
            provider: "imap".into(),
            address: "alpha@example.test".into(),
        })
        .await
        .unwrap();
    let mut expected_cursor = None;
    let mut expected_id = None;
    for hint in [
        None,
        Some("\\Unmarked"),
        None,
        Some("\\Marked"),
        Some("\\unmarked"),
        None,
    ] {
        let mut state = mailbox("INBOX", 9001, 1);
        state.attributes.push("\\HasNoChildren".into());
        if let Some(hint) = hint {
            state.attributes.push(hint.into());
        }
        let run = begin(&store, account).await;
        let id = store
            .stage_imap_mailbox(account.to_string(), run.clone(), state.clone())
            .await
            .unwrap();
        store
            .promote_mail(account.to_string(), run, None, 20)
            .await
            .unwrap();
        let saved = store.imap_mailboxes(account.to_string()).await.unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(
            saved[0].state.attributes,
            state.validated().unwrap().attributes,
            "the original provider hints must remain stored"
        );
        let cursor = store
            .query_mail(query(account))
            .await
            .unwrap()
            .coverage
            .cursor;
        assert!(cursor.is_some());
        if expected_cursor.is_some() {
            assert_eq!(
                cursor, expected_cursor,
                "optional LIST interest hints must not change synchronized coverage"
            );
            assert_eq!(Some(id), expected_id);
        }
        expected_cursor = cursor;
        expected_id = Some(id);
    }
    let mut state = mailbox("INBOX", 9001, 1);
    state.attributes.push("\\HasNoChildren".into());
    for change in 0..5 {
        match change {
            0 => state.attributes.push("\\Sent".into()),
            1 => state.delimiter = Some('.'),
            2 => state.uid_next = Some(2),
            3 => state.highest_mod_seq = Some(17),
            _ => state.uid_validity = Some(9002),
        }
        let run = begin(&store, account).await;
        let id = store
            .stage_imap_mailbox(account.to_string(), run.clone(), state.clone())
            .await
            .unwrap();
        store
            .promote_mail(account.to_string(), run, None, 20)
            .await
            .unwrap();
        let cursor = store
            .query_mail(query(account))
            .await
            .unwrap()
            .coverage
            .cursor;
        assert_ne!(
            cursor, expected_cursor,
            "real mailbox state change {change} must change coverage"
        );
        assert_eq!(Some(id), expected_id);
        expected_cursor = cursor;
    }
    store.close().await.unwrap();
}
