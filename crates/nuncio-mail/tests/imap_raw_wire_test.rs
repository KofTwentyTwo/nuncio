//! Wire-level tests for the raw IMAP command layer.
//!
//! These drive the real `async-imap` client over real TCP against
//! [`MockImapServer`], because the whole claim being tested is that responses
//! the typed API discards can be recovered from the wire. A unit test over a
//! hand-built struct could not show that: it would assert on data this crate
//! produced rather than data a server sent.
//!
//! Each test pins one response that decides whether a sync saw everything, or
//! whether a mutation actually happened.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use nuncio_mail::imap_raw;
use nuncio_mail::test_server::{MockImapServer, ServerProfile};

/// Connect the real client to the mock server and authenticate.
async fn login(
    server: &MockImapServer,
) -> async_imap::Session<tokio::io::BufWriter<tokio::net::TcpStream>> {
    let stream = tokio::net::TcpStream::connect((server.host(), server.port()))
        .await
        .expect("connect to the mock server");
    let client = async_imap::Client::new(tokio::io::BufWriter::new(stream));
    client
        .login("user", "pass")
        .await
        .map_err(|(e, _)| e)
        .expect("login succeeds")
}

#[tokio::test]
async fn qresync_select_recovers_vanished_uids_the_typed_select_would_drop() {
    // The mechanism that fixes ghost messages. `async-imap`'s own `select()`
    // parser has no arm for VANISHED, so this is the difference between
    // learning a message was deleted and never learning it.
    let server = MockImapServer::start(ServerProfile::Qresync)
        .await
        .expect("server starts");
    server.create_mailbox("INBOX");
    let keep = server.append_message("INBOX", "Subject: keep\r\n\r\nx", &[]);
    let gone = server.append_message("INBOX", "Subject: gone\r\n\r\nx", &[]);

    let mut session = login(&server).await;
    assert!(
        imap_raw::enable(&mut session, &["QRESYNC"])
            .await
            .expect("ENABLE succeeds"),
        "the server advertises QRESYNC, so ENABLE must be accepted"
    );

    let initial = imap_raw::run_collected(&mut session, "SELECT INBOX")
        .await
        .expect("plain SELECT");
    let modseq = initial
        .highest_modseq
        .expect("a CONDSTORE server reports HIGHESTMODSEQ");
    let uid_validity = initial.uid_validity.expect("SELECT reports UIDVALIDITY");

    // Another client removes a message while we are away.
    assert!(server.expunge("INBOX", gone));

    let resync = imap_raw::select_qresync(&mut session, "INBOX", uid_validity, modseq, None)
        .await
        .expect("QRESYNC SELECT succeeds");

    assert!(
        resync.vanished_uids.contains(&gone),
        "the removal must be reported, got {:?}",
        resync.vanished_uids
    );
    assert!(
        !resync.vanished_uids.contains(&keep),
        "a surviving message must not be reported as vanished"
    );
}

#[tokio::test]
async fn a_conditional_store_that_loses_a_race_reports_modified() {
    // RFC 7162 3.1.3. MODIFIED rides on the tagged OK, which
    // `run_command_and_check_ok` discards -- so without this layer a lost
    // update is indistinguishable from a successful one.
    let server = MockImapServer::start(ServerProfile::Condstore)
        .await
        .expect("server starts");
    server.create_mailbox("INBOX");
    let uid = server.append_message("INBOX", "Subject: contested\r\n\r\nx", &[]);

    let mut writer = login(&server).await;
    let mut loser = login(&server).await;
    imap_raw::run_collected(&mut writer, "SELECT INBOX")
        .await
        .expect("writer selects");
    imap_raw::run_collected(&mut loser, "SELECT INBOX")
        .await
        .expect("loser selects");

    // The loser captures the current mod-sequence, then the writer changes the
    // message underneath it.
    let observed = imap_raw::run_collected(&mut loser, &format!("UID FETCH {uid} (FLAGS)"))
        .await
        .expect("fetch flags");
    assert_eq!(observed.fetched_uids, vec![uid]);
    let modseq = imap_raw::run_collected(&mut loser, "SELECT INBOX")
        .await
        .expect("re-select")
        .highest_modseq
        .expect("HIGHESTMODSEQ reported");

    imap_raw::run_collected(&mut writer, &format!("UID STORE {uid} +FLAGS (\\Seen)"))
        .await
        .expect("the writer wins the race");

    let outcome = imap_raw::uid_store_unchanged_since(
        &mut loser,
        &uid.to_string(),
        modseq,
        "+FLAGS",
        "\\Flagged",
    )
    .await
    .expect("a refused conditional store is not a transport failure");

    assert_eq!(
        outcome.modified_uids(),
        Some(vec![uid]),
        "the conflict must name the UID; information was {:?}",
        outcome.information
    );

    // And the refused flag genuinely did not apply.
    let flags = server.flags("INBOX", uid).unwrap_or_default();
    assert!(
        !flags.iter().any(|f| f == "\\Flagged"),
        "a MODIFIED store must leave the message untouched, got {flags:?}"
    );
}

