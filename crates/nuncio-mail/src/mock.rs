//! Deterministic mock mail backend for offline testing and integration verification.

use async_trait::async_trait;
use nuncio_core::model::{Email, Folder, IdentitySource, Placement};
use std::sync::{Arc, Mutex};

use crate::backend::{
    FolderChanges, MailBackend, MessageSender, MutationOutcome, OutboundMessage, PlacedMessage,
    RemoteMutationSpec,
};
use crate::parser::MailError;

/// Thread-safe mock mail backend for offline testing.
#[derive(Debug, Clone, Default)]
pub struct MockMailBackend {
    folders: Arc<Mutex<Vec<Folder>>>,
    messages: Arc<Mutex<Vec<PlacedMessage>>>,
    should_fail: Arc<Mutex<bool>>,
    /// Every `since_state` argument this mock's `sync_messages` was called
    /// with, in call order, so tests can prove a caller threads the returned
    /// checkpoint back into the next sync (real incrementality).
    since_state_calls: Arc<Mutex<Vec<Option<String>>>>,
    /// The checkpoint token this mock returns from `sync_messages`.
    returned_state: Arc<Mutex<String>>,
    /// Every [`RemoteMutationSpec`] passed to `apply_mutation`, in call order,
    /// so a daemon E2E can prove the outbox executor genuinely invoked the
    /// backend op (not a fabricated completion).
    applied_mutations: Arc<Mutex<Vec<RemoteMutationSpec>>>,
}

impl MockMailBackend {
    /// Create a new `MockMailBackend` with empty storage.
    pub fn new() -> Self {
        let backend = Self::default();
        if let Ok(mut state) = backend.returned_state.lock() {
            *state = "mock-state-token-100".to_string();
        }
        backend
    }

    /// Override the checkpoint token this mock returns from `sync_messages`,
    /// so a test can control the exact state a caller must persist and thread
    /// back on the next sync.
    pub fn set_returned_state(&self, state: &str) {
        if let Ok(mut guard) = self.returned_state.lock() {
            *guard = state.to_string();
        }
    }

    /// Every `since_state` argument passed to `sync_messages`, in call order.
    /// A caller that genuinely resumes from a checkpoint will show `None` on
    /// the first call and the previously-returned state on the next.
    pub fn since_state_calls(&self) -> Vec<Option<String>> {
        self.since_state_calls
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// Configure the mock to simulate network failure errors.
    pub fn set_should_fail(&self, fail: bool) {
        if let Ok(mut flag) = self.should_fail.lock() {
            *flag = fail;
        }
    }

    /// Add a mock folder to the storage.
    pub fn add_folder(&self, folder: Folder) {
        if let Ok(mut guard) = self.folders.lock() {
            guard.push(folder);
        }
    }

    /// Add a mock message to the storage, as the occupancy a backend would
    /// have found it in. Taking a [`PlacedMessage`] rather than a bare `Email`
    /// lets a test stage the same message key in two folders and prove the
    /// caller treats it as one message in two places.
    pub fn add_message(&self, message: PlacedMessage) {
        if let Ok(mut guard) = self.messages.lock() {
            guard.push(message);
        }
    }

    /// Account every message staged by [`Self::shared_message_in`] and
    /// [`Self::with_same_message_in`] belongs to.
    pub const SHARED_ACCOUNT_ID: &'static str = "acct-1";

    /// The one derived message key every occupancy staged by
    /// [`Self::shared_message_in`] carries, whatever folder it sits in.
    pub const SHARED_MESSAGE_KEY: &'static str = "msg-shared-identity";

    /// One occupancy of the single shared message, in `folder_id` under
    /// `remote_id`.
    ///
    /// Identity is byte-identical across every folder -- that is the whole
    /// point: a caller cannot tell a first arrival in a second mailbox from a
    /// message it already stores unless it compares occupancies rather than
    /// message keys.
    pub fn shared_message_in(folder_id: &str, remote_id: &str) -> PlacedMessage {
        PlacedMessage {
            email: Email {
                id: Self::SHARED_MESSAGE_KEY.to_string(),
                account_id: Self::SHARED_ACCOUNT_ID.to_string(),
                subject: "Shared across folders".to_string(),
                sender: "alice@nuncio.mx".to_string(),
                recipient: "bob@nuncio.mx".to_string(),
                received_at: 1_700_000_000,
                body_plain: Some("one message, several mailboxes".to_string()),
                body_html: None,
                message_id: Some("shared@nuncio.mx".to_string()),
                content_hash: None,
                attachments: Vec::new(),
            },
            // EmailId, not Surrogate: a folder-independent key is exactly what
            // a server-assigned identity buys, and it is the tier under which
            // one message legitimately reports from several mailboxes.
            source: IdentitySource::EmailId,
            placement: Placement {
                account_id: Self::SHARED_ACCOUNT_ID.to_string(),
                folder_id: folder_id.to_string(),
                uid_validity: "1".to_string(),
                remote_id: remote_id.to_string(),
                read: false,
            },
        }
    }

    /// A backend serving `folders`, each holding the same message under its own
    /// `remote_id` -- one identity, one occupancy per folder.
    ///
    /// Stages the folders as well as the messages, so a caller that enumerates
    /// folders and then syncs each one sees every occupancy.
    pub fn with_same_message_in(folders: &[&str]) -> Self {
        let backend = Self::new();
        for (index, folder_id) in folders.iter().enumerate() {
            backend.add_folder(Folder {
                id: (*folder_id).to_string(),
                name: (*folder_id).to_string(),
                total_messages: 1,
                unread_messages: 1,
            });
            // A distinct UID per folder: UIDs are folder-scoped, so the same
            // mail in two mailboxes is addressed by two different ones.
            backend.add_message(Self::shared_message_in(folder_id, &(index + 1).to_string()));
        }
        backend
    }

    /// Every [`RemoteMutationSpec`] this mock's `apply_mutation` was called
    /// with, in call order. A failing configuration (`set_should_fail(true)`)
    /// records nothing, so a test can prove a failed op is never counted as
    /// applied.
    pub fn applied_mutations(&self) -> Vec<RemoteMutationSpec> {
        self.applied_mutations
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }
}

#[async_trait]
impl MailBackend for MockMailBackend {
    async fn sync_folders(&self) -> Result<Vec<Folder>, MailError> {
        let should_fail = self
            .should_fail
            .lock()
            .map_err(|e| MailError::ParseFailed(e.to_string()))?;
        if *should_fail {
            return Err(MailError::ParseFailed(
                "simulated network failure".to_string(),
            ));
        }
        let folders = self
            .folders
            .lock()
            .map_err(|e| MailError::ParseFailed(e.to_string()))?;
        Ok(folders.clone())
    }

