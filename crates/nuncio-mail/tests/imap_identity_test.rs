//! Offline tests for server-assigned message identity over IMAP.
//!
//! The claim under test is the one ADR 0002 rests on: a message has one
//! account-scoped key, and which protocol observed it does not change that key.
//! RFC 8474 exists precisely so a server implementing both IMAP and JMAP
//! reports the same object id through both, so the last test here compares an
//! IMAP-derived key against a JMAP-derived one and requires them to be equal.
//!
//! Everything runs against [`MockImapServer`] over loopback and against a JMAP
//! response parsed from a literal; no test here reaches a network.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use nuncio_core::model::IdentitySource;
use nuncio_mail::backend::FolderChanges;
use nuncio_mail::test_server::{IdentityExtensions, MockImapServer, ObjectIds, ServerProfile};
use nuncio_mail::{JmapEngine, MailBackend};

/// Sync one folder through the real engine against the mock.
async fn sync(server: &MockImapServer, account_id: &str, folder: &str) -> FolderChanges {
    let engine = nuncio_mail::ImapEngine::with_credentials(
        account_id,
        server.host(),
        server.port(),
        nuncio_core::TlsMode::Plain,
        "user",
        "pass",
    );
    engine
        .sync_changes(folder, None)
        .await
        .expect("folder sync succeeds")
}

#[tokio::test]
async fn an_objectid_server_keys_messages_by_their_emailid() {
    // Tier 1. Reaching it at all is the difference between an identity the
    // server vouches for and one derived from whatever the message happened to
    // contain.
    let server =
        MockImapServer::start_with_identity(ServerProfile::Qresync, IdentityExtensions::objectid())
            .await
            .expect("server starts");
    server.create_mailbox("INBOX");
    let uid = server.append_message("INBOX", "Subject: one\r\n\r\nbody", &[]);

    let changes = sync(&server, "acct-objectid", "INBOX").await;

    assert_eq!(changes.upserts.len(), 1);
    assert_eq!(
        changes.upserts[0].source,
        IdentitySource::EmailId,
        "an OBJECTID server must be asked for EMAILID and the answer used"
    );
    assert_eq!(changes.upserts[0].placement.remote_id, uid.to_string());
    assert!(
        server
            .commands_matching("EMAILID")
            .iter()
            .any(|c| c.to_ascii_uppercase().contains("UID FETCH")),
        "the object id must come from a real UID FETCH, got {:?}",
        server.commands()
    );
}

#[tokio::test]
async fn the_same_message_in_a_second_folder_keys_identically() {
    // The property a folder-scoped surrogate can never have, and the reason
    // tier 1 is worth a separate round trip: two placements, one message.
    let server =
        MockImapServer::start_with_identity(ServerProfile::Qresync, IdentityExtensions::objectid())
            .await
            .expect("server starts");
    server.create_mailbox("INBOX");
    server.create_mailbox("Archive");

    let shared = ObjectIds {
        email_id: "M00000042".to_string(),
        gm_msgid: 1_278_455_344_230_334_865,
    };
    let body = "Subject: filed\r\n\r\nbody";
    server.append_message_with_object_ids("INBOX", body, &[], shared.clone());
    server.append_message_with_object_ids("Archive", body, &[], shared);

    let inbox = sync(&server, "acct-shared", "INBOX").await;
    let archive = sync(&server, "acct-shared", "Archive").await;

    assert_eq!(inbox.upserts.len(), 1);
    assert_eq!(archive.upserts.len(), 1);
    assert_eq!(
        inbox.upserts[0].email.id, archive.upserts[0].email.id,
        "one message in two folders must be one identity"
    );
    assert_eq!(inbox.upserts[0].source, IdentitySource::EmailId);
    assert_eq!(archive.upserts[0].source, IdentitySource::EmailId);
    assert_ne!(
        inbox.upserts[0].placement, archive.upserts[0].placement,
        "the two occupancies are still distinct; only the message is shared"
    );
}

#[tokio::test]
async fn a_gmail_shaped_server_keys_messages_by_x_gm_msgid() {
    // Gmail advertises X-GM-EXT-1 and not OBJECTID, so tier 2 is the best it
    // offers -- and it is reachable through the typed client, with no extra
    // round trip, because `imap-proto` can parse that attribute.
    let server =
        MockImapServer::start_with_identity(ServerProfile::Condstore, IdentityExtensions::gmail())
            .await
            .expect("server starts");
    server.create_mailbox("INBOX");
    server.append_message("INBOX", "Subject: gmail\r\n\r\nbody", &[]);

    let changes = sync(&server, "acct-gmail", "INBOX").await;

    assert_eq!(changes.upserts.len(), 1);
    assert_eq!(
        changes.upserts[0].source,
        IdentitySource::GmailMsgId,
        "X-GM-EXT-1 must lift identity to the Gmail message id"
    );
    assert!(
        server.commands_matching("EMAILID").is_empty(),
        "a server without OBJECTID must never be asked for EMAILID, got {:?}",
        server.commands()
    );
}