#[tokio::test]
async fn a_conditional_store_that_wins_reports_no_conflict() {
    let server = MockImapServer::start(ServerProfile::Condstore)
        .await
        .expect("server starts");
    server.create_mailbox("INBOX");
    let uid = server.append_message("INBOX", "Subject: uncontested\r\n\r\nx", &[]);

    let mut session = login(&server).await;
    let modseq = imap_raw::run_collected(&mut session, "SELECT INBOX")
        .await
        .expect("select")
        .highest_modseq
        .expect("HIGHESTMODSEQ reported");

    let outcome = imap_raw::uid_store_unchanged_since(
        &mut session,
        &uid.to_string(),
        modseq,
        "+FLAGS",
        "\\Flagged",
    )
    .await
    .expect("store succeeds");

    assert_eq!(
        outcome.modified_uids(),
        None,
        "an uncontested store must report no conflict"
    );
    assert!(server
        .flags("INBOX", uid)
        .unwrap_or_default()
        .iter()
        .any(|f| f == "\\Flagged"));
}

#[tokio::test]
async fn a_move_reports_copyuid_from_an_untagged_ok() {
    // RFC 6851 4.3 advises COPYUID in an *untagged* OK for MOVE. A collector
    // that only read the tagged response would miss it on every conforming
    // server, and then have no proof the move happened.
    let server = MockImapServer::start(ServerProfile::Qresync)
        .await
        .expect("server starts");
    server.create_mailbox("INBOX");
    server.create_mailbox("Archive");
    let uid = server.append_message("INBOX", "Subject: moving\r\n\r\nx", &[]);

    let mut session = login(&server).await;
    imap_raw::run_collected(&mut session, "SELECT INBOX")
        .await
        .expect("select");

    let outcome = imap_raw::uid_move_collected(&mut session, &uid.to_string(), "Archive")
        .await
        .expect("move succeeds");

    let copy_uid = outcome
        .copy_uid
        .expect("a real move must be provable by COPYUID");
    assert_eq!(copy_uid.source_uids, vec![uid]);
    assert_eq!(copy_uid.destination_uids.len(), 1);
    assert_eq!(server.uids("INBOX"), Vec::<u32>::new());
    assert_eq!(server.uids("Archive"), copy_uid.destination_uids);
}

#[tokio::test]
async fn a_move_of_an_already_moved_uid_is_unknown_not_applied() {
    // RFC 3501 6.4.8: a UID command matching nothing succeeds having done
    // nothing. The command reports OK, so only the *absence* of COPYUID
    // distinguishes it -- and absence means unknown, never applied.
    let server = MockImapServer::start(ServerProfile::Qresync)
        .await
        .expect("server starts");
    server.create_mailbox("INBOX");
    server.create_mailbox("Archive");
    server.create_mailbox("Trash");
    let uid = server.append_message("INBOX", "Subject: contested\r\n\r\nx", &[]);

    let mut winner = login(&server).await;
    let mut loser = login(&server).await;
    imap_raw::run_collected(&mut winner, "SELECT INBOX")
        .await
        .expect("select");
    imap_raw::run_collected(&mut loser, "SELECT INBOX")
        .await
        .expect("select");

    imap_raw::uid_move_collected(&mut winner, &uid.to_string(), "Archive")
        .await
        .expect("the winner moves it");

    let outcome = imap_raw::uid_move_collected(&mut loser, &uid.to_string(), "Trash")
        .await
        .expect("the losing move still reports OK");

    assert!(
        outcome.ok,
        "a UID set matching nothing is a successful no-op, not an error"
    );
    assert_eq!(
        outcome.copy_uid, None,
        "nothing moved, so there is no COPYUID to prove one did"
    );
    assert!(
        server.uids("Trash").is_empty(),
        "and genuinely nothing arrived at the destination"
    );
}