    async fn sync_changes(
        &self,
        folder_id: &str,
        since_state: Option<&str>,
    ) -> Result<FolderChanges, MailError> {
        // Record the checkpoint arg before any early return, so even a failing
        // call is visible to a test asserting on how the caller resumes.
        if let Ok(mut calls) = self.since_state_calls.lock() {
            calls.push(since_state.map(str::to_string));
        }

        let should_fail = self
            .should_fail
            .lock()
            .map_err(|e| MailError::ParseFailed(e.to_string()))?;
        if *should_fail {
            return Err(MailError::ParseFailed(
                "simulated network failure".to_string(),
            ));
        }
        let returned_state = self
            .returned_state
            .lock()
            .map_err(|e| MailError::ParseFailed(e.to_string()))?
            .clone();

        // Simulate a since_state-aware backend: with a prior checkpoint there
        // is nothing new to hand back, so the incremental fetch is genuinely
        // narrower (empty) rather than a full re-report of every message.
        if since_state.is_some() {
            return Ok(FolderChanges {
                next_state: returned_state,
                ..Default::default()
            });
        }

        let messages = self
            .messages
            .lock()
            .map_err(|e| MailError::ParseFailed(e.to_string()))?;
        let matches: Vec<_> = messages
            .iter()
            .filter(|m| m.placement.folder_id == folder_id)
            .cloned()
            .collect();
        // A full pass over the mock's store did see everything in the folder,
        // so it can honestly report what is present -- as occupancies, since
        // that is what a folder can speak to.
        let present = matches.iter().map(|m| m.placement.key()).collect();
        Ok(FolderChanges {
            upserts: matches,
            removals: Vec::new(),
            present: Some(present),
            next_state: returned_state,
        })
    }

    async fn apply_mutation(
        &self,
        spec: &RemoteMutationSpec,
    ) -> Result<MutationOutcome, MailError> {
        let should_fail = self
            .should_fail
            .lock()
            .map_err(|e| MailError::ParseFailed(e.to_string()))?;
        if *should_fail {
            return Err(MailError::ImapError(
                "simulated remote mutation failure".to_string(),
            ));
        }
        // Record only genuinely-applied mutations, so a test can distinguish a
        // real op from a fabricated completion.
        let mut applied = self
            .applied_mutations
            .lock()
            .map_err(|e| MailError::ParseFailed(e.to_string()))?;
        applied.push(spec.clone());
        Ok(MutationOutcome::Applied { token: None })
    }
}

/// Deterministic mock [`MessageSender`] for offline testing of the outbound
/// send RPC: records every [`OutboundMessage`] it is given, rather than
/// touching a real SMTP transport, and can be
/// configured to simulate a transport failure so callers can prove a failed
/// send surfaces as a genuine error rather than a fabricated success.
#[derive(Debug, Clone, Default)]
pub struct MockMessageSender {
    sent: Arc<Mutex<Vec<OutboundMessage>>>,
    should_fail: Arc<Mutex<bool>>,
}

impl MockMessageSender {
    /// Create a new `MockMessageSender` with empty storage.
    pub fn new() -> Self {
        Self::default()
    }

