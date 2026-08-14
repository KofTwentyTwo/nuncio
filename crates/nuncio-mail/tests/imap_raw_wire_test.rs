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