#[tokio::test]
async fn expunge_is_provable_under_both_qresync_and_plain_servers() {
    // RFC 6851 4.4: a QRESYNC server answers UID EXPUNGE with VANISHED rather
    // than EXPUNGE. Counting only EXPUNGE reports failure on every successful
    // delete against such a server, so both must be collected.
    for profile in [ServerProfile::Qresync, ServerProfile::Condstore] {
        let server = MockImapServer::start(profile).await.expect("server starts");
        server.create_mailbox("INBOX");
        let uid = server.append_message("INBOX", "Subject: gone\r\n\r\nx", &["\\Deleted"]);

        let mut session = login(&server).await;
        if profile == ServerProfile::Qresync {
            imap_raw::enable(&mut session, &["QRESYNC"])
                .await
                .expect("ENABLE succeeds");
        }
        imap_raw::run_collected(&mut session, "SELECT INBOX")
            .await
            .expect("select");

        let outcome = imap_raw::uid_expunge_collected(&mut session, &uid.to_string())
            .await
            .expect("expunge succeeds");

        let reported = outcome.vanished_uids.len() + outcome.expunged_seqs.len();
        assert_eq!(
            reported, 1,
            "the removal must be provable on {profile:?}: vanished={:?} expunged={:?}",
            outcome.vanished_uids, outcome.expunged_seqs
        );
        assert!(server.uids("INBOX").is_empty());
    }
}

#[tokio::test]
async fn a_qresync_select_without_enable_is_refused() {
    // RFC 7162 3.2.3 requires a tagged BAD. Surfacing it as an error keeps a
    // client that skipped ENABLE from silently degrading in production.
    let server = MockImapServer::start(ServerProfile::Qresync)
        .await
        .expect("server starts");
    server.create_mailbox("INBOX");

    let mut session = login(&server).await;
    let err = imap_raw::select_qresync(&mut session, "INBOX", 1000, 1, None)
        .await
        .expect_err("QRESYNC without ENABLE must fail");
    assert!(
        err.to_string().contains("refused"),
        "the refusal must be surfaced, got: {err}"
    );
}

/// Drive a real folder sync against the mock and return what it observed.
async fn sync_folder(
    server: &MockImapServer,
    since: Option<&str>,
) -> nuncio_mail::backend::FolderChanges {
    use nuncio_mail::MailBackend;
    let engine = nuncio_mail::ImapEngine::with_credentials(
        "acct-1",
        server.host(),
        server.port(),
        nuncio_core::TlsMode::Plain,
        "user",
        "pass",
    );
    engine
        .sync_changes("INBOX", since)
        .await
        .expect("folder sync succeeds")
}

#[tokio::test]
async fn every_rung_reports_a_message_that_disappeared() {
    // The defect this whole change exists for. A forward-only sync can only
    // ever add, so a message removed by another client stays local forever.
    // Each rung must be able to say it is gone -- QRESYNC because the server
    // states it, the other two because the pass enumerated what remains.
    for profile in [
        ServerProfile::Qresync,
        ServerProfile::Condstore,
        ServerProfile::Basic,
    ] {
        let server = MockImapServer::start(profile).await.expect("server starts");
        server.create_mailbox("INBOX");
        let keep = server.append_message("INBOX", "Subject: keep\r\n\r\nkeep", &[]);
        let gone = server.append_message("INBOX", "Subject: gone\r\n\r\ngone", &[]);

        let first = sync_folder(&server, None).await;
        assert_eq!(first.upserts.len(), 2, "{profile:?}: first pass sees both");

        // Another client removes one while we are away.
        assert!(server.expunge("INBOX", gone));

        let second = sync_folder(&server, Some(&first.next_state)).await;

        // However the rung learned it, the caller must end up able to identify
        // the removed message and only that one.
        let mut reported: Vec<String> = second.removals.clone();
        if let Some(present) = &second.present {
            let stored_gone = first
                .upserts
                .iter()
                .map(|e| e.id.clone())
                .filter(|id| !present.contains(id));
            reported.extend(stored_gone);
        }
        reported.sort();
        reported.dedup();

        assert_eq!(
            reported.len(),
            1,
            "{profile:?}: exactly one message must be reported gone, got {reported:?}"
        );
        let surviving = first
            .upserts
            .iter()
            .find(|e| e.remote_id == keep.to_string())
            .expect("the kept message was in the first pass");
        assert!(
            !reported.contains(&surviving.id),
            "{profile:?}: the surviving message must not be reported gone"
        );
    }
}