    /// Configure the mock to simulate a transport-level send failure.
    pub fn set_should_fail(&self, fail: bool) {
        if let Ok(mut flag) = self.should_fail.lock() {
            *flag = fail;
        }
    }

    /// Retrieve every message recorded by the mock, in send order.
    pub fn sent_messages(&self) -> Vec<OutboundMessage> {
        self.sent
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }
}

#[async_trait]
impl MessageSender for MockMessageSender {
    async fn send(&self, message: &OutboundMessage) -> Result<(), MailError> {
        let should_fail = self
            .should_fail
            .lock()
            .map_err(|e| MailError::ParseFailed(e.to_string()))?;
        if *should_fail {
            return Err(MailError::TransportFailed(
                "simulated SMTP transport failure".to_string(),
            ));
        }
        let mut sent = self
            .sent
            .lock()
            .map_err(|e| MailError::ParseFailed(e.to_string()))?;
        sent.push(message.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_backend_folder_and_message_operations() {
        let mock = MockMailBackend::new();
        mock.add_folder(Folder {
            id: "inbox".to_string(),
            name: "Inbox".to_string(),
            total_messages: 1,
            unread_messages: 1,
        });

        let placed = PlacedMessage {
            email: Email {
                id: "msg-mock-1".to_string(),
                account_id: "acct-1".to_string(),
                subject: "Mock Test".to_string(),
                sender: "alice@nuncio.mx".to_string(),
                recipient: "bob@nuncio.mx".to_string(),
                received_at: 1700000000,
                body_plain: Some("Mock body".to_string()),
                body_html: None,
                attachments: Vec::new(),
                message_id: None,
                content_hash: None,
            },
            source: IdentitySource::Surrogate,
            placement: Placement {
                account_id: "acct-1".to_string(),
                folder_id: "inbox".to_string(),
                uid_validity: "1".to_string(),
                remote_id: "1".to_string(),
                read: false,
            },
        };
        mock.add_message(placed.clone());

        let folders = mock.sync_folders().await.expect("sync folders succeeds");
        assert_eq!(folders.len(), 1);

        let (msgs, state) = mock
            .sync_messages("inbox", None)
            .await
            .expect("sync messages succeeds");
        assert_eq!(msgs.len(), 1);
        assert_eq!(
            msgs[0], placed,
            "the mock must hand back exactly the placement it was staged with"
        );
        assert_eq!(state, "mock-state-token-100");

        // Test error simulation
        mock.set_should_fail(true);
        assert!(mock.sync_folders().await.is_err());
        assert!(mock.sync_messages("inbox", None).await.is_err());
    }

    /// The staging helper is only useful if it really produces one identity in
    /// two places: same message key, different occupancies. If it ever drifted
    /// into two keys, every test built on it would pass for the wrong reason.
    #[tokio::test]
    async fn with_same_message_in_stages_one_identity_across_every_folder() {
        let mock = MockMailBackend::with_same_message_in(&["INBOX", "Archive"]);

        let folders = mock.sync_folders().await.expect("sync folders succeeds");
        let folder_ids: Vec<&str> = folders.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(folder_ids, vec!["INBOX", "Archive"]);

        let inbox = mock
            .sync_changes("INBOX", None)
            .await
            .expect("inbox pass succeeds");
        let archive = mock
            .sync_changes("Archive", None)
            .await
            .expect("archive pass succeeds");
        assert_eq!(inbox.upserts.len(), 1);
        assert_eq!(archive.upserts.len(), 1);

        assert_eq!(
            inbox.upserts[0].email.id, archive.upserts[0].email.id,
            "both occupancies must carry the same derived message key"
        );
        assert_ne!(
            inbox.upserts[0].placement.key(),
            archive.upserts[0].placement.key(),
            "the occupancies themselves must differ, or there is nothing to classify"
        );
    }

    fn sample_outbound_message() -> OutboundMessage {
        OutboundMessage {
            from: "alice@nuncio.mx".to_string(),
            to: "bob@nuncio.mx".to_string(),
            cc: None,
            subject: "Mock Send".to_string(),
            body_plain: Some("Mock body".to_string()),
            body_html: None,
            attachments: Vec::new(),
        }
    }

    #[tokio::test]
    async fn mock_message_sender_records_exact_message_and_supports_failure_simulation() {
        let mock = MockMessageSender::new();
        let message = sample_outbound_message();

        mock.send(&message).await.expect("send succeeds");
        let sent = mock.sent_messages();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0], message);

        mock.set_should_fail(true);
        let err = mock
            .send(&message)
            .await
            .expect_err("simulated failure must surface as an error");
        assert!(matches!(err, MailError::TransportFailed(_)));
        // The failed attempt must never be recorded as "sent".
        assert_eq!(mock.sent_messages().len(), 1);
    }
}