#[tokio::test]
async fn a_server_advertising_neither_extension_is_left_exactly_as_it_was() {
    // The regression that matters most: capability negotiation must be
    // invisible to a server that has neither extension. No new command, and
    // the same tier it reached before any of this existed.
    for profile in [
        ServerProfile::Qresync,
        ServerProfile::Condstore,
        ServerProfile::Basic,
    ] {
        let server = MockImapServer::start(profile).await.expect("server starts");
        server.create_mailbox("INBOX");
        server.append_message("INBOX", "Subject: plain\r\n\r\nbody", &[]);

        let changes = sync(&server, "acct-plain", "INBOX").await;

        assert_eq!(changes.upserts.len(), 1, "{profile:?}");
        assert_eq!(
            changes.upserts[0].source,
            IdentitySource::Surrogate,
            "{profile:?}: with no server id and no octets, the surrogate is the honest answer"
        );
        assert!(
            server.commands_matching("EMAILID").is_empty(),
            "{profile:?}: no EMAILID command may be issued, got {:?}",
            server.commands()
        );
        assert!(
            server.commands_matching("X-GM-MSGID").is_empty(),
            "{profile:?}: no X-GM-MSGID item may be requested, got {:?}",
            server.commands()
        );
    }
}

#[tokio::test]
async fn an_imap_emailid_and_a_jmap_object_id_produce_the_same_key() {
    // The convergence assertion. One account, one message, one server-assigned
    // id -- reported as RFC 8474 EMAILID over IMAP and as the Email object id
    // over JMAP. If either engine derived its key from a different tier, or
    // hashed under a different domain label, these two strings would differ.
    const ACCOUNT: &str = "acct-converge";
    const OBJECT_ID: &str = "M00000001";

    let server =
        MockImapServer::start_with_identity(ServerProfile::Qresync, IdentityExtensions::objectid())
            .await
            .expect("server starts");
    server.create_mailbox("INBOX");
    server.append_message_with_object_ids(
        "INBOX",
        "Subject: both\r\n\r\nbody",
        &[],
        ObjectIds {
            email_id: OBJECT_ID.to_string(),
            gm_msgid: 1,
        },
    );
    let imap = sync(&server, ACCOUNT, "INBOX").await;

    let jmap_response = format!(
        r#"{{"methodResponses":[["Email/get",{{"accountId":"{ACCOUNT}",
           "state":"s-1","list":[{{"id":"{OBJECT_ID}","subject":"both",
           "from":[{{"email":"a@nuncio.mx"}}],"to":[{{"email":"b@nuncio.mx"}}],
           "receivedAt":1700001000,"isUnread":true}}]}},"c1"]]}}"#
    );
    let (jmap, _state) = JmapEngine::new(ACCOUNT)
        .parse_email_get_response(&jmap_response, "mb-inbox")
        .expect("the JMAP response parses");

    assert_eq!(imap.upserts.len(), 1);
    assert_eq!(jmap.len(), 1);
    assert_eq!(
        imap.upserts[0].source,
        IdentitySource::EmailId,
        "the IMAP side must have entered at the object-id tier"
    );
    assert_eq!(
        jmap[0].source,
        IdentitySource::EmailId,
        "the JMAP side must have entered at the object-id tier"
    );
    assert_eq!(
        imap.upserts[0].email.id, jmap[0].email.id,
        "two engines syncing one account must agree on what the message is"
    );

    // Equality alone would survive both engines moving to a different tier or
    // a different domain label together, since they share one derivation. The
    // golden pins the actual value: SHA-256 over the length-prefixed
    // (tier, account, object id) triple. It is a tripwire, not a spec -- if it
    // moves, every stored message_key has moved with it, and
    // `IDENTITY_SCHEMA_VERSION` must move in the same commit.
    const EXPECTED_KEY: &str = "ba2eb228c6b973c548837e32b84e95cf06b73664b3e350f5dfa30614b2e38bd0";
    assert_eq!(
        imap.upserts[0].email.id, EXPECTED_KEY,
        "the object-id tier's derivation changed; bump IDENTITY_SCHEMA_VERSION with it"
    );

    // And the two engines still describe *where* it sits in their own terms:
    // convergence is a claim about identity, never about placement.
    assert_ne!(imap.upserts[0].placement, jmap[0].placement);
}