#[tokio::test]
async fn an_incremental_pass_that_cannot_see_absence_says_so() {
    // `present: None` versus `Some(vec![])` is the difference between "I did
    // not look" and "the folder is empty". Conflating them deletes a mailbox,
    // so QRESYNC -- which never enumerates -- must report None.
    let server = MockImapServer::start(ServerProfile::Qresync)
        .await
        .expect("server starts");
    server.create_mailbox("INBOX");
    server.append_message("INBOX", "Subject: one\r\n\r\nx", &[]);

    let first = sync_folder(&server, None).await;
    let second = sync_folder(&server, Some(&first.next_state)).await;

    assert!(
        second.next_state.starts_with("v2:modseq:"),
        "a QRESYNC-capable mailbox must checkpoint by mod-sequence, got {:?}",
        second.next_state
    );
    assert_eq!(
        second.present, None,
        "a QRESYNC pass does not enumerate the folder, so it cannot claim to know what is present"
    );
}

#[tokio::test]
async fn a_condstore_only_server_still_checkpoints_by_modseq_and_enumerates() {
    // Gmail's rung. It cannot be told what vanished, so it must enumerate --
    // and it must still upgrade its checkpoint so the next pass is narrow.
    let server = MockImapServer::start(ServerProfile::Condstore)
        .await
        .expect("server starts");
    server.create_mailbox("INBOX");
    server.append_message("INBOX", "Subject: one\r\n\r\nx", &[]);

    let first = sync_folder(&server, None).await;
    assert!(
        first.next_state.starts_with("v2:modseq:"),
        "CONDSTORE alone is enough to checkpoint by mod-sequence, got {:?}",
        first.next_state
    );

    let second = sync_folder(&server, Some(&first.next_state)).await;
    assert!(
        second.present.is_some(),
        "without QRESYNC the only way to find removals is to enumerate"
    );
    assert!(
        second.upserts.is_empty(),
        "nothing changed, so CHANGEDSINCE must fetch no bodies"
    );
}

#[tokio::test]
async fn a_server_without_modseq_falls_back_to_a_uid_boundary_checkpoint() {
    // Exchange's rung: no CONDSTORE, no QRESYNC.
    let server = MockImapServer::start(ServerProfile::Basic)
        .await
        .expect("server starts");
    server.create_mailbox("INBOX");
    server.append_message("INBOX", "Subject: one\r\n\r\nx", &[]);

    let first = sync_folder(&server, None).await;
    assert!(
        first.next_state.starts_with("v1:uidnext:"),
        "a mailbox with no mod-sequences must checkpoint by UID boundary, got {:?}",
        first.next_state
    );
    assert!(first.present.is_some());
}

#[tokio::test]
async fn a_uidvalidity_change_refetches_rather_than_deleting_the_folder() {
    // After a renumbering every stored UID names a different message. The
    // dangerous outcome would be enumerating the new UID space, finding none
    // of the old ids in it, and concluding the whole folder was deleted.
    let server = MockImapServer::start(ServerProfile::Qresync)
        .await
        .expect("server starts");
    server.create_mailbox("INBOX");
    server.append_message("INBOX", "Subject: before\r\n\r\nx", &[]);

    let first = sync_folder(&server, None).await;
    assert_eq!(first.upserts.len(), 1);

    server.bump_uid_validity("INBOX");
    server.append_message("INBOX", "Subject: after\r\n\r\nx", &[]);

    let second = sync_folder(&server, Some(&first.next_state)).await;

    assert_eq!(
        second.upserts.len(),
        1,
        "a renumbered mailbox must be re-fetched in full"
    );
    assert!(
        second.removals.is_empty(),
        "a renumbering is not a removal report"
    );
    // The re-fetched message has a different surrogate, because UIDVALIDITY is
    // part of identity -- so the caller replaces rather than merges.
    assert_ne!(second.upserts[0].id, first.upserts[0].id);
}

/// Build an engine pointed at the mock and apply one mutation through the
/// real backend path.
async fn apply(
    server: &MockImapServer,
    kind: nuncio_mail::RemoteMutationKind,
    uid: u32,
    uid_validity: &str,
) -> nuncio_mail::MutationOutcome {
    use nuncio_mail::MailBackend;
    let engine = nuncio_mail::ImapEngine::with_credentials(
        "acct-1",
        server.host(),
        server.port(),
        nuncio_core::TlsMode::Plain,
        "user",
        "pass",
    );
    engine
        .apply_mutation(&nuncio_mail::RemoteMutationSpec {
            message_id: "m-1".to_string(),
            remote_id: uid.to_string(),
            folder_id: "INBOX".to_string(),
            uid_validity: uid_validity.to_string(),
            kind,
        })
        .await
        .expect("the mutation attempt itself succeeds")
}

/// UIDVALIDITY the mock assigns to the first mailbox created.
const FIRST_UID_VALIDITY: &str = "1000";

#[tokio::test]
async fn a_move_that_lost_a_race_is_unknown_rather_than_applied() {
    // The silent-loss case. The server answers OK for a UID that no longer
    // exists (RFC 3501 6.4.8), so only the absence of COPYUID distinguishes a
    // real move from a no-op -- and absence cannot mean "applied".
    let server = MockImapServer::start(ServerProfile::Qresync)
        .await
        .expect("server starts");
    server.create_mailbox("INBOX");
    server.create_mailbox("Archive");
    server.create_mailbox("Trash");
    let uid = server.append_message("INBOX", "Subject: contested\r\n\r\nx", &[]);

    let applied = apply(
        &server,
        nuncio_mail::RemoteMutationKind::Move {
            to_folder: "Archive".to_string(),
        },
        uid,
        FIRST_UID_VALIDITY,
    )
    .await;
    assert!(
        matches!(
            applied,
            nuncio_mail::MutationOutcome::Applied { token: Some(_) }
        ),
        "a real move must be Applied and name its destination UID, got {applied:?}"
    );

    let lost = apply(
        &server,
        nuncio_mail::RemoteMutationKind::Move {
            to_folder: "Trash".to_string(),
        },
        uid,
        FIRST_UID_VALIDITY,
    )
    .await;
    assert!(
        matches!(lost, nuncio_mail::MutationOutcome::Unknown { .. }),
        "a move of an already-moved UID must be Unknown, never Applied, got {lost:?}"
    );
    assert!(
        server.uids("Trash").is_empty(),
        "and nothing actually reached the destination"
    );
}

#[tokio::test]
async fn a_delete_is_applied_only_when_the_server_reports_the_removal() {
    let server = MockImapServer::start(ServerProfile::Qresync)
        .await
        .expect("server starts");
    server.create_mailbox("INBOX");
    let uid = server.append_message("INBOX", "Subject: gone\r\n\r\nx", &[]);

    let outcome = apply(
        &server,
        nuncio_mail::RemoteMutationKind::Delete,
        uid,
        FIRST_UID_VALIDITY,
    )
    .await;

    // Under QRESYNC the proof arrives as VANISHED, not EXPUNGE. Reading only
    // the typed expunge stream would report nothing removed here.
    assert!(
        matches!(outcome, nuncio_mail::MutationOutcome::Applied { .. }),
        "a delete the server confirmed must be Applied, got {outcome:?}"
    );
    assert!(server.uids("INBOX").is_empty());
}

#[tokio::test]
async fn a_delete_of_an_absent_uid_is_unknown() {
    let server = MockImapServer::start(ServerProfile::Qresync)
        .await
        .expect("server starts");
    server.create_mailbox("INBOX");
    server.append_message("INBOX", "Subject: present\r\n\r\nx", &[]);

    // UID 999 does not exist; the server ignores it and answers OK.
    let outcome = apply(
        &server,
        nuncio_mail::RemoteMutationKind::Delete,
        999,
        FIRST_UID_VALIDITY,
    )
    .await;

    assert!(
        matches!(outcome, nuncio_mail::MutationOutcome::Unknown { .. }),
        "a delete the server never confirmed must not be Applied, got {outcome:?}"
    );
    assert_eq!(
        server.uids("INBOX").len(),
        1,
        "the real message must be untouched"
    );
}
