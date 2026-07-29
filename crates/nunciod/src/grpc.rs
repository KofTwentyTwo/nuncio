//! gRPC server wiring for `nunciod`: serves the versioned `nuncio.v1.System`
//! contract (see `nuncio-proto`) over loopback, authenticated by a bearer
//! token sourced from the OS keyring vault
//! (`nuncio_store::vault::GRPC_TOKEN_ACCOUNT`).
//!
//! This is the daemon's sole client-facing transport.

use nuncio_cal::CalendarBackend;
use nuncio_contacts::ContactsBackend;
use nuncio_core::{CoreCommand, CoreEvent, EventBus};
use nuncio_filter::{FilterEngine, NsqlParser, NsqlValidator, ValidationOptions};
use nuncio_mail::{MailBackend, MessageSender};
use nuncio_proto::v1::accounts_server::{Accounts, AccountsServer};
use nuncio_proto::v1::audit_server::{Audit, AuditServer};
use nuncio_proto::v1::calendar_server::{Calendar, CalendarServer};
use nuncio_proto::v1::contacts_server::{Contacts, ContactsServer};
use nuncio_proto::v1::event::Kind;
use nuncio_proto::v1::export_server::{Export, ExportServer};
use nuncio_proto::v1::filters_server::{Filters, FiltersServer};
use nuncio_proto::v1::mail_server::{Mail, MailServer};
use nuncio_proto::v1::system_server::{System, SystemServer};
use nuncio_proto::v1::{
    export_request, AccountConfig as AccountConfigProto, AccountProtocol as AccountProtocolProto,
    AddAccountRequest, AddAccountResponse, Attachment as AttachmentProto,
    AuditRecord as AuditRecordProto, BatchFilterProgress, CalendarEvent as CalendarEventProto,
    CalendarSyncRequest, CalendarSyncResponse, Contact as ContactProto,
    ContactEmail as ContactEmailProto, ContactPhone as ContactPhoneProto, ContactsSyncRequest,
    ContactsSyncResponse, CreateContactRequest, CreateContactResponse, CreateRuleRequest,
    CreateRuleResponse, DatabaseRecovered, DeleteRuleRequest, DeleteRuleResponse, Event,
    EventError, ExportFormat as ExportFormatProto, ExportRequest, ExportResponse, FilterExecuted,
    FilterRule as FilterRuleProto, Folder as FolderProto, GetContactRequest, GetContactResponse,
    GetEventRequest, GetEventResponse, GetMessageRequest, GetMessageResponse, GetStatusRequest,
    GetStatusResponse, ListAccountsRequest, ListAccountsResponse, ListContactsRequest,
    ListContactsResponse, ListEventsRequest, ListEventsResponse, ListFoldersRequest,
    ListFoldersResponse, ListMessagesRequest, ListMessagesResponse, ListRecordsRequest,
    ListRecordsResponse, ListRulesRequest, ListRulesResponse, MarkReadRequest, MarkReadResponse,
    Message as MessageProto, MessageFlagsChanged, MessageSearchHit, PreviewRuleRequest,
    PreviewRuleResponse, SearchMessagesRequest, SearchMessagesResponse, SendMessageRequest,
    SendMessageResponse, ShuttingDown, SubscribeRequest, SyncCompleted, SyncRequest, SyncResponse,
    SyncStarted, TlsMode as TlsModeProto, UpdateAvailable, ValidateRuleRequest,
    ValidateRuleResponse, VerifyChainRequest, VerifyChainResponse,
};
use nuncio_store::db::DatabaseEngine;
use nuncio_store::search::SearchEngine;
use nuncio_store::vault::SecretManager;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;
use thiserror::Error;
use tokio::net::TcpListener;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::{BroadcastStream, TcpListenerStream};
use tokio_stream::{Stream, StreamExt};
use tonic::transport::Server;
use tonic::{Request, Response, Status};

// The gRPC loopback address defaults/resolver live in `nuncio-proto` (see
// `nuncio_proto::addr`) so that thin presentation-shell clients (e.g.
// `nuncio-cli`) agree on the same default address without depending on this
// `nunciod` binary crate. Re-exported here so existing callers of
// `nunciod::grpc::{DEFAULT_GRPC_ADDR, GRPC_ADDR_ENV_VAR, grpc_addr_from_env}`
// keep working unchanged.
pub use nuncio_proto::{grpc_addr_from_env, DEFAULT_GRPC_ADDR, GRPC_ADDR_ENV_VAR};

/// Errors that can occur while binding or running the `nuncio.v1` gRPC server.
#[derive(Debug, Error)]
pub enum GrpcServeError {
    /// Failed to bind the loopback TCP listener.
    #[error("failed to bind gRPC listener on {addr}: {source}")]
    Bind {
        /// The address that failed to bind.
        addr: String,
        /// Underlying IO error.
        #[source]
        source: std::io::Error,
    },
    /// The tonic transport server failed while serving requests.
    #[error("gRPC transport server failed: {0}")]
    Transport(#[from] tonic::transport::Error),
}

/// Maps a `nuncio_core::CoreEvent` domain event onto its wire-format
/// `nuncio.v1.Event` representation, faithfully carrying every variant's
/// fields into the matching `oneof` case (see `proto/nuncio/v1/nuncio.proto`
/// for the wire contract).
///
/// `usize` fields (`processed`, `total`, `matched`, `salvaged_rules_count`)
/// are narrowed to `u64` for the wire; these are in-process counters that
/// cannot realistically approach `u64::MAX`, and protobuf has no native
/// `usize` type.
fn map_core_event(event: CoreEvent) -> Event {
    let kind = match event {
        CoreEvent::SyncStarted { account_id } => Kind::SyncStarted(SyncStarted { account_id }),
        CoreEvent::SyncCompleted { account_id } => {
            Kind::SyncCompleted(SyncCompleted { account_id })
        }
        CoreEvent::MessageFlagsChanged { message_id, read } => {
            Kind::MessageFlagsChanged(MessageFlagsChanged { message_id, read })
        }
        CoreEvent::FilterExecuted {
            rule_id,
            message_id,
            action_taken,
        } => Kind::FilterExecuted(FilterExecuted {
            rule_id,
            message_id,
            action_taken,
        }),
        CoreEvent::BatchFilterProgress {
            processed,
            total,
            matched,
        } => Kind::BatchFilterProgress(BatchFilterProgress {
            processed: processed as u64,
            total: total as u64,
            matched: matched as u64,
        }),
        CoreEvent::DatabaseRecovered {
            backup_path,
            salvaged_rules_count,
            resync_triggered,
        } => Kind::DatabaseRecovered(DatabaseRecovered {
            backup_path,
            salvaged_rules_count: salvaged_rules_count as u64,
            resync_triggered,
        }),
        CoreEvent::UpdateAvailable {
            version,
            release_notes,
        } => Kind::UpdateAvailable(UpdateAvailable {
            version,
            release_notes,
        }),
        CoreEvent::Error { message } => Kind::Error(EventError { message }),
        CoreEvent::ShuttingDown => Kind::ShuttingDown(ShuttingDown {}),
    };
    Event { kind: Some(kind) }
}

/// `nuncio.v1.System` gRPC service implementation backed by the daemon's live
/// [`EventBus`] state.
struct SystemGrpcService {
    event_bus: Arc<EventBus>,
}

#[tonic::async_trait]
impl System for SystemGrpcService {
    async fn get_status(
        &self,
        _request: Request<GetStatusRequest>,
    ) -> Result<Response<GetStatusResponse>, Status> {
        let state = self.event_bus.current_state();
        Ok(Response::new(GetStatusResponse {
            engine_status: format!("{:?}", state.status),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }))
    }

    /// Server-streaming push feed of daemon domain events. Defined on
    /// `System` (not a separate service) so this RPC is automatically
    /// covered by the same `BearerAuthInterceptor` that guards `GetStatus`,
    /// rather than risking a second, un-intercepted service being mounted by
    /// mistake.
    ///
    /// The subscription is registered (`event_bus.subscribe_events()`)
    /// synchronously before this method returns, so once a client's
    /// `subscribe` call resolves, any event published afterwards is
    /// guaranteed to be observed on the returned stream — no fixed sleep is
    /// needed by callers to avoid a race between "start subscribing" and
    /// "event published".
    ///
    /// A lagging subscriber (`BroadcastStreamRecvError::Lagged`) is handled
    /// by logging a warning and skipping to the next available event rather
    /// than terminating the stream or panicking; the daemon's event bus is
    /// a best-effort broadcast feed, not a durable log, so a slow subscriber
    /// missing some events is preferable to it never recovering. The stream
    /// ends (returns `None`) only when the event bus itself is closed (all
    /// `EventBus` senders dropped) or the client disconnects.
    type SubscribeStream = Pin<Box<dyn Stream<Item = Result<Event, Status>> + Send + 'static>>;

    async fn subscribe(
        &self,
        _request: Request<SubscribeRequest>,
    ) -> Result<Response<Self::SubscribeStream>, Status> {
        let receiver = self.event_bus.subscribe_events();
        let mapped = BroadcastStream::new(receiver).filter_map(|item| match item {
            Ok(core_event) => Some(Ok(map_core_event(core_event))),
            Err(BroadcastStreamRecvError::Lagged(skipped)) => {
                tracing::warn!(
                    "gRPC Subscribe stream lagged behind the daemon event bus; skipped {} \
                     event(s); resuming from the next available event",
                    skipped
                );
                None
            }
        });
        Ok(Response::new(Box::pin(mapped)))
    }
}

/// Maps a `nuncio_core::AccountProtocol` onto its wire-format
/// `nuncio.v1.AccountProtocol` enum value.
fn map_account_protocol_to_proto(protocol: nuncio_core::AccountProtocol) -> AccountProtocolProto {
    match protocol {
        nuncio_core::AccountProtocol::Jmap => AccountProtocolProto::Jmap,
        nuncio_core::AccountProtocol::ImapSmtp => AccountProtocolProto::ImapSmtp,
    }
}

/// Maps a wire-format `nuncio.v1.AccountProtocol` enum value back onto
/// `nuncio_core::AccountProtocol`. `Unspecified` is treated as an invalid
/// request rather than silently defaulting to a protocol the caller never
/// asked for.
fn map_account_protocol_from_proto(
    protocol: AccountProtocolProto,
) -> Result<nuncio_core::AccountProtocol, Status> {
    match protocol {
        AccountProtocolProto::Jmap => Ok(nuncio_core::AccountProtocol::Jmap),
        AccountProtocolProto::ImapSmtp => Ok(nuncio_core::AccountProtocol::ImapSmtp),
        AccountProtocolProto::Unspecified => {
            Err(Status::invalid_argument("account protocol is required"))
        }
    }
}

/// Maps a `nuncio_core::TlsMode` onto its wire-format `nuncio.v1.TlsMode`
/// enum value.
fn map_tls_mode_to_proto(mode: nuncio_core::TlsMode) -> TlsModeProto {
    match mode {
        nuncio_core::TlsMode::ImplicitTls => TlsModeProto::ImplicitTls,
        nuncio_core::TlsMode::StartTls => TlsModeProto::StartTls,
        nuncio_core::TlsMode::Plain => TlsModeProto::Plain,
    }
}

/// Maps a wire-format `nuncio.v1.TlsMode` enum value back onto
/// `nuncio_core::TlsMode`. `Unspecified` is rejected rather than silently
/// defaulting.
fn map_tls_mode_from_proto(mode: TlsModeProto) -> Result<nuncio_core::TlsMode, Status> {
    match mode {
        TlsModeProto::ImplicitTls => Ok(nuncio_core::TlsMode::ImplicitTls),
        TlsModeProto::StartTls => Ok(nuncio_core::TlsMode::StartTls),
        TlsModeProto::Plain => Ok(nuncio_core::TlsMode::Plain),
        TlsModeProto::Unspecified => Err(Status::invalid_argument("tls mode is required")),
    }
}

/// Maps a `nuncio_core::AccountConfig` onto its wire-format
/// `nuncio.v1.AccountConfig` representation for `ListAccounts` responses.
/// The password credential is NEVER part of `AccountConfig` (it lives only
/// in the OS keyring, keyed by `keyring_secret_key`), so there is nothing to
/// scrub here -- there is simply no field to carry it.
fn map_account_config_to_proto(config: nuncio_core::AccountConfig) -> AccountConfigProto {
    AccountConfigProto {
        id: config.id,
        name: config.name,
        email_address: config.email_address,
        protocol: map_account_protocol_to_proto(config.protocol).into(),
        server_host: config.server_host,
        server_port: u32::from(config.server_port),
        use_tls: config.use_tls,
        imap_tls_mode: map_tls_mode_to_proto(config.imap_tls_mode).into(),
        smtp_tls_mode: map_tls_mode_to_proto(config.smtp_tls_mode).into(),
        keyring_secret_key: config.keyring_secret_key,
        sync_interval_secs: config.sync_interval_secs,
        smtp_host: config.smtp_host,
        smtp_port: u32::from(config.smtp_port),
    }
}

/// Maps a wire-format `nuncio.v1.AccountConfig` request payload back onto
/// `nuncio_core::AccountConfig`. Rejects a `server_port` outside `1..=65535`
/// with `Status::invalid_argument` rather than silently truncating it.
fn map_account_config_from_proto(
    config: AccountConfigProto,
) -> Result<nuncio_core::AccountConfig, Status> {
    let protocol = map_account_protocol_from_proto(config.protocol())?;
    let imap_tls_mode = map_tls_mode_from_proto(config.imap_tls_mode())?;
    let smtp_tls_mode = map_tls_mode_from_proto(config.smtp_tls_mode())?;
    let server_port = u16::try_from(config.server_port)
        .map_err(|_| Status::invalid_argument("server_port must be in range 1..=65535"))?;
    let smtp_port = u16::try_from(config.smtp_port)
        .map_err(|_| Status::invalid_argument("smtp_port must be in range 1..=65535"))?;

    Ok(nuncio_core::AccountConfig {
        id: config.id,
        name: config.name,
        email_address: config.email_address,
        protocol,
        server_host: config.server_host,
        server_port,
        smtp_host: config.smtp_host,
        smtp_port,
        use_tls: config.use_tls,
        imap_tls_mode,
        smtp_tls_mode,
        keyring_secret_key: config.keyring_secret_key,
        sync_interval_secs: config.sync_interval_secs,
    })
}

/// `nuncio.v1.Accounts` gRPC service implementation backed by the daemon's
/// live [`DatabaseEngine`] and [`SecretManager`] vault.
///
/// Persists [`nuncio_core::AccountConfig`] rows via `DatabaseEngine::save_account`
/// and routes the password credential exclusively to the OS keyring vault via
/// `SecretManager::set_secret` -- the password is never written to SQLite,
/// never logged, and never echoed back in any RPC response.
struct AccountsGrpcService {
    db: Arc<DatabaseEngine>,
    secrets: Arc<SecretManager>,
}

#[tonic::async_trait]
impl Accounts for AccountsGrpcService {
    async fn add_account(
        &self,
        request: Request<AddAccountRequest>,
    ) -> Result<Response<AddAccountResponse>, Status> {
        let req = request.into_inner();
        let proto_config = req
            .config
            .ok_or_else(|| Status::invalid_argument("config is required"))?;
        if req.password.is_empty() {
            return Err(Status::invalid_argument("password is required"));
        }

        let config = map_account_config_from_proto(proto_config)?;
        config
            .validate()
            .map_err(|e| Status::invalid_argument(e.to_string()))?;

        // Store the password credential in the OS keyring FIRST, keyed by
        // this account's `keyring_secret_key` -- it is never written to
        // SQLite. If this fails, the account row below is deliberately
        // never saved, so a failed credential write can never leave behind
        // an account config with no retrievable password.
        self.secrets
            .set_secret(&config.keyring_secret_key, &req.password)
            .map_err(|e| Status::internal(format!("failed to store credential in vault: {e}")))?;

        self.db
            .save_account(&config)
            .await
            .map_err(|e| Status::internal(format!("failed to persist account: {e}")))?;

        Ok(Response::new(AddAccountResponse { id: config.id }))
    }

    async fn list_accounts(
        &self,
        _request: Request<ListAccountsRequest>,
    ) -> Result<Response<ListAccountsResponse>, Status> {
        let accounts = self
            .db
            .list_accounts()
            .await
            .map_err(|e| Status::internal(format!("failed to list accounts: {e}")))?
            .into_iter()
            .map(map_account_config_to_proto)
            .collect();

        Ok(Response::new(ListAccountsResponse { accounts }))
    }
}

/// Default cap on messages returned by `ListMessages` when the caller
/// supplies `limit: 0` ("use the server default").
const DEFAULT_LIST_MESSAGES_LIMIT: usize = 50;

/// Maps a `nuncio_core::model::Attachment` onto its wire-format
/// `nuncio.v1.Attachment` representation.
fn map_attachment_to_proto(attachment: nuncio_core::model::Attachment) -> AttachmentProto {
    AttachmentProto {
        filename: attachment.filename,
        mime_type: attachment.mime_type,
        content: attachment.content.to_vec(),
    }
}

/// Maps a wire-format `nuncio.v1.Attachment` request payload back onto
/// `nuncio_core::model::Attachment`, used by `SendMessage`.
fn map_attachment_from_proto(attachment: AttachmentProto) -> nuncio_core::model::Attachment {
    nuncio_core::model::Attachment {
        filename: attachment.filename,
        mime_type: attachment.mime_type,
        content: bytes::Bytes::from(attachment.content),
    }
}

/// Maps a `nuncio_core::model::Email` onto its wire-format `nuncio.v1.Message`
/// representation. The store (`DatabaseEngine::get_message` /
/// `list_messages`) already returns `body_plain`/`body_html` decrypted, so
/// there is no further decryption to do here -- just field-for-field
/// mapping.
fn map_email_to_proto(email: nuncio_core::model::Email) -> MessageProto {
    MessageProto {
        id: email.id,
        account_id: email.account_id,
        folder_id: email.folder_id,
        subject: email.subject,
        sender: email.sender,
        recipient: email.recipient,
        received_at: email.received_at,
        read: email.read,
        body_plain: email.body_plain,
        body_html: email.body_html,
        attachments: email
            .attachments
            .into_iter()
            .map(map_attachment_to_proto)
            .collect(),
    }
}

/// Maps a `nuncio_core::model::Folder` onto its wire-format `nuncio.v1.Folder`
/// representation. `usize` counts are narrowed to `u64` for the wire; these
/// are message counts that cannot realistically approach `u64::MAX`, and
/// protobuf has no native `usize` type.
fn map_folder_to_proto(folder: nuncio_core::model::Folder) -> FolderProto {
    FolderProto {
        id: folder.id,
        name: folder.name,
        total_messages: folder.total_messages as u64,
        unread_messages: folder.unread_messages as u64,
    }
}

/// Maps a `nuncio_core::model::CalendarEvent` onto its wire-format
/// `nuncio.v1.CalendarEvent` representation. Field-for-field, no
/// decryption/derivation to do here -- calendar events are never encrypted
/// at rest (see `DatabaseEngine::save_calendar_event`'s doc comment).
fn map_calendar_event_to_proto(event: nuncio_core::model::CalendarEvent) -> CalendarEventProto {
    CalendarEventProto {
        id: event.id,
        account_id: event.account_id,
        calendar_id: event.calendar_id,
        summary: event.summary,
        start_time: event.start_time,
        end_time: event.end_time,
        rrule: event.rrule,
        location: event.location,
    }
}

/// Maps a `nuncio_contacts::Contact` onto its wire-format `nuncio.v1.Contact`
/// representation. The internal bookkeeping timestamps
/// (`created_at`/`updated_at`/`last_interacted_at`) are deliberately not
/// carried over -- see the `Contact` message's doc comment in the `.proto`
/// source.
fn map_contact_to_proto(contact: nuncio_contacts::Contact) -> ContactProto {
    ContactProto {
        id: contact.id,
        account_id: contact.account_id,
        display_name: contact.display_name,
        given_name: contact.given_name,
        family_name: contact.family_name,
        organization: contact.organization,
        job_title: contact.job_title,
        notes: contact.notes,
        avatar_url: contact.avatar_url,
        emails: contact
            .emails
            .into_iter()
            .map(|e| ContactEmailProto {
                email: e.email,
                label: e.label,
                is_primary: e.is_primary,
            })
            .collect(),
        phones: contact
            .phones
            .into_iter()
            .map(|p| ContactPhoneProto {
                phone: p.phone,
                label: p.label,
                is_primary: p.is_primary,
            })
            .collect(),
        is_favorite: contact.is_favorite,
        interaction_count: contact.interaction_count,
    }
}

/// Combines an account's in-window events with its recurring masters into the final event
/// list for a `ListEvents` query window, expanding each recurring master into its real
/// occurrences via [`nuncio_cal::RecurrenceEngine::expand_occurrences`] rather than trusting
/// the master row's own stored `start_time`/`end_time` to represent every future instance.
///
/// `recurring` events are never sourced from `all_in_window` for the final result -- only
/// `e.rrule.is_none()` rows from `all_in_window` are kept verbatim -- so a recurring master
/// that also happens to overlap the window in `all_in_window` is never emitted a second time
/// alongside its expanded occurrences.
///
/// A recurring event whose `rrule` fails to parse degrades to the raw stored master (a genuine persisted row -- just unexpanded, not fabricated data) with a warning
/// logged, rather than failing the whole query for one malformed rule among many.
fn expand_events_for_window(
    all_in_window: Vec<nuncio_core::model::CalendarEvent>,
    recurring: Vec<nuncio_core::model::CalendarEvent>,
    start_window: i64,
    end_window: i64,
) -> Vec<nuncio_core::model::CalendarEvent> {
    let mut events: Vec<nuncio_core::model::CalendarEvent> = all_in_window
        .into_iter()
        .filter(|e| e.rrule.is_none())
        .collect();

    for event in recurring {
        match nuncio_cal::RecurrenceEngine::expand_occurrences(&event, start_window, end_window) {
            Ok(occurrences) => events.extend(occurrences),
            Err(e) => {
                tracing::warn!(
                    "Calendar: failed to expand rrule for event '{}', falling back to raw \
                     stored event: {}",
                    event.id,
                    e
                );
                events.push(event);
            }
        }
    }

    events
}

/// Test-only injection point for the daemon's calendar sync backend,
/// mirroring [`MailEngineOverrides`]'s shape.
///
/// Production (every existing caller of [`serve`] / [`serve_on_listener`])
/// never constructs a non-default instance: [`Default`] yields
/// `calendar_backend: None`, which routes `Calendar/Sync` to an honest
/// error, since there is currently no persisted per-account CalDAV
/// configuration to build a real backend from. Only a full-daemon E2E test
/// supplies `Some(..)`, standing in a [`nuncio_cal::MockCalendarBackend`]
/// for real network I/O.
#[derive(Clone, Default)]
pub struct CalendarEngineOverrides {
    /// When `Some`, `Calendar/Sync` fetches from this backend instead of
    /// returning an honest "no CalDAV configuration" error.
    pub calendar_backend: Option<Arc<dyn CalendarBackend>>,
}

impl std::fmt::Debug for CalendarEngineOverrides {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CalendarEngineOverrides")
            .field("calendar_backend", &self.calendar_backend.is_some())
            .finish()
    }
}

/// `nuncio.v1.Calendar` gRPC service implementation backed by the daemon's
/// live [`DatabaseEngine`] calendar-event read/write methods.
///
/// `ListEvents`/`GetEvent` always read genuinely persisted data. `Sync`
/// fetches from [`CalendarEngineOverrides::calendar_backend`] when injected;
/// otherwise it returns an honest error, since there is no persisted
/// per-account CalDAV configuration to build a real backend from yet (see
/// `nunciod::calendar_sync`'s doc comment).
struct CalendarGrpcService {
    db: Arc<DatabaseEngine>,
    overrides: CalendarEngineOverrides,
}

#[tonic::async_trait]
impl Calendar for CalendarGrpcService {
    async fn sync(
        &self,
        request: Request<CalendarSyncRequest>,
    ) -> Result<Response<CalendarSyncResponse>, Status> {
        let req = request.into_inner();

        let synced = match &self.overrides.calendar_backend {
            Some(backend) => crate::calendar_sync::sync_with_backend(
                &self.db,
                backend.as_ref(),
                &req.calendar_id,
                req.start_window,
                req.end_window,
            )
            .await
            .map_err(|e| Status::internal(format!("calendar sync failed: {e}")))?,
            None => {
                return Err(Status::internal(format!(
                    "no CalDAV configuration exists for account '{}'; per-account CalDAV \
                     configuration is not yet implemented",
                    req.account_id
                )));
            }
        };

        Ok(Response::new(CalendarSyncResponse {
            synced_count: synced as u64,
        }))
    }

    async fn list_events(
        &self,
        request: Request<ListEventsRequest>,
    ) -> Result<Response<ListEventsResponse>, Status> {
        let req = request.into_inner();
        if req.account_id.is_empty() {
            return Err(Status::invalid_argument("account_id is required"));
        }

        let all_in_window = self
            .db
            .list_calendar_events(&req.account_id, req.start_window, req.end_window)
            .await
            .map_err(|e| Status::internal(format!("failed to list calendar events: {e}")))?;

        let recurring = self
            .db
            .list_recurring_calendar_events(&req.account_id)
            .await
            .map_err(|e| Status::internal(format!("failed to list recurring events: {e}")))?;

        let mut events =
            expand_events_for_window(all_in_window, recurring, req.start_window, req.end_window);

        events.retain(|e| req.calendar_id.is_empty() || e.calendar_id == req.calendar_id);

        let events = events
            .into_iter()
            .map(map_calendar_event_to_proto)
            .collect();

        Ok(Response::new(ListEventsResponse { events }))
    }

    async fn get_event(
        &self,
        request: Request<GetEventRequest>,
    ) -> Result<Response<GetEventResponse>, Status> {
        let req = request.into_inner();
        if req.event_id.is_empty() {
            return Err(Status::invalid_argument("event_id is required"));
        }

        let event = self
            .db
            .get_calendar_event(&req.event_id)
            .await
            .map_err(|e| Status::not_found(format!("event '{}' not found: {e}", req.event_id)))?;

        Ok(Response::new(GetEventResponse {
            event: Some(map_calendar_event_to_proto(event)),
        }))
    }
}

/// Test-only injection point for the daemon's real per-account CardDAV
/// contacts backend, mirroring [`CalendarEngineOverrides`] exactly.
///
/// Production (every existing caller of [`serve`] / [`serve_on_listener`])
/// never constructs a non-default instance: [`Default`] yields `None`,
/// which routes `Contacts/Sync` to an honest "no CardDAV configuration"
/// error, since `nuncio_core::AccountConfig` has no CardDAV collection
/// URL/credential to build a real per-account `CardDavClient` from yet. Only
/// a full-daemon E2E test supplies `Some(..)`, standing in a
/// [`nuncio_contacts::MockContactsBackend`] for real network I/O.
#[derive(Clone, Default)]
pub struct ContactsEngineOverrides {
    /// When `Some`, `Contacts/Sync` fetches from this backend instead of
    /// returning an honest "no CardDAV configuration" error.
    pub contacts_backend: Option<Arc<dyn ContactsBackend>>,
}

impl std::fmt::Debug for ContactsEngineOverrides {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContactsEngineOverrides")
            .field("contacts_backend", &self.contacts_backend.is_some())
            .finish()
    }
}

/// `nuncio.v1.Contacts` gRPC service implementation backed by the daemon's
/// live [`DatabaseEngine`] contact read/write methods.
///
/// `ListContacts`/`GetContact` always read genuinely persisted data.
/// `CreateContact` persists a locally-authored contact directly to the
/// store -- real local persistence, never a fabricated CardDAV write-back.
/// `Sync` fetches from [`ContactsEngineOverrides::contacts_backend`] when
/// injected; otherwise it returns an honest error, since there is no
/// persisted per-account CardDAV configuration to build a real backend from
/// yet (see `nunciod::contacts_sync`'s doc comment).
struct ContactsGrpcService {
    db: Arc<DatabaseEngine>,
    overrides: ContactsEngineOverrides,
}

#[tonic::async_trait]
impl Contacts for ContactsGrpcService {
    async fn sync(
        &self,
        request: Request<ContactsSyncRequest>,
    ) -> Result<Response<ContactsSyncResponse>, Status> {
        let req = request.into_inner();

        let synced = match &self.overrides.contacts_backend {
            Some(backend) => {
                crate::contacts_sync::sync_with_backend(&self.db, backend.as_ref(), &req.account_id)
                    .await
                    .map_err(|e| Status::internal(format!("contacts sync failed: {e}")))?
            }
            None => {
                return Err(Status::internal(format!(
                    "no CardDAV configuration exists for account '{}'; per-account CardDAV \
                     configuration is not yet implemented",
                    req.account_id
                )));
            }
        };

        Ok(Response::new(ContactsSyncResponse {
            synced_count: synced as u64,
        }))
    }

    async fn list_contacts(
        &self,
        request: Request<ListContactsRequest>,
    ) -> Result<Response<ListContactsResponse>, Status> {
        let req = request.into_inner();
        if req.account_id.is_empty() {
            return Err(Status::invalid_argument("account_id is required"));
        }

        let contacts = self
            .db
            .list_contacts(&req.account_id)
            .await
            .map_err(|e| Status::internal(format!("failed to list contacts: {e}")))?
            .into_iter()
            .map(map_contact_to_proto)
            .collect();

        Ok(Response::new(ListContactsResponse { contacts }))
    }

    async fn get_contact(
        &self,
        request: Request<GetContactRequest>,
    ) -> Result<Response<GetContactResponse>, Status> {
        let req = request.into_inner();
        if req.contact_id.is_empty() {
            return Err(Status::invalid_argument("contact_id is required"));
        }

        let contact = self.db.get_contact(&req.contact_id).await.map_err(|e| {
            Status::not_found(format!("contact '{}' not found: {e}", req.contact_id))
        })?;

        Ok(Response::new(GetContactResponse {
            contact: Some(map_contact_to_proto(contact)),
        }))
    }

    async fn create_contact(
        &self,
        request: Request<CreateContactRequest>,
    ) -> Result<Response<CreateContactResponse>, Status> {
        let req = request.into_inner();
        if req.account_id.is_empty() {
            return Err(Status::invalid_argument("account_id is required"));
        }
        if req.display_name.is_empty() {
            return Err(Status::invalid_argument("display_name is required"));
        }

        let primary_email = req
            .emails
            .iter()
            .find(|e| e.is_primary)
            .or_else(|| req.emails.first())
            .map(|e| e.email.clone())
            .unwrap_or_default();

        let mut contact = nuncio_contacts::Contact::new(req.display_name, primary_email);
        contact.account_id = Some(req.account_id);
        contact.organization = req.organization;
        if !req.emails.is_empty() {
            contact.emails = req
                .emails
                .into_iter()
                .map(|e| nuncio_contacts::ContactEmail {
                    email: e.email,
                    label: e.label,
                    is_primary: e.is_primary,
                })
                .collect();
        }
        contact.phones = req
            .phones
            .into_iter()
            .map(|p| nuncio_contacts::ContactPhone {
                phone: p.phone,
                label: p.label,
                is_primary: p.is_primary,
            })
            .collect();

        self.db
            .save_contact(&contact)
            .await
            .map_err(|e| Status::internal(format!("failed to save contact: {e}")))?;

        Ok(Response::new(CreateContactResponse {
            contact: Some(map_contact_to_proto(contact)),
        }))
    }
}

/// Test-only injection point for the daemon's inbound sync and outbound
/// send engines, threaded through the daemon's runtime wiring for the
/// full-daemon offline spine E2E test.
///
/// Production (`nunciod::main`, and every existing caller of [`serve`] /
/// [`serve_on_listener`]) never constructs a non-default instance:
/// [`Default`] yields every field `None`, which routes `Mail/Sync` and
/// `Mail/SendMessage` through the exact same real production entry points
/// (`nunciod::sync::run_all_accounts_sync` / `run_account_sync`,
/// `nunciod::send::send_message_for_account`) as before this override
/// existed. Production code depends only on the `nuncio_mail::{MailBackend,
/// MessageSender}` trait objects here -- never on `nuncio_mail`'s mock
/// types -- so this seam never makes production depend on a test double.
///
/// Only a full-daemon E2E test supplies `Some(..)`, standing in a
/// [`nuncio_mail::MockMailBackend`] / [`nuncio_mail::MockMessageSender`]
/// for real network I/O, and drives the override through the exact same
/// authenticated gRPC API a real client uses.
#[derive(Clone, Default)]
pub struct MailEngineOverrides {
    /// When `Some`, `Mail/Sync` fetches from this backend instead of
    /// building a real per-account engine from keyring credentials.
    pub mail_backend: Option<Arc<dyn MailBackend>>,
    /// When `Some`, `Mail/SendMessage` sends through this sender instead of
    /// building a real `SmtpTransportEngine` from keyring credentials.
    pub message_sender: Option<Arc<dyn MessageSender>>,
}

impl std::fmt::Debug for MailEngineOverrides {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MailEngineOverrides")
            .field("mail_backend", &self.mail_backend.is_some())
            .field("message_sender", &self.message_sender.is_some())
            .finish()
    }
}

/// `nuncio.v1.Mail` gRPC service implementation backed by the daemon's live
/// [`DatabaseEngine`] read/mark methods and [`SearchEngine`] FTS index, and
/// the live [`SecretManager`] vault used to build a real outbound SMTP
/// transport for `SendMessage`.
///
/// This exposes the mail READ path (list folders, list messages, read a
/// message, mark read/unread, search) over the real, persistent store that
/// the sync path writes into, AND the outbound SEND path (`SendMessage`) --
/// as opposed to the CLI's previous local ephemeral `HeadlessRunner`
/// database, which was thrown away when the CLI process exited, and its
/// previous fabricated "Message sent" output, which never actually dialed an
/// SMTP server. `Sync` triggers a real inbound sync over the same
/// authenticated API.
struct MailGrpcService {
    db: Arc<DatabaseEngine>,
    event_bus: Arc<EventBus>,
    secrets: Arc<SecretManager>,
    overrides: MailEngineOverrides,
}

#[tonic::async_trait]
impl Mail for MailGrpcService {
    async fn list_folders(
        &self,
        _request: Request<ListFoldersRequest>,
    ) -> Result<Response<ListFoldersResponse>, Status> {
        let folders = self
            .db
            .list_folders()
            .await
            .map_err(|e| Status::internal(format!("failed to list folders: {e}")))?
            .into_iter()
            .map(map_folder_to_proto)
            .collect();

        Ok(Response::new(ListFoldersResponse { folders }))
    }

    async fn list_messages(
        &self,
        request: Request<ListMessagesRequest>,
    ) -> Result<Response<ListMessagesResponse>, Status> {
        let req = request.into_inner();
        if req.folder_id.is_empty() {
            return Err(Status::invalid_argument("folder_id is required"));
        }
        let limit = if req.limit == 0 {
            DEFAULT_LIST_MESSAGES_LIMIT
        } else {
            req.limit as usize
        };

        let messages = self
            .db
            .list_messages(&req.folder_id, limit)
            .await
            .map_err(|e| Status::internal(format!("failed to list messages: {e}")))?
            .into_iter()
            .map(map_email_to_proto)
            .collect();

        Ok(Response::new(ListMessagesResponse { messages }))
    }

    async fn get_message(
        &self,
        request: Request<GetMessageRequest>,
    ) -> Result<Response<GetMessageResponse>, Status> {
        let req = request.into_inner();
        if req.message_id.is_empty() {
            return Err(Status::invalid_argument("message_id is required"));
        }

        let email = self.db.get_message(&req.message_id).await.map_err(|e| {
            Status::not_found(format!("message '{}' not found: {e}", req.message_id))
        })?;

        Ok(Response::new(GetMessageResponse {
            message: Some(map_email_to_proto(email)),
        }))
    }

    /// Persists the flag change via `DatabaseEngine::set_message_read` FIRST
    /// (so a stream subscriber never observes `MessageFlagsChanged` for a
    /// change that failed to persist), then drives it through
    /// `EventBus::process_command(CoreCommand::MarkRead { .. })` -- the same
    /// path the JSON-RPC IPC transport's `MarkRead` command uses -- so the
    /// in-memory unread-count state updates AND the existing
    /// `CoreEvent::MessageFlagsChanged` event is published, which is what
    /// makes this flow through `System/Subscribe` for free.
    async fn mark_read(
        &self,
        request: Request<MarkReadRequest>,
    ) -> Result<Response<MarkReadResponse>, Status> {
        let req = request.into_inner();
        if req.message_id.is_empty() {
            return Err(Status::invalid_argument("message_id is required"));
        }

        self.db
            .set_message_read(&req.message_id, req.read)
            .await
            .map_err(|e| {
                Status::not_found(format!("message '{}' not found: {e}", req.message_id))
            })?;

        self.event_bus.process_command(CoreCommand::MarkRead {
            message_id: req.message_id,
            read: req.read,
        });

        Ok(Response::new(MarkReadResponse {}))
    }

    async fn search_messages(
        &self,
        request: Request<SearchMessagesRequest>,
    ) -> Result<Response<SearchMessagesResponse>, Status> {
        let req = request.into_inner();
        let search = SearchEngine::new(&self.db);
        let hits = search
            .search_messages(&req.query)
            .await
            .map_err(|e| Status::internal(format!("search failed: {e}")))?
            .into_iter()
            .map(|hit| MessageSearchHit {
                id: hit.id,
                title: hit.title,
                snippet: hit.snippet,
            })
            .collect();

        Ok(Response::new(SearchMessagesResponse { hits }))
    }

    /// SendMessage composes and sends a real outbound email over SMTP via
    /// [`crate::send::send_message_for_account`], which resolves the sending
    /// account, its keyring password, and builds a real
    /// [`nuncio_mail::SmtpTransportEngine`] from the account's SMTP
    /// endpoint. Returns `Ok` ONLY when the transport genuinely accepted the
    /// message -- a resolution or transport failure surfaces as
    /// `Status::internal`/`Status::invalid_argument`, never a fabricated
    /// `SendMessageResponse`.
    ///
    /// When [`MailEngineOverrides::message_sender`] is injected, sends
    /// through it instead via
    /// [`crate::send::send_message_with_injected_sender`] -- no keyring
    /// lookup, no real SMTP transport -- so a full-daemon E2E test can
    /// assert on the exact outbound message an injected
    /// [`nuncio_mail::MockMessageSender`] captured.
    async fn send_message(
        &self,
        request: Request<SendMessageRequest>,
    ) -> Result<Response<SendMessageResponse>, Status> {
        let req = request.into_inner();
        if req.to.trim().is_empty() {
            return Err(Status::invalid_argument("to is required"));
        }
        if req.subject.trim().is_empty() {
            return Err(Status::invalid_argument("subject is required"));
        }

        let compose = crate::send::ComposeRequest {
            to: req.to,
            cc: req.cc,
            subject: req.subject,
            body_text: Some(req.body_text),
            body_html: req.body_html,
            attachments: req
                .attachments
                .into_iter()
                .map(map_attachment_from_proto)
                .collect(),
        };

        let message_id = if let Some(sender) = &self.overrides.message_sender {
            crate::send::send_message_with_injected_sender(&self.db, sender.as_ref(), compose)
                .await
                .map_err(|e| Status::internal(format!("failed to send message: {e}")))?
        } else {
            crate::send::send_message_for_account(&self.db, &self.secrets, compose)
                .await
                .map_err(|e| Status::internal(format!("failed to send message: {e}")))?
        };

        Ok(Response::new(SendMessageResponse { message_id }))
    }

    /// Sync triggers a real inbound mail synchronization and awaits full
    /// completion before returning, so a successful response guarantees the
    /// synced messages are already visible to `ListMessages`/`GetMessage`.
    /// `account_id` mirrors `CoreCommand::SyncAccount`/`SyncAll`: `Some`
    /// scopes the sync to one account, `None` syncs every configured
    /// account.
    ///
    /// When [`MailEngineOverrides::mail_backend`] is injected, fetches from
    /// it via [`crate::sync::sync_with_backend`] instead of resolving a
    /// real per-account engine from keyring credentials -- this is what
    /// lets a full-daemon E2E test drive a real inbound sync entirely over
    /// this authenticated gRPC API with no live network, exactly as a real
    /// client would trigger it (no un-intercepted back door).
    async fn sync(&self, request: Request<SyncRequest>) -> Result<Response<SyncResponse>, Status> {
        let account_id = request.into_inner().account_id;

        let synced = if let Some(backend) = &self.overrides.mail_backend {
            crate::sync::sync_with_backend(&self.db, &self.event_bus, backend.as_ref(), account_id)
                .await
                .map_err(|e| Status::internal(format!("sync failed: {e}")))?
        } else if let Some(account_id) = account_id {
            crate::sync::run_account_sync(&self.db, &self.secrets, &self.event_bus, &account_id)
                .await
                .map_err(|e| Status::internal(format!("sync failed: {e}")))?
        } else {
            crate::sync::run_all_accounts_sync(&self.db, &self.secrets, &self.event_bus).await
        };

        Ok(Response::new(SyncResponse {
            synced_count: synced as u64,
        }))
    }
}

/// Maps a `nuncio_filter::FilterRule` onto its wire-format
/// `nuncio.v1.FilterRule` representation. The parsed condition AST is
/// deliberately NOT carried over the wire -- see the
/// message's doc comment in `proto/nuncio/v1/nuncio.proto` -- `actions` is
/// rendered as NSQL strings via `RuleAction::to_nsql`.
fn map_filter_rule_to_proto(rule: nuncio_filter::FilterRule) -> FilterRuleProto {
    FilterRuleProto {
        id: rule.id,
        name: rule.name,
        target_account: rule.target_account,
        priority: rule.priority,
        enabled: rule.enabled,
        nsql_text: rule.nsql_text,
        actions: rule.actions.iter().map(|a| a.to_nsql()).collect(),
        created_at: rule.created_at,
        updated_at: rule.updated_at,
    }
}

/// Maps a `nuncio_filter::FilterPreviewResult` onto its wire-format
/// `nuncio.v1.PreviewRuleResponse` representation.
fn map_preview_result_to_proto(preview: nuncio_filter::FilterPreviewResult) -> PreviewRuleResponse {
    PreviewRuleResponse {
        message_id: preview.message_id,
        matched: preview.matched,
        matched_rule_id: preview.matched_rule_id,
        matched_rule_name: preview.matched_rule_name,
        actions_evaluated: preview
            .actions_evaluated
            .iter()
            .map(|a| a.to_nsql())
            .collect(),
        execution_time_us: preview.execution_time_us,
        condition_traces: preview.condition_traces,
    }
}

/// Builds a fixed synthetic sample message for `PreviewRule` to evaluate
/// against when the caller supplies no `message_id` (or one that does not
/// resolve to a stored message), mirroring the exact fallback sample the
/// CLI's previous local `filter test` path used -- so this RPC can always
/// return a preview result rather than requiring pre-existing stored mail.
fn synthetic_preview_email(id: &str) -> nuncio_core::model::Email {
    let received_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    nuncio_core::model::Email {
        id: id.to_string(),
        account_id: "acct-1".to_string(),
        folder_id: "inbox".to_string(),
        subject: "Test Subject".to_string(),
        sender: "test@nuncio.mx".to_string(),
        recipient: "me@nuncio.mx".to_string(),
        received_at,
        read: false,
        body_plain: Some("Sample body text".to_string()),
        body_html: None,
        attachments: Vec::new(),
    }
}

/// `nuncio.v1.Filters` gRPC service implementation backed by the daemon's
/// live [`DatabaseEngine`] filter-rule CRUD and the daemon's live
/// [`FilterEngine`] -- the authenticated gRPC replacement for the existing
/// hand-rolled `filter.*` JSON-RPC IPC methods (`nunciod::main`'s
/// `CustomRpcHandler`), which remain in place (both transports coexist)
/// until retired.
///
/// `CreateRule`/`DeleteRule` persist through `db` FIRST, then reload
/// `filter_engine`'s `ArcSwap`-backed rule set from the freshly persisted
/// state via `db.list_filter_rules()`, so the live engine and the persisted
/// store never disagree about which rules exist. A reload failure (e.g. a
/// rule whose regex no longer compiles) is logged but does NOT fail the
/// RPC: the mutation already succeeded in the store, so failing the RPC
/// here would misreport a successful write as an error.
struct FiltersGrpcService {
    db: Arc<DatabaseEngine>,
    filter_engine: Arc<FilterEngine>,
}

impl FiltersGrpcService {
    /// Reloads `self.filter_engine`'s active rule set from `self.db`'s
    /// current persisted rules, logging (rather than propagating) a reload
    /// failure -- shared by `create_rule` and `delete_rule` after each
    /// persists its own mutation.
    async fn reload_engine_from_store(&self) {
        match self.db.list_filter_rules().await {
            Ok(rules) => {
                if let Err(e) = self.filter_engine.reload_rules(rules) {
                    tracing::warn!("Filters: failed to reload live FilterEngine rules: {}", e);
                }
            }
            Err(e) => {
                tracing::warn!(
                    "Filters: failed to re-list persisted filter rules for engine reload: {}",
                    e
                );
            }
        }
    }
}

#[tonic::async_trait]
impl Filters for FiltersGrpcService {
    async fn create_rule(
        &self,
        request: Request<CreateRuleRequest>,
    ) -> Result<Response<CreateRuleResponse>, Status> {
        let req = request.into_inner();

        let rule = NsqlParser::parse_rule(req.name, req.priority, &req.nsql)
            .map_err(|e| Status::invalid_argument(format!("NSQL syntax error: {e}")))?;

        NsqlValidator::validate(&rule, &ValidationOptions::default())
            .map_err(|e| Status::invalid_argument(format!("NSQL validation error: {e}")))?;

        self.db
            .save_filter_rule(&rule)
            .await
            .map_err(|e| Status::internal(format!("failed to persist filter rule: {e}")))?;

        self.reload_engine_from_store().await;

        Ok(Response::new(CreateRuleResponse {
            rule: Some(map_filter_rule_to_proto(rule)),
        }))
    }

    async fn list_rules(
        &self,
        _request: Request<ListRulesRequest>,
    ) -> Result<Response<ListRulesResponse>, Status> {
        let rules = self
            .db
            .list_filter_rules()
            .await
            .map_err(|e| Status::internal(format!("failed to list filter rules: {e}")))?
            .into_iter()
            .map(map_filter_rule_to_proto)
            .collect();

        Ok(Response::new(ListRulesResponse { rules }))
    }

    async fn delete_rule(
        &self,
        request: Request<DeleteRuleRequest>,
    ) -> Result<Response<DeleteRuleResponse>, Status> {
        let req = request.into_inner();
        if req.id.is_empty() {
            return Err(Status::invalid_argument("id is required"));
        }

        self.db
            .delete_filter_rule(&req.id)
            .await
            .map_err(|e| Status::internal(format!("failed to delete filter rule: {e}")))?;

        self.reload_engine_from_store().await;

        Ok(Response::new(DeleteRuleResponse {}))
    }

    /// Unlike `create_rule`, an invalid rule is never an RPC failure here --
    /// see this RPC's doc comment in `proto/nuncio/v1/nuncio.proto`.
    async fn validate_rule(
        &self,
        request: Request<ValidateRuleRequest>,
    ) -> Result<Response<ValidateRuleResponse>, Status> {
        let req = request.into_inner();

        let rule = match NsqlParser::parse_rule("Validation Preview", 0, &req.nsql) {
            Ok(rule) => rule,
            Err(e) => {
                return Ok(Response::new(ValidateRuleResponse {
                    valid: false,
                    error: format!("NSQL syntax error: {e}"),
                }));
            }
        };

        match NsqlValidator::validate(&rule, &ValidationOptions::default()) {
            Ok(()) => Ok(Response::new(ValidateRuleResponse {
                valid: true,
                error: String::new(),
            })),
            Err(e) => Ok(Response::new(ValidateRuleResponse {
                valid: false,
                error: format!("NSQL validation error: {e}"),
            })),
        }
    }

    /// Dry-run evaluates an ad hoc, NOT-yet-persisted NSQL rule against a
    /// stored message (or a synthetic sample -- see
    /// [`synthetic_preview_email`]), via a fresh one-off [`FilterEngine`]
    /// built from just that single rule and its existing `preview` path.
    /// This deliberately does NOT use `self.filter_engine` (the daemon's
    /// live, persisted rule set): the whole point of a preview is to try out
    /// a candidate rule that may not be, and may never be, saved.
    async fn preview_rule(
        &self,
        request: Request<PreviewRuleRequest>,
    ) -> Result<Response<PreviewRuleResponse>, Status> {
        let req = request.into_inner();

        let rule = NsqlParser::parse_rule("Preview Rule", 0, &req.nsql)
            .map_err(|e| Status::invalid_argument(format!("NSQL syntax error: {e}")))?;

        let preview_engine = FilterEngine::new(vec![rule])
            .map_err(|e| Status::internal(format!("failed to build preview engine: {e}")))?;

        let email = match &req.message_id {
            Some(message_id) if !message_id.is_empty() => {
                match self.db.get_message(message_id).await {
                    Ok(email) => email,
                    Err(_) => synthetic_preview_email(message_id),
                }
            }
            _ => synthetic_preview_email("msg-test"),
        };

        let preview = preview_engine.preview(&email);
        Ok(Response::new(map_preview_result_to_proto(preview)))
    }
}

/// Maps a wire-format `nuncio.v1.ExportFormat` enum value back onto
/// `nuncio_core::ExportFormat`. `Unspecified` is rejected rather than
/// silently defaulting to a format the caller never asked for.
fn map_export_format_from_proto(
    format: ExportFormatProto,
) -> Result<nuncio_core::ExportFormat, Status> {
    match format {
        ExportFormatProto::Mbox => Ok(nuncio_core::ExportFormat::Mbox),
        ExportFormatProto::EmlZip => Ok(nuncio_core::ExportFormat::EmlZip),
        ExportFormatProto::Json => Ok(nuncio_core::ExportFormat::Json),
        ExportFormatProto::Jsonl => Ok(nuncio_core::ExportFormat::JsonLines),
        ExportFormatProto::Unspecified => {
            Err(Status::invalid_argument("export format is required"))
        }
    }
}

/// `nuncio.v1.Export` gRPC service implementation backed by the daemon's
/// live [`DatabaseEngine`]: loads messages via
/// [`DatabaseEngine::list_messages_for_export`] (scoped by `account_id`,
/// `folder_id`, or unscoped for "every message"), then writes them to
/// `output_path` on the local host via
/// [`DatabaseEngine::export_messages_to_file`] -- the SAME real
/// `nuncio_core::export::ExportEngine` `nuncio-cli`'s previous local-only
/// export path used, now reachable over the authenticated gRPC API against
/// the daemon's real, persistent store. `export_messages_to_file` also
/// appends a WORM audit record for the export as a side effect, so every
/// export is itself auditable via `Audit`.
struct ExportGrpcService {
    db: Arc<DatabaseEngine>,
}

#[tonic::async_trait]
impl Export for ExportGrpcService {
    async fn export_mailbox(
        &self,
        request: Request<ExportRequest>,
    ) -> Result<Response<ExportResponse>, Status> {
        let req = request.into_inner();
        if req.output_path.trim().is_empty() {
            return Err(Status::invalid_argument("output_path is required"));
        }
        let format = map_export_format_from_proto(req.format())?;

        let messages = match req.scope {
            Some(export_request::Scope::AccountId(account_id)) => {
                self.db
                    .list_messages_for_export(Some(&account_id), None)
                    .await
            }
            Some(export_request::Scope::FolderId(folder_id)) => {
                self.db
                    .list_messages_for_export(None, Some(&folder_id))
                    .await
            }
            None => self.db.list_messages_for_export(None, None).await,
        }
        .map_err(|e| Status::internal(format!("failed to load messages for export: {e}")))?;

        let output_path = std::path::PathBuf::from(&req.output_path);
        let summary = self
            .db
            .export_messages_to_file(&messages, format, &output_path)
            .await
            .map_err(|e| Status::internal(format!("export failed: {e}")))?;

        Ok(Response::new(ExportResponse {
            output_path: summary.output_path,
            message_count: summary.message_count as u64,
            bytes_written: summary.bytes_written,
        }))
    }
}

/// Default cap on records returned by `Audit/ListRecords` when the caller
/// supplies `limit: 0` ("use the server default").
const DEFAULT_LIST_RECORDS_LIMIT: u32 = 100;

/// Maps a `nuncio_core::WormAuditRecord` onto its wire-format
/// `nuncio.v1.AuditRecord` representation. `record_hmac` is a verification
/// MAC output, never the WORM HMAC signing key itself -- see the message's
/// doc comment in `proto/nuncio/v1/nuncio.proto`.
fn map_audit_record_to_proto(record: nuncio_core::WormAuditRecord) -> AuditRecordProto {
    AuditRecordProto {
        sequence: record.sequence,
        timestamp_ns: record.timestamp_ns,
        actor: record.actor,
        action: record.action,
        data_hash: record.data_hash,
        previous_block_hash: record.previous_block_hash,
        record_hmac: record.record_hmac,
    }
}

/// `nuncio.v1.Audit` gRPC service implementation backed by the daemon's
/// live [`DatabaseEngine`] WORM audit ledger.
/// A read/verify-only surface -- there is no RPC here to create or mutate
/// records, since the ledger is written internally by the daemon as a side
/// effect of other operations (see `ExportGrpcService`).
struct AuditGrpcService {
    db: Arc<DatabaseEngine>,
}

#[tonic::async_trait]
impl Audit for AuditGrpcService {
    async fn list_records(
        &self,
        request: Request<ListRecordsRequest>,
    ) -> Result<Response<ListRecordsResponse>, Status> {
        let req = request.into_inner();
        let limit = if req.limit == 0 {
            DEFAULT_LIST_RECORDS_LIMIT
        } else {
            req.limit
        };

        let records = self
            .db
            .list_worm_audit_records(limit, req.offset)
            .await
            .map_err(|e| Status::internal(format!("failed to list audit records: {e}")))?
            .into_iter()
            .map(map_audit_record_to_proto)
            .collect();

        Ok(Response::new(ListRecordsResponse { records }))
    }

    /// Re-verifies the ENTIRE persisted WORM audit ledger via
    /// [`DatabaseEngine::verify_worm_audit_chain_report`], using the ledger's
    /// real WORM HMAC key. Never fabricates `valid: true`; a genuine
    /// signing/crypto failure (as opposed to a specific broken record) is
    /// surfaced as `Status::internal` rather than a false `valid: false`.
    async fn verify_chain(
        &self,
        _request: Request<VerifyChainRequest>,
    ) -> Result<Response<VerifyChainResponse>, Status> {
        let report = self
            .db
            .verify_worm_audit_chain_report()
            .await
            .map_err(|e| Status::internal(format!("failed to verify audit chain: {e}")))?;

        Ok(Response::new(VerifyChainResponse {
            valid: report.valid,
            record_count: report.record_count as u64,
            first_broken_seq: report.first_broken_seq,
        }))
    }
}

/// Bearer-token authentication interceptor for the loopback `nuncio.v1` gRPC
/// server.
///
/// Requires an `authorization: Bearer <token>` metadata header on every call
/// and rejects missing, malformed, or mismatched tokens with
/// `Status::unauthenticated`. The comparison against the expected token uses
/// [`subtle::ConstantTimeEq`] so a mismatching guess cannot be distinguished
/// by timing. The token itself is never logged.
#[derive(Clone)]
struct BearerAuthInterceptor {
    expected_token: Arc<str>,
}

impl BearerAuthInterceptor {
    fn new(expected_token: impl Into<Arc<str>>) -> Self {
        Self {
            expected_token: expected_token.into(),
        }
    }
}

impl tonic::service::Interceptor for BearerAuthInterceptor {
    fn call(&mut self, request: Request<()>) -> Result<Request<()>, Status> {
        let header = request
            .metadata()
            .get("authorization")
            .ok_or_else(|| Status::unauthenticated("missing authorization metadata"))?;
        let header_str = header
            .to_str()
            .map_err(|_| Status::unauthenticated("authorization metadata is not valid ASCII"))?;
        let provided = header_str.strip_prefix("Bearer ").ok_or_else(|| {
            Status::unauthenticated("expected 'Bearer <token>' authorization scheme")
        })?;

        let matches: bool = provided
            .as_bytes()
            .ct_eq(self.expected_token.as_bytes())
            .into();
        if matches {
            Ok(request)
        } else {
            Err(Status::unauthenticated("invalid bearer token"))
        }
    }
}

/// Binds a loopback TCP listener at `addr` and serves the `nuncio.v1.System`,
/// `nuncio.v1.Accounts`, `nuncio.v1.Mail`, `nuncio.v1.Filters`,
/// `nuncio.v1.Export`, `nuncio.v1.Audit`, and `nuncio.v1.Calendar` gRPC
/// services on it, all authenticated by `token`, until the transport server
/// errors.
///
/// `addr` MUST be a loopback address (e.g. `127.0.0.1:PORT`); callers are
/// responsible for passing loopback-only addresses (see
/// [`grpc_addr_from_env`]).
pub async fn serve(
    addr: &str,
    event_bus: Arc<EventBus>,
    db: Arc<DatabaseEngine>,
    filter_engine: Arc<FilterEngine>,
    secrets: Arc<SecretManager>,
    token: impl Into<Arc<str>>,
) -> Result<(), GrpcServeError> {
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|source| GrpcServeError::Bind {
            addr: addr.to_string(),
            source,
        })?;
    serve_on_listener(listener, event_bus, db, filter_engine, secrets, token).await
}

/// Serves the `nuncio.v1.System`, `nuncio.v1.Accounts`, `nuncio.v1.Mail`,
/// `nuncio.v1.Filters`, `nuncio.v1.Export`, `nuncio.v1.Audit`,
/// `nuncio.v1.Calendar`, and `nuncio.v1.Contacts` gRPC services on an
/// already-bound [`TcpListener`].
///
/// # Security
///
/// EVERY service mounted on this server MUST be wrapped in its own
/// [`BearerAuthInterceptor`] via `*Server::with_interceptor`, exactly like
/// `System` and `Accounts` below -- never `add_service(SomeServer::new(...))`
/// unwrapped. This is a hard, non-negotiable invariant: an un-intercepted
/// service mounted here would be reachable by any local process without
/// authentication. If a future service is added, mount it the same way.
///
/// Exposed separately from [`serve`] so tests can bind an ephemeral loopback
/// port (`127.0.0.1:0`), read back the OS-assigned port via
/// `TcpListener::local_addr`, and connect a client to it deterministically:
/// once `TcpListener::bind` returns, the OS is already accepting connections
/// on that port, so no fixed sleep is needed before dialing it.
pub async fn serve_on_listener(
    listener: TcpListener,
    event_bus: Arc<EventBus>,
    db: Arc<DatabaseEngine>,
    filter_engine: Arc<FilterEngine>,
    secrets: Arc<SecretManager>,
    token: impl Into<Arc<str>>,
) -> Result<(), GrpcServeError> {
    serve_on_listener_with_overrides(
        listener,
        event_bus,
        db,
        filter_engine,
        secrets,
        token,
        MailEngineOverrides::default(),
        CalendarEngineOverrides::default(),
        ContactsEngineOverrides::default(),
    )
    .await
}

/// Identical to [`serve_on_listener`], except the `nuncio.v1.Mail` service's
/// inbound sync and outbound send engines, the `nuncio.v1.Calendar`
/// service's sync backend, and the `nuncio.v1.Contacts` service's sync
/// backend can be overridden.
/// [`serve_on_listener`] is simply this function called with
/// `MailEngineOverrides::default()` / `CalendarEngineOverrides::default()` /
/// `ContactsEngineOverrides::default()`
/// (i.e. every field `None`, which is production's exact prior behavior);
/// this function exists so a full-daemon offline E2E test can supply
/// `Some(..)` without changing [`serve_on_listener`]'s signature for its many
/// existing callers.
#[allow(clippy::too_many_arguments)]
pub async fn serve_on_listener_with_overrides(
    listener: TcpListener,
    event_bus: Arc<EventBus>,
    db: Arc<DatabaseEngine>,
    filter_engine: Arc<FilterEngine>,
    secrets: Arc<SecretManager>,
    token: impl Into<Arc<str>>,
    overrides: MailEngineOverrides,
    calendar_overrides: CalendarEngineOverrides,
    contacts_overrides: ContactsEngineOverrides,
) -> Result<(), GrpcServeError> {
    let token: Arc<str> = token.into();

    let system_service = SystemGrpcService {
        event_bus: event_bus.clone(),
    };
    let system_interceptor = BearerAuthInterceptor::new(token.clone());
    let system_svc = SystemServer::with_interceptor(system_service, system_interceptor);

    let accounts_service = AccountsGrpcService {
        db: db.clone(),
        secrets: secrets.clone(),
    };
    let accounts_interceptor = BearerAuthInterceptor::new(token.clone());
    let accounts_svc = AccountsServer::with_interceptor(accounts_service, accounts_interceptor);

    // Mail: mounted behind its own `BearerAuthInterceptor`, exactly like
    // `System` and `Accounts` above -- see the hard invariant documented on
    // this function's doc comment.
    let mail_service = MailGrpcService {
        db: db.clone(),
        event_bus,
        secrets: secrets.clone(),
        overrides,
    };
    let mail_interceptor = BearerAuthInterceptor::new(token.clone());
    let mail_svc = MailServer::with_interceptor(mail_service, mail_interceptor);

    // Filters: mounted behind its own `BearerAuthInterceptor`, exactly like
    // `System`/`Accounts`/`Mail` above -- see the hard invariant documented
    // on this function's doc comment.
    let filters_service = FiltersGrpcService {
        db: db.clone(),
        filter_engine,
    };
    let filters_interceptor = BearerAuthInterceptor::new(token.clone());
    let filters_svc = FiltersServer::with_interceptor(filters_service, filters_interceptor);

    // Export: mounted behind its own `BearerAuthInterceptor`, exactly like
    // every other service above -- see the hard invariant documented on
    // this function's doc comment.
    let export_service = ExportGrpcService { db: db.clone() };
    let export_interceptor = BearerAuthInterceptor::new(token.clone());
    let export_svc = ExportServer::with_interceptor(export_service, export_interceptor);

    // Audit: mounted behind its own `BearerAuthInterceptor`, exactly like
    // every other service above -- see the hard invariant documented on
    // this function's doc comment.
    let audit_service = AuditGrpcService { db: db.clone() };
    let audit_interceptor = BearerAuthInterceptor::new(token.clone());
    let audit_svc = AuditServer::with_interceptor(audit_service, audit_interceptor);

    // Calendar: mounted behind its own `BearerAuthInterceptor`, exactly like
    // every other service above -- see the hard invariant documented on
    // this function's doc comment.
    let calendar_service = CalendarGrpcService {
        db: db.clone(),
        overrides: calendar_overrides,
    };
    let calendar_interceptor = BearerAuthInterceptor::new(token.clone());
    let calendar_svc = CalendarServer::with_interceptor(calendar_service, calendar_interceptor);

    // Contacts: mounted behind its own `BearerAuthInterceptor`, exactly like
    // every other service above -- see the hard invariant documented on
    // this function's doc comment.
    let contacts_service = ContactsGrpcService {
        db,
        overrides: contacts_overrides,
    };
    let contacts_interceptor = BearerAuthInterceptor::new(token);
    let contacts_svc = ContactsServer::with_interceptor(contacts_service, contacts_interceptor);

    Server::builder()
        .add_service(system_svc)
        .add_service(accounts_svc)
        .add_service(mail_svc)
        .add_service(filters_svc)
        .add_service(export_svc)
        .add_service(audit_svc)
        .add_service(calendar_svc)
        .add_service(contacts_svc)
        .serve_with_incoming(TcpListenerStream::new(listener))
        .await
        .map_err(GrpcServeError::Transport)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nuncio_core::{CoreCommand, EngineStatus};
    use nuncio_proto::v1::accounts_client::AccountsClient;
    use nuncio_proto::v1::system_client::SystemClient;
    use tonic::Code;

    /// Spawns a test server backed by a fresh ephemeral database, a fresh
    /// empty [`FilterEngine`], and a fresh [`SecretManager::mock`] vault --
    /// the right default for every test that only exercises
    /// `System`/`Accounts`/`Mail` or doesn't care about pre-existing
    /// account/keyring/filter-rule state. Tests that DO care
    /// (persistence-across-restart, credential-secrecy, the `Filters`
    /// service itself) build their own `db`/`secrets`/`filter_engine` and
    /// call [`spawn_test_server_with`] / [`spawn_test_server_with_overrides`]
    /// directly instead.
    async fn spawn_test_server(
        event_bus: Arc<EventBus>,
        token: &str,
    ) -> (
        std::net::SocketAddr,
        tokio::task::JoinHandle<()>,
        tempfile::TempDir,
    ) {
        // The `TempDir` guard must outlive this function: the spawned server
        // task keeps using `db` long after we return, and on Linux dropping
        // the guard here unlinks the directory backing the sqlite file out
        // from under it, so any connection the pool opens afterward fails
        // with "unable to open database file". Return it to the caller so it
        // stays alive for the test's duration.
        let (db, dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("connect ephemeral test db");
        let secrets = Arc::new(SecretManager::mock());
        let filter_engine = Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set"));
        let (addr, handle) =
            spawn_test_server_with(event_bus, Arc::new(db), filter_engine, secrets, token).await;
        (addr, handle, dir)
    }

    /// Spawns a test server on an ephemeral loopback port backed by the
    /// given `db`, `filter_engine`, and `secrets`, so tests can share (and
    /// re-open) the same database path / engine instance / mock keyring
    /// state across multiple server instances.
    async fn spawn_test_server_with(
        event_bus: Arc<EventBus>,
        db: Arc<DatabaseEngine>,
        filter_engine: Arc<FilterEngine>,
        secrets: Arc<SecretManager>,
        token: &str,
    ) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
        spawn_test_server_with_overrides(
            event_bus,
            db,
            filter_engine,
            secrets,
            token,
            MailEngineOverrides::default(),
        )
        .await
    }

    /// Spawns a test server on an ephemeral loopback port backed by the
    /// given `db`/`filter_engine`/`secrets`/`overrides`: the helper every
    /// test that injects a [`MailEngineOverrides::mail_backend`] /
    /// [`MailEngineOverrides::message_sender`] uses to prove `Sync` /
    /// `SendMessage` drive an injected test double over the real
    /// authenticated gRPC API.
    async fn spawn_test_server_with_overrides(
        event_bus: Arc<EventBus>,
        db: Arc<DatabaseEngine>,
        filter_engine: Arc<FilterEngine>,
        secrets: Arc<SecretManager>,
        token: &str,
        overrides: MailEngineOverrides,
    ) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
        spawn_test_server_with_all_overrides(
            event_bus,
            db,
            filter_engine,
            secrets,
            token,
            overrides,
            CalendarEngineOverrides::default(),
            ContactsEngineOverrides::default(),
        )
        .await
    }

    /// Spawns a test server on an ephemeral loopback port backed by the
    /// given `db`/`filter_engine`/`secrets`/`overrides`/`calendar_overrides`/
    /// `contacts_overrides`: the helper every test that injects a
    /// [`CalendarEngineOverrides::calendar_backend`] /
    /// [`ContactsEngineOverrides::contacts_backend`] uses to prove `Sync`
    /// drives an injected test double over the real authenticated gRPC API.
    #[allow(clippy::too_many_arguments)]
    async fn spawn_test_server_with_all_overrides(
        event_bus: Arc<EventBus>,
        db: Arc<DatabaseEngine>,
        filter_engine: Arc<FilterEngine>,
        secrets: Arc<SecretManager>,
        token: &str,
        overrides: MailEngineOverrides,
        calendar_overrides: CalendarEngineOverrides,
        contacts_overrides: ContactsEngineOverrides,
    ) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral loopback port");
        let addr = listener.local_addr().expect("listener has local addr");
        let token = token.to_string();
        let handle = tokio::spawn(async move {
            let _ = serve_on_listener_with_overrides(
                listener,
                event_bus,
                db,
                filter_engine,
                secrets,
                token,
                overrides,
                calendar_overrides,
                contacts_overrides,
            )
            .await;
        });
        (addr, handle)
    }

    #[test]
    fn grpc_addr_from_env_defaults_when_unset() {
        // Serialize access to the shared process environment variable so
        // this test cannot interleave with any other test touching it.
        static ENV_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());

        std::env::remove_var(GRPC_ADDR_ENV_VAR);
        assert_eq!(grpc_addr_from_env(), DEFAULT_GRPC_ADDR);

        std::env::set_var(GRPC_ADDR_ENV_VAR, "127.0.0.1:12345");
        assert_eq!(grpc_addr_from_env(), "127.0.0.1:12345");
        std::env::remove_var(GRPC_ADDR_ENV_VAR);
    }

    #[test]
    fn grpc_serve_error_messages_do_not_leak_and_are_displayable() {
        let bind_err = GrpcServeError::Bind {
            addr: "127.0.0.1:9420".to_string(),
            source: std::io::Error::new(std::io::ErrorKind::AddrInUse, "in use"),
        };
        assert!(bind_err.to_string().contains("127.0.0.1:9420"));
    }

    #[tokio::test]
    async fn get_status_rejects_missing_bearer_token() {
        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle, _dir) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = SystemClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let err = client
            .get_status(GetStatusRequest {})
            .await
            .expect_err("missing token must be rejected");
        assert_eq!(err.code(), Code::Unauthenticated);
    }

    #[tokio::test]
    async fn get_status_rejects_wrong_bearer_token() {
        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle, _dir) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = SystemClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let mut request = Request::new(GetStatusRequest {});
        request.metadata_mut().insert(
            "authorization",
            "Bearer wrong-token"
                .parse()
                .expect("valid ascii metadata value"),
        );
        let err = client
            .get_status(request)
            .await
            .expect_err("wrong token must be rejected");
        assert_eq!(err.code(), Code::Unauthenticated);
    }

    #[tokio::test]
    async fn get_status_rejects_malformed_authorization_header() {
        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle, _dir) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = SystemClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let mut request = Request::new(GetStatusRequest {});
        request.metadata_mut().insert(
            "authorization",
            "not-a-bearer-scheme"
                .parse()
                .expect("valid ascii metadata value"),
        );
        let err = client
            .get_status(request)
            .await
            .expect_err("malformed scheme must be rejected");
        assert_eq!(err.code(), Code::Unauthenticated);
    }

    #[tokio::test]
    async fn get_status_accepts_correct_bearer_token_and_reports_live_state() {
        let event_bus = Arc::new(EventBus::new());
        event_bus.process_command(CoreCommand::SyncAll);
        assert_eq!(event_bus.current_state().status, EngineStatus::Syncing);

        let (addr, _handle, _dir) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = SystemClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let mut request = Request::new(GetStatusRequest {});
        request.metadata_mut().insert(
            "authorization",
            "Bearer correct-token"
                .parse()
                .expect("valid ascii metadata value"),
        );
        let response = client
            .get_status(request)
            .await
            .expect("valid token is accepted")
            .into_inner();
        assert_eq!(response.engine_status, "Syncing");
        assert_eq!(response.version, env!("CARGO_PKG_VERSION"));
    }

    #[tokio::test]
    async fn serve_binds_and_starts_on_a_valid_loopback_address() {
        // Exercises the `serve()` wrapper's happy path (bind succeeds, then
        // delegates into `serve_on_listener`), as distinct from the
        // `spawn_test_server` helper above which pre-binds the listener
        // itself and calls `serve_on_listener` directly.
        let event_bus = Arc::new(EventBus::new());
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("connect ephemeral test db");
        let secrets = Arc::new(SecretManager::mock());
        let filter_engine = Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set"));
        let _handle = tokio::spawn(async move {
            let _ = serve(
                "127.0.0.1:0",
                event_bus,
                Arc::new(db),
                filter_engine,
                secrets,
                "token",
            )
            .await;
        });
        // Yield so the spawned task is polled at least once and actually
        // reaches the bind + delegate-to-`serve_on_listener` call before
        // this test function returns.
        tokio::task::yield_now().await;
    }

    #[tokio::test]
    async fn serve_fails_closed_when_bind_address_is_invalid() {
        let event_bus = Arc::new(EventBus::new());
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("connect ephemeral test db");
        let secrets = Arc::new(SecretManager::mock());
        let filter_engine = Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set"));
        let err = serve(
            "not-a-valid-addr",
            event_bus,
            Arc::new(db),
            filter_engine,
            secrets,
            "token",
        )
        .await
        .expect_err("invalid bind address must fail");
        assert!(matches!(err, GrpcServeError::Bind { .. }));
    }

    fn authed_bearer_request<T>(payload: T) -> Request<T> {
        let mut request = Request::new(payload);
        request.metadata_mut().insert(
            "authorization",
            "Bearer correct-token"
                .parse()
                .expect("valid ascii metadata value"),
        );
        request
    }

    #[tokio::test]
    async fn subscribe_streams_mapped_core_events_from_the_live_event_bus() {
        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle, _dir) = spawn_test_server(event_bus.clone(), "correct-token").await;

        let mut client = SystemClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");

        // `client.subscribe(..).await` only resolves once the server-side
        // `subscribe` method has returned `Response::new(stream)`, and that
        // method registers `event_bus.subscribe_events()` before returning.
        // So once this `.await` completes, the subscription is guaranteed
        // to already be live: publishing an event afterwards cannot race
        // ahead of the subscriber registering, and no fixed sleep is
        // needed to make this deterministic.
        let mut stream = client
            .subscribe(authed_bearer_request(SubscribeRequest {}))
            .await
            .expect("subscribe is accepted for a valid bearer token")
            .into_inner();

        event_bus.process_command(CoreCommand::SyncAccount {
            account_id: "acct-42".to_string(),
        });

        let event = stream
            .message()
            .await
            .expect("stream yields without a transport error")
            .expect("stream produces an event rather than ending");

        match event.kind {
            Some(Kind::SyncStarted(SyncStarted { account_id })) => {
                assert_eq!(account_id, Some("acct-42".to_string()));
            }
            other => panic!("expected a mapped SyncStarted event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn subscribe_rejects_missing_bearer_token() {
        // Confirms `Subscribe` inherits the same `BearerAuthInterceptor` as
        // `GetStatus` because both are defined on the single `System`
        // service (no separate, un-intercepted service).
        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle, _dir) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = SystemClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let err = client
            .subscribe(SubscribeRequest {})
            .await
            .expect_err("missing bearer token must be rejected");
        assert_eq!(err.code(), Code::Unauthenticated);
    }

    #[tokio::test]
    async fn concurrent_unary_and_streaming_calls_do_not_interleave_or_corrupt() {
        // Regression test for the property the old hand-rolled JSON-RPC IPC
        // transport lacked: a response/notification demux race that could
        // corrupt reads when a streamed notification and a unary response
        // were in flight at the same time. gRPC/HTTP2 multiplexes streaming
        // and unary calls on independent logical streams, so this asserts
        // both an active event subscription and many concurrent unary
        // `GetStatus` calls complete correctly and without corrupting one
        // another.
        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle, _dir) = spawn_test_server(event_bus.clone(), "correct-token").await;

        // Open (and await) the subscription first, on its own connection,
        // so the server-side registration has deterministically happened
        // before any event below is published (see the comment in the
        // single-event test above for why this ordering is race-free).
        let mut sub_client = SystemClient::connect(format!("http://{addr}"))
            .await
            .expect("subscribe client connects");
        let mut stream = sub_client
            .subscribe(authed_bearer_request(SubscribeRequest {}))
            .await
            .expect("subscribe is accepted for a valid bearer token")
            .into_inner();

        const ROUNDS: usize = 20;
        let mut unary_client = SystemClient::connect(format!("http://{addr}"))
            .await
            .expect("unary client connects");
        let unary_task = tokio::spawn(async move {
            let mut responses = Vec::with_capacity(ROUNDS);
            for _ in 0..ROUNDS {
                let response = unary_client
                    .get_status(authed_bearer_request(GetStatusRequest {}))
                    .await
                    .expect("unary call succeeds")
                    .into_inner();
                responses.push(response);
            }
            responses
        });

        for i in 0..ROUNDS {
            event_bus.process_command(CoreCommand::SyncAccount {
                account_id: format!("acct-{i}"),
            });
        }

        let unary_responses = unary_task.await.expect("unary task does not panic");
        assert_eq!(unary_responses.len(), ROUNDS);
        for response in &unary_responses {
            // Every unary response must be a well-formed, uncorrupted
            // `GetStatusResponse`: the correct build version, and a status
            // string matching a real `EngineStatus` variant -- never bytes
            // bled in from the concurrently-running event stream.
            assert_eq!(response.version, env!("CARGO_PKG_VERSION"));
            assert!(
                matches!(
                    response.engine_status.as_str(),
                    "Idle" | "Syncing" | "ShuttingDown"
                ),
                "unexpected/corrupted engine_status: {:?}",
                response.engine_status
            );
        }

        for i in 0..ROUNDS {
            let event = stream
                .message()
                .await
                .expect("stream yields without a transport error")
                .expect("stream produces an event rather than ending early");
            match event.kind {
                Some(Kind::SyncStarted(SyncStarted { account_id })) => {
                    assert_eq!(account_id, Some(format!("acct-{i}")));
                }
                other => panic!("expected mapped SyncStarted(acct-{i}) event, got {other:?}"),
            }
        }
    }

    fn sample_account_config_proto(id: &str, keyring_secret_key: &str) -> AccountConfigProto {
        AccountConfigProto {
            id: id.to_string(),
            name: "Restart Test Account".to_string(),
            email_address: format!("{id}@nuncio.mx"),
            protocol: AccountProtocolProto::ImapSmtp.into(),
            server_host: "imap.nuncio.mx".to_string(),
            server_port: 993,
            use_tls: true,
            imap_tls_mode: TlsModeProto::ImplicitTls.into(),
            smtp_tls_mode: TlsModeProto::ImplicitTls.into(),
            keyring_secret_key: keyring_secret_key.to_string(),
            sync_interval_secs: 60,
            smtp_host: "smtp.nuncio.mx".to_string(),
            smtp_port: 465,
        }
    }

    #[tokio::test]
    async fn add_account_rejects_missing_bearer_token() {
        // Confirms `Accounts` is mounted behind its own `BearerAuthInterceptor`
        // exactly like `System` (no un-intercepted service).
        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle, _dir) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = AccountsClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let err = client
            .add_account(AddAccountRequest {
                config: Some(sample_account_config_proto(
                    "acct-noauth",
                    "nuncio/acct-noauth",
                )),
                password: "irrelevant".to_string(),
            })
            .await
            .expect_err("missing bearer token must be rejected");
        assert_eq!(err.code(), Code::Unauthenticated);
    }

    #[tokio::test]
    async fn list_accounts_rejects_missing_bearer_token() {
        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle, _dir) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = AccountsClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let err = client
            .list_accounts(ListAccountsRequest {})
            .await
            .expect_err("missing bearer token must be rejected");
        assert_eq!(err.code(), Code::Unauthenticated);
    }

    #[tokio::test]
    async fn add_account_rejects_empty_password() {
        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle, _dir) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = AccountsClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let err = client
            .add_account(authed_bearer_request(AddAccountRequest {
                config: Some(sample_account_config_proto("acct-nopw", "nuncio/acct-nopw")),
                password: String::new(),
            }))
            .await
            .expect_err("empty password must be rejected");
        assert_eq!(err.code(), Code::InvalidArgument);
    }

    #[tokio::test]
    async fn add_account_rejects_missing_config() {
        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle, _dir) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = AccountsClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let err = client
            .add_account(authed_bearer_request(AddAccountRequest {
                config: None,
                password: "some-password".to_string(),
            }))
            .await
            .expect_err("missing config must be rejected");
        assert_eq!(err.code(), Code::InvalidArgument);
    }

    /// Proves an account added through the daemon persists across a daemon
    /// restart. Simulates the restart by
    /// dropping the first `DatabaseEngine`/server pair and opening a brand
    /// new one at the SAME on-disk database path (a real persistent temp
    /// file, not `connect_ephemeral`'s throwaway one), then calling
    /// `ListAccounts` against the second server instance.
    #[tokio::test]
    async fn add_account_persists_across_a_simulated_daemon_restart() {
        let dir = tempfile::tempdir().expect("create persistent temp dir");
        let db_path = dir.path().join("accounts_restart_test.db");
        let secrets = Arc::new(SecretManager::mock());

        // "First daemon run".
        let db_first_run = Arc::new(
            DatabaseEngine::connect_file(&db_path, &secrets)
                .await
                .expect("open db on first run"),
        );
        let (addr, _handle) = spawn_test_server_with(
            Arc::new(EventBus::new()),
            db_first_run.clone(),
            Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set")),
            secrets.clone(),
            "correct-token",
        )
        .await;

        let mut client = AccountsClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let add_response = client
            .add_account(authed_bearer_request(AddAccountRequest {
                config: Some(sample_account_config_proto(
                    "acct-restart-1",
                    "nuncio/acct-restart-1",
                )),
                password: "restart-secret-pw".to_string(),
            }))
            .await
            .expect("add_account succeeds")
            .into_inner();
        assert_eq!(add_response.id, "acct-restart-1");

        db_first_run.close().await;

        // "Second daemon run": a brand new `DatabaseEngine`, opened at the
        // exact same `db_path`, sharing the SAME mock `SecretManager` (so
        // the previously-provisioned keyring entry is still readable, just
        // like a real OS keyring survives a daemon restart).
        let db_second_run = Arc::new(
            DatabaseEngine::connect_file(&db_path, &secrets)
                .await
                .expect("re-open db on second run"),
        );
        let (addr2, _handle2) = spawn_test_server_with(
            Arc::new(EventBus::new()),
            db_second_run,
            Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set")),
            secrets.clone(),
            "correct-token",
        )
        .await;

        let mut client2 = AccountsClient::connect(format!("http://{addr2}"))
            .await
            .expect("client connects");
        let list_response = client2
            .list_accounts(authed_bearer_request(ListAccountsRequest {}))
            .await
            .expect("list_accounts succeeds")
            .into_inner();

        assert_eq!(list_response.accounts.len(), 1);
        let restored = &list_response.accounts[0];
        assert_eq!(restored.id, "acct-restart-1");
        assert_eq!(restored.email_address, "acct-restart-1@nuncio.mx");
        assert_eq!(restored.keyring_secret_key, "nuncio/acct-restart-1");

        // The credential itself must have survived too, in the (mock)
        // keyring -- never in the database.
        let stored_password = secrets
            .get_secret("nuncio/acct-restart-1")
            .expect("password still retrievable from vault after restart");
        assert_eq!(stored_password, "restart-secret-pw");
    }

    /// Proves the password credential is retrievable from the (mock)
    /// keyring, but never appears in any `accounts` SQLite column, and
    /// never appears in the `ListAccounts` wire response.
    #[tokio::test]
    async fn add_account_password_never_touches_sqlite_or_the_list_response() {
        const PASSWORD: &str = "super-secret-credential-42";

        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("connect ephemeral test db");
        let db = Arc::new(db);
        let secrets = Arc::new(SecretManager::mock());
        let (addr, _handle) = spawn_test_server_with(
            Arc::new(EventBus::new()),
            db.clone(),
            Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set")),
            secrets.clone(),
            "correct-token",
        )
        .await;

        let mut client = AccountsClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        client
            .add_account(authed_bearer_request(AddAccountRequest {
                config: Some(sample_account_config_proto(
                    "acct-secrecy-1",
                    "nuncio/acct-secrecy-1",
                )),
                password: PASSWORD.to_string(),
            }))
            .await
            .expect("add_account succeeds");

        // The password IS retrievable from the (mock) keyring vault, keyed
        // by this account's `keyring_secret_key`.
        let stored_password = secrets
            .get_secret("nuncio/acct-secrecy-1")
            .expect("password stored in vault");
        assert_eq!(stored_password, PASSWORD);

        // The password is NEVER present in any `accounts` table column.
        #[allow(clippy::type_complexity)]
        let rows: Vec<(
            String,
            String,
            String,
            String,
            String,
            i64,
            i64,
            String,
            i64,
        )> = sqlx::query_as(
            "SELECT id, name, email_address, protocol, server_host, server_port, use_tls, \
                 keyring_secret_key, sync_interval_secs FROM accounts",
        )
        .fetch_all(db.pool())
        .await
        .expect("query accounts table directly");
        assert_eq!(rows.len(), 1);
        let (id, name, email_address, protocol, server_host, _port, _tls, keyring_key, _interval) =
            &rows[0];
        for text_column in [
            id.as_str(),
            name.as_str(),
            email_address.as_str(),
            protocol.as_str(),
            server_host.as_str(),
            keyring_key.as_str(),
        ] {
            assert!(
                !text_column.contains(PASSWORD),
                "password leaked into an accounts DB column: {text_column:?}"
            );
        }

        // The password is NEVER present in the `ListAccounts` response.
        let list_response = client
            .list_accounts(authed_bearer_request(ListAccountsRequest {}))
            .await
            .expect("list_accounts succeeds")
            .into_inner();
        assert_eq!(list_response.accounts.len(), 1);
        let response_debug = format!("{list_response:?}");
        assert!(
            !response_debug.contains(PASSWORD),
            "password leaked into the ListAccounts response: {response_debug}"
        );
    }

    // ---- Mail ----

    use nuncio_proto::v1::mail_client::MailClient;
    use nuncio_proto::v1::{
        GetMessageRequest, ListFoldersRequest, ListMessagesRequest, MarkReadRequest,
        SearchMessagesRequest, SendMessageRequest, SyncRequest,
    };

    fn sample_email(
        id: &str,
        folder_id: &str,
        subject: &str,
        body: &str,
    ) -> nuncio_core::model::Email {
        nuncio_core::model::Email {
            id: id.to_string(),
            account_id: "acct-mail-1".to_string(),
            folder_id: folder_id.to_string(),
            subject: subject.to_string(),
            sender: "alice@nuncio.mx".to_string(),
            recipient: "bob@nuncio.mx".to_string(),
            received_at: 1_700_000_000,
            read: false,
            body_plain: Some(body.to_string()),
            body_html: None,
            attachments: Vec::new(),
        }
    }

    #[tokio::test]
    async fn mail_rpcs_reject_missing_bearer_token() {
        // Confirms `Mail` is mounted behind its own `BearerAuthInterceptor`
        // exactly like `System` and `Accounts` (no un-intercepted service).
        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle, _dir) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = MailClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");

        let err = client
            .list_folders(ListFoldersRequest {})
            .await
            .expect_err("missing bearer token must be rejected");
        assert_eq!(err.code(), Code::Unauthenticated);

        let err = client
            .list_messages(ListMessagesRequest {
                folder_id: "inbox".to_string(),
                limit: 10,
            })
            .await
            .expect_err("missing bearer token must be rejected");
        assert_eq!(err.code(), Code::Unauthenticated);

        let err = client
            .get_message(GetMessageRequest {
                message_id: "msg-1".to_string(),
            })
            .await
            .expect_err("missing bearer token must be rejected");
        assert_eq!(err.code(), Code::Unauthenticated);

        let err = client
            .mark_read(MarkReadRequest {
                message_id: "msg-1".to_string(),
                read: true,
            })
            .await
            .expect_err("missing bearer token must be rejected");
        assert_eq!(err.code(), Code::Unauthenticated);

        let err = client
            .search_messages(SearchMessagesRequest {
                query: "hello".to_string(),
            })
            .await
            .expect_err("missing bearer token must be rejected");
        assert_eq!(err.code(), Code::Unauthenticated);

        let err = client
            .send_message(SendMessageRequest {
                to: "bob@nuncio.mx".to_string(),
                cc: None,
                subject: "Hi".to_string(),
                body_text: "Body".to_string(),
                body_html: None,
                attachments: Vec::new(),
            })
            .await
            .expect_err("missing bearer token must be rejected");
        assert_eq!(err.code(), Code::Unauthenticated);

        let err = client
            .sync(SyncRequest { account_id: None })
            .await
            .expect_err("missing bearer token must be rejected");
        assert_eq!(err.code(), Code::Unauthenticated);
    }

    #[tokio::test]
    async fn calendar_rpcs_reject_missing_bearer_token() {
        // Confirms `Calendar` is mounted behind its own
        // `BearerAuthInterceptor` exactly like every other service (no
        // un-intercepted service).
        use nuncio_proto::v1::calendar_client::CalendarClient;
        use nuncio_proto::v1::{CalendarSyncRequest, GetEventRequest, ListEventsRequest};

        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle, _dir) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = CalendarClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");

        let err = client
            .list_events(ListEventsRequest {
                account_id: "acct-1".to_string(),
                calendar_id: "cal-1".to_string(),
                start_window: 0,
                end_window: i64::MAX,
            })
            .await
            .expect_err("missing bearer token must be rejected");
        assert_eq!(err.code(), Code::Unauthenticated);

        let err = client
            .get_event(GetEventRequest {
                event_id: "evt-1".to_string(),
            })
            .await
            .expect_err("missing bearer token must be rejected");
        assert_eq!(err.code(), Code::Unauthenticated);

        let err = client
            .sync(CalendarSyncRequest {
                account_id: "acct-1".to_string(),
                calendar_id: "cal-1".to_string(),
                start_window: 0,
                end_window: i64::MAX,
            })
            .await
            .expect_err("missing bearer token must be rejected");
        assert_eq!(err.code(), Code::Unauthenticated);
    }

    #[tokio::test]
    async fn get_message_reports_not_found_for_unknown_message_id() {
        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle, _dir) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = MailClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let err = client
            .get_message(authed_bearer_request(GetMessageRequest {
                message_id: "does-not-exist".to_string(),
            }))
            .await
            .expect_err("unknown message id must be rejected");
        assert_eq!(err.code(), Code::NotFound);
    }

    #[tokio::test]
    async fn mark_read_reports_not_found_for_unknown_message_id() {
        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle, _dir) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = MailClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let err = client
            .mark_read(authed_bearer_request(MarkReadRequest {
                message_id: "does-not-exist".to_string(),
                read: true,
            }))
            .await
            .expect_err("unknown message id must be rejected");
        assert_eq!(err.code(), Code::NotFound);
    }

    #[tokio::test]
    async fn list_messages_rejects_empty_folder_id() {
        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle, _dir) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = MailClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let err = client
            .list_messages(authed_bearer_request(ListMessagesRequest {
                folder_id: String::new(),
                limit: 10,
            }))
            .await
            .expect_err("empty folder_id must be rejected");
        assert_eq!(err.code(), Code::InvalidArgument);
    }

    /// End-to-end proof: seeds the daemon's real, persistent store directly
    /// via `DatabaseEngine::save_email` (standing in for the real sync path,
    /// which uses the exact same write path), then proves every `Mail` RPC
    /// round-trips real data: `ListFolders` reports the seeded folder,
    /// `ListMessages` returns the seeded messages, `GetMessage` returns the
    /// full (decrypted) body, `SearchMessages` finds a body term via FTS5,
    /// and `MarkRead` persists across a second `GetMessage` call AND
    /// publishes `MessageFlagsChanged` on the live event bus (proving it
    /// would stream via `System/Subscribe`).
    #[tokio::test]
    async fn mail_rpcs_round_trip_real_seeded_data_and_mark_read_persists_and_streams() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("connect ephemeral test db");
        db.save_email(&sample_email(
            "msg-mail-1",
            "inbox",
            "Quarterly Roadmap",
            "Let's discuss the annual revenue forecast",
        ))
        .await
        .expect("seed message 1");
        db.save_email(&sample_email(
            "msg-mail-2",
            "inbox",
            "Lunch Plans",
            "Sandwiches at noon",
        ))
        .await
        .expect("seed message 2");

        let event_bus = Arc::new(EventBus::new());
        let mut events = event_bus.subscribe_events();
        let secrets = Arc::new(SecretManager::mock());
        let filter_engine = Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set"));
        let (addr, _handle) = spawn_test_server_with(
            event_bus,
            Arc::new(db),
            filter_engine,
            secrets,
            "correct-token",
        )
        .await;

        let mut client = MailClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");

        // ListFolders reports the seeded folder with the right counts.
        let folders = client
            .list_folders(authed_bearer_request(ListFoldersRequest {}))
            .await
            .expect("list_folders succeeds")
            .into_inner()
            .folders;
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].id, "inbox");
        assert_eq!(folders[0].total_messages, 2);
        assert_eq!(folders[0].unread_messages, 2);

        // ListMessages returns both seeded messages, newest-first.
        let messages = client
            .list_messages(authed_bearer_request(ListMessagesRequest {
                folder_id: "inbox".to_string(),
                limit: 10,
            }))
            .await
            .expect("list_messages succeeds")
            .into_inner()
            .messages;
        assert_eq!(messages.len(), 2);
        let ids: Vec<&str> = messages.iter().map(|m| m.id.as_str()).collect();
        assert!(ids.contains(&"msg-mail-1"));
        assert!(ids.contains(&"msg-mail-2"));

        // GetMessage returns the full, decrypted body.
        let fetched = client
            .get_message(authed_bearer_request(GetMessageRequest {
                message_id: "msg-mail-1".to_string(),
            }))
            .await
            .expect("get_message succeeds")
            .into_inner()
            .message
            .expect("message present in response");
        assert_eq!(fetched.subject, "Quarterly Roadmap");
        assert_eq!(
            fetched.body_plain.as_deref(),
            Some("Let's discuss the annual revenue forecast")
        );
        assert!(!fetched.read);

        // SearchMessages finds a body term via the real FTS5 index.
        let hits = client
            .search_messages(authed_bearer_request(SearchMessagesRequest {
                query: "revenue".to_string(),
            }))
            .await
            .expect("search_messages succeeds")
            .into_inner()
            .hits;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "msg-mail-1");

        // MarkRead persists: a second GetMessage call reflects the flip.
        client
            .mark_read(authed_bearer_request(MarkReadRequest {
                message_id: "msg-mail-1".to_string(),
                read: true,
            }))
            .await
            .expect("mark_read succeeds");

        let refetched = client
            .get_message(authed_bearer_request(GetMessageRequest {
                message_id: "msg-mail-1".to_string(),
            }))
            .await
            .expect("get_message succeeds after mark_read")
            .into_inner()
            .message
            .expect("message present in response");
        assert!(refetched.read);

        // MarkRead publishes `MessageFlagsChanged` on the live event bus, so
        // it would stream via `System/Subscribe` for a connected client.
        let event = events.recv().await.expect("event bus yields an event");
        assert_eq!(
            event,
            CoreEvent::MessageFlagsChanged {
                message_id: "msg-mail-1".to_string(),
                read: true,
            }
        );
    }

    // ---- SendMessage ----

    fn valid_send_message_request() -> SendMessageRequest {
        SendMessageRequest {
            to: "bob@nuncio.mx".to_string(),
            cc: None,
            subject: "Quarterly Roadmap".to_string(),
            body_text: "Let's discuss the roadmap.".to_string(),
            body_html: None,
            attachments: Vec::new(),
        }
    }

    #[tokio::test]
    async fn send_message_rejects_empty_to() {
        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle, _dir) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = MailClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let mut req = valid_send_message_request();
        req.to = String::new();
        let err = client
            .send_message(authed_bearer_request(req))
            .await
            .expect_err("empty to must be rejected");
        assert_eq!(err.code(), Code::InvalidArgument);
    }

    #[tokio::test]
    async fn send_message_rejects_empty_subject() {
        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle, _dir) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = MailClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let mut req = valid_send_message_request();
        req.subject = String::new();
        let err = client
            .send_message(authed_bearer_request(req))
            .await
            .expect_err("empty subject must be rejected");
        assert_eq!(err.code(), Code::InvalidArgument);
    }

    /// Proves `SendMessage` never fabricates success -- with no account
    /// configured at all, the daemon has nothing to send from, and this
    /// surfaces as `Status::internal` rather than a fabricated
    /// `SendMessageResponse`.
    #[tokio::test]
    async fn send_message_reports_honest_error_when_no_account_configured() {
        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle, _dir) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = MailClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let err = client
            .send_message(authed_bearer_request(valid_send_message_request()))
            .await
            .expect_err("no configured account must be rejected");
        assert_eq!(err.code(), Code::Internal);
    }

    /// Proves `SendMessage` genuinely builds a real SMTP transport from the
    /// account's `smtp_host`/`smtp_port` and its keyring password, and
    /// surfaces a real transport failure honestly --
    /// WITHOUT any live network dependency: `smtp_host`/`smtp_port` point at
    /// a reserved loopback port nothing is listening on, so the connection
    /// attempt fails fast and deterministically (mirroring
    /// `nuncio_mail::smtp`'s own unreachable-server test).
    #[tokio::test]
    async fn send_message_builds_real_smtp_transport_and_fails_honestly_when_unreachable() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("connect ephemeral test db");
        let db = Arc::new(db);
        let secrets = Arc::new(SecretManager::mock());

        let config = nuncio_core::AccountConfig {
            id: "acct-send-1".to_string(),
            name: "Send Test Account".to_string(),
            email_address: "sender@nuncio.mx".to_string(),
            protocol: nuncio_core::AccountProtocol::ImapSmtp,
            server_host: "imap.nuncio.mx".to_string(),
            server_port: 993,
            smtp_host: "127.0.0.1".to_string(),
            smtp_port: 1,
            use_tls: true,
            imap_tls_mode: nuncio_core::TlsMode::ImplicitTls,
            smtp_tls_mode: nuncio_core::TlsMode::ImplicitTls,
            keyring_secret_key: "nuncio/acct-send-1".to_string(),
            sync_interval_secs: 60,
        };
        db.save_account(&config).await.expect("save account");
        secrets
            .set_secret(&config.keyring_secret_key, "irrelevant-password")
            .expect("store credential in mock vault");

        let filter_engine = Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set"));
        let (addr, _handle) = spawn_test_server_with(
            Arc::new(EventBus::new()),
            db,
            filter_engine,
            secrets,
            "correct-token",
        )
        .await;

        let mut client = MailClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let err = client
            .send_message(authed_bearer_request(valid_send_message_request()))
            .await
            .expect_err("delivery to an unreachable SMTP host must fail");
        assert_eq!(err.code(), Code::Internal);
        assert!(err.message().contains("failed to send message"));
    }

    // ---- Sync ----

    /// With no `MailEngineOverrides` and no configured accounts, `Sync`
    /// exercises the real production `run_all_accounts_sync` path and
    /// honestly reports zero messages synced (never fabricating a nonzero
    /// count).
    #[tokio::test]
    async fn sync_without_override_and_without_accounts_reports_zero_synced() {
        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle, _dir) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = MailClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let response = client
            .sync(authed_bearer_request(SyncRequest { account_id: None }))
            .await
            .expect("sync succeeds")
            .into_inner();
        assert_eq!(response.synced_count, 0);
    }

    /// With no `MailEngineOverrides`, requesting a sync for an account_id
    /// that is not configured exercises the real production
    /// `run_account_sync` path and surfaces an honest `Status::internal`
    /// error rather than a fabricated success.
    #[tokio::test]
    async fn sync_without_override_reports_error_for_unknown_account() {
        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle, _dir) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = MailClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let err = client
            .sync(authed_bearer_request(SyncRequest {
                account_id: Some("acct-does-not-exist".to_string()),
            }))
            .await
            .expect_err("unknown account must be rejected");
        assert_eq!(err.code(), Code::Internal);
        assert!(err.message().contains("acct-does-not-exist"));
    }

    /// With a [`MailEngineOverrides::mail_backend`] injected, `Sync` fetches
    /// from it (via `sync_with_backend`) instead of resolving a real
    /// per-account engine from keyring credentials, and awaits full
    /// completion before returning -- proving a caller can immediately
    /// `ListMessages`/`GetMessage` the synced data with no fixed sleep.
    /// This is the exact mechanism the full-daemon offline spine E2E test
    /// (`spine_e2e_test.rs`) uses.
    #[tokio::test]
    async fn sync_with_injected_backend_persists_and_reports_count_deterministically() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("connect ephemeral test db");
        let db = Arc::new(db);
        let secrets = Arc::new(SecretManager::mock());

        let mock_backend = nuncio_mail::MockMailBackend::new();
        mock_backend.add_folder(nuncio_core::model::Folder {
            id: "inbox".to_string(),
            name: "Inbox".to_string(),
            total_messages: 1,
            unread_messages: 1,
        });
        mock_backend.add_message(sample_email(
            "msg-sync-injected-1",
            "inbox",
            "Injected Sync Subject",
            "Injected sync body",
        ));

        let overrides = MailEngineOverrides {
            mail_backend: Some(Arc::new(mock_backend)),
            message_sender: None,
        };
        let filter_engine = Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set"));
        let (addr, _handle) = spawn_test_server_with_overrides(
            Arc::new(EventBus::new()),
            db,
            filter_engine,
            secrets,
            "correct-token",
            overrides,
        )
        .await;

        let mut client = MailClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");

        let response = client
            .sync(authed_bearer_request(SyncRequest { account_id: None }))
            .await
            .expect("sync succeeds")
            .into_inner();
        assert_eq!(response.synced_count, 1);

        // Immediately visible, no fixed sleep: `Sync` only returns once the
        // fetch-and-persist work has fully completed.
        let fetched = client
            .get_message(authed_bearer_request(GetMessageRequest {
                message_id: "msg-sync-injected-1".to_string(),
            }))
            .await
            .expect("get_message succeeds")
            .into_inner()
            .message
            .expect("message present in response");
        assert_eq!(fetched.subject, "Injected Sync Subject");
        assert_eq!(fetched.body_plain.as_deref(), Some("Injected sync body"));
    }

    /// With a [`MailEngineOverrides::message_sender`] injected,
    /// `SendMessage` sends through it (via
    /// `send_message_with_injected_sender`) instead of building a real SMTP
    /// transport, and the injected mock captures the EXACT outbound message
    /// -- recipient, subject, and body -- the caller sent over the wire.
    #[tokio::test]
    async fn send_message_with_injected_sender_captures_exact_outbound_message() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("connect ephemeral test db");
        let db = Arc::new(db);
        let secrets = Arc::new(SecretManager::mock());

        let config = nuncio_core::AccountConfig {
            id: "acct-injected-send-1".to_string(),
            name: "Injected Send Test Account".to_string(),
            email_address: "sender@nuncio.mx".to_string(),
            protocol: nuncio_core::AccountProtocol::ImapSmtp,
            server_host: "imap.nuncio.mx".to_string(),
            server_port: 993,
            smtp_host: "smtp.nuncio.mx".to_string(),
            smtp_port: 465,
            use_tls: true,
            imap_tls_mode: nuncio_core::TlsMode::ImplicitTls,
            smtp_tls_mode: nuncio_core::TlsMode::ImplicitTls,
            keyring_secret_key: "nuncio/acct-injected-send-1".to_string(),
            sync_interval_secs: 60,
        };
        // Deliberately never stores a keyring credential -- this path must
        // never need one.
        db.save_account(&config).await.expect("save account");

        let mock_sender = nuncio_mail::MockMessageSender::new();
        let sent_probe = mock_sender.clone();
        let overrides = MailEngineOverrides {
            mail_backend: None,
            message_sender: Some(Arc::new(mock_sender)),
        };
        let filter_engine = Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set"));
        let (addr, _handle) = spawn_test_server_with_overrides(
            Arc::new(EventBus::new()),
            db,
            filter_engine,
            secrets,
            "correct-token",
            overrides,
        )
        .await;

        let mut client = MailClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");

        let response = client
            .send_message(authed_bearer_request(SendMessageRequest {
                to: "alice@nuncio.mx".to_string(),
                cc: Some("carol@nuncio.mx".to_string()),
                subject: "Injected Send Subject".to_string(),
                body_text: "Injected send body".to_string(),
                body_html: None,
                attachments: Vec::new(),
            }))
            .await
            .expect("send_message succeeds")
            .into_inner();
        assert!(response.message_id.starts_with("sent-"));

        let sent = sent_probe.sent_messages();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].from, "sender@nuncio.mx");
        assert_eq!(sent[0].to, "alice@nuncio.mx");
        assert_eq!(sent[0].cc.as_deref(), Some("carol@nuncio.mx"));
        assert_eq!(sent[0].subject, "Injected Send Subject");
        assert_eq!(sent[0].body_plain.as_deref(), Some("Injected send body"));
    }

    /// With a [`MailEngineOverrides::mail_backend`] injected and configured
    /// to simulate a failure, `Sync` surfaces the genuine
    /// `sync_with_backend` error as `Status::internal` rather than a
    /// fabricated success.
    #[tokio::test]
    async fn sync_with_injected_backend_reports_error_when_backend_fails() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("connect ephemeral test db");
        let db = Arc::new(db);
        let secrets = Arc::new(SecretManager::mock());

        let mock_backend = nuncio_mail::MockMailBackend::new();
        mock_backend.set_should_fail(true);
        let overrides = MailEngineOverrides {
            mail_backend: Some(Arc::new(mock_backend)),
            message_sender: None,
        };
        let filter_engine = Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set"));
        let (addr, _handle) = spawn_test_server_with_overrides(
            Arc::new(EventBus::new()),
            db,
            filter_engine,
            secrets,
            "correct-token",
            overrides,
        )
        .await;

        let mut client = MailClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let err = client
            .sync(authed_bearer_request(SyncRequest { account_id: None }))
            .await
            .expect_err("a failing injected backend must be rejected honestly");
        assert_eq!(err.code(), Code::Internal);
        assert!(err.message().contains("sync failed"));
    }

    /// With a [`MailEngineOverrides::message_sender`] injected and
    /// configured to simulate a transport failure, `SendMessage` surfaces
    /// the genuine `send_message_with_injected_sender` error as
    /// `Status::internal` rather than a fabricated success.
    #[tokio::test]
    async fn send_message_with_injected_sender_reports_error_when_transport_fails() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("connect ephemeral test db");
        let db = Arc::new(db);
        let secrets = Arc::new(SecretManager::mock());

        let config = nuncio_core::AccountConfig {
            id: "acct-injected-send-fail-1".to_string(),
            name: "Injected Send Failure Test Account".to_string(),
            email_address: "sender@nuncio.mx".to_string(),
            protocol: nuncio_core::AccountProtocol::ImapSmtp,
            server_host: "imap.nuncio.mx".to_string(),
            server_port: 993,
            smtp_host: "smtp.nuncio.mx".to_string(),
            smtp_port: 465,
            use_tls: true,
            imap_tls_mode: nuncio_core::TlsMode::ImplicitTls,
            smtp_tls_mode: nuncio_core::TlsMode::ImplicitTls,
            keyring_secret_key: "nuncio/acct-injected-send-fail-1".to_string(),
            sync_interval_secs: 60,
        };
        db.save_account(&config).await.expect("save account");

        let mock_sender = nuncio_mail::MockMessageSender::new();
        mock_sender.set_should_fail(true);
        let overrides = MailEngineOverrides {
            mail_backend: None,
            message_sender: Some(Arc::new(mock_sender)),
        };
        let filter_engine = Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set"));
        let (addr, _handle) = spawn_test_server_with_overrides(
            Arc::new(EventBus::new()),
            db,
            filter_engine,
            secrets,
            "correct-token",
            overrides,
        )
        .await;

        let mut client = MailClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let err = client
            .send_message(authed_bearer_request(valid_send_message_request()))
            .await
            .expect_err("a failing injected sender must be rejected honestly");
        assert_eq!(err.code(), Code::Internal);
        assert!(err.message().contains("failed to send message"));
    }

    // ---- Filters ----

    use nuncio_proto::v1::filters_client::FiltersClient;
    use nuncio_proto::v1::{
        CreateRuleRequest, DeleteRuleRequest, ListRulesRequest, PreviewRuleRequest,
        ValidateRuleRequest,
    };

    const SAMPLE_RULE_NSQL: &str =
        "WHERE subject CONTAINS 'Urgent' ACTION MARK READ, MOVE TO 'Priority'";

    /// Spawns a test server on an ephemeral loopback port backed by a fresh
    /// ephemeral database and a fresh empty [`FilterEngine`], returning the
    /// address AND the shared `filter_engine`/`db` handles so `Filters`
    /// tests can assert directly on live engine state (proving the
    /// `ArcSwap` reload actually happened) in addition to driving the gRPC
    /// API.
    async fn spawn_filters_test_server(
        token: &str,
    ) -> (
        std::net::SocketAddr,
        tokio::task::JoinHandle<()>,
        Arc<DatabaseEngine>,
        Arc<FilterEngine>,
        tempfile::TempDir,
    ) {
        // The `TempDir` guard must outlive this function: on Linux, dropping it
        // here would unlink the directory backing the sqlite file while `db`
        // is still in use, so any pool connection opened *after* this
        // function returns fails with "unable to open database file". Return
        // it to the caller instead so it stays alive for the test's duration.
        let (db, dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("connect ephemeral test db");
        let db = Arc::new(db);
        let filter_engine = Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set"));
        let secrets = Arc::new(SecretManager::mock());
        let (addr, handle) = spawn_test_server_with(
            Arc::new(EventBus::new()),
            db.clone(),
            filter_engine.clone(),
            secrets,
            token,
        )
        .await;
        (addr, handle, db, filter_engine, dir)
    }

    #[tokio::test]
    async fn filters_rpcs_reject_missing_bearer_token() {
        // Confirms `Filters` is mounted behind its own `BearerAuthInterceptor`
        // exactly like `System`/`Accounts`/`Mail` (no un-intercepted
        // service).
        let (addr, _handle, _db, _engine, _dir) = spawn_filters_test_server("correct-token").await;

        let mut client = FiltersClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");

        let err = client
            .create_rule(CreateRuleRequest {
                name: "Unauthed".to_string(),
                nsql: SAMPLE_RULE_NSQL.to_string(),
                priority: 0,
            })
            .await
            .expect_err("missing bearer token must be rejected");
        assert_eq!(err.code(), Code::Unauthenticated);

        let err = client
            .list_rules(ListRulesRequest {})
            .await
            .expect_err("missing bearer token must be rejected");
        assert_eq!(err.code(), Code::Unauthenticated);

        let err = client
            .delete_rule(DeleteRuleRequest {
                id: "rule-1".to_string(),
            })
            .await
            .expect_err("missing bearer token must be rejected");
        assert_eq!(err.code(), Code::Unauthenticated);

        let err = client
            .validate_rule(ValidateRuleRequest {
                nsql: SAMPLE_RULE_NSQL.to_string(),
            })
            .await
            .expect_err("missing bearer token must be rejected");
        assert_eq!(err.code(), Code::Unauthenticated);

        let err = client
            .preview_rule(PreviewRuleRequest {
                nsql: SAMPLE_RULE_NSQL.to_string(),
                message_id: None,
            })
            .await
            .expect_err("missing bearer token must be rejected");
        assert_eq!(err.code(), Code::Unauthenticated);
    }

    /// End-to-end proof: `CreateRule` persists AND reloads the live
    /// `FilterEngine`'s `ArcSwap` rule set
    /// (proven by evaluating the SAME `filter_engine` instance directly,
    /// not just re-reading it back over `ListRules`); `ListRules` reflects
    /// the persisted rule; `DeleteRule` removes it AND reloads the engine
    /// back to empty.
    #[tokio::test]
    async fn create_list_and_delete_rule_round_trip_and_keep_the_live_engine_in_sync() {
        let (addr, _handle, _db, filter_engine, _dir) =
            spawn_filters_test_server("correct-token").await;

        let mut client = FiltersClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");

        let sample_email = |subject: &str| nuncio_core::model::Email {
            id: "msg-filters-1".to_string(),
            account_id: "acct-1".to_string(),
            folder_id: "inbox".to_string(),
            subject: subject.to_string(),
            sender: "alice@nuncio.mx".to_string(),
            recipient: "bob@nuncio.mx".to_string(),
            received_at: 1_700_000_000,
            read: false,
            body_plain: Some("body".to_string()),
            body_html: None,
            attachments: Vec::new(),
        };

        // Before creating anything, the live engine matches nothing.
        assert!(filter_engine
            .evaluate(&sample_email("Urgent Meeting"))
            .is_empty());

        let create_response = client
            .create_rule(authed_bearer_request(CreateRuleRequest {
                name: "Urgent Rule".to_string(),
                nsql: SAMPLE_RULE_NSQL.to_string(),
                priority: 5,
            }))
            .await
            .expect("create_rule succeeds")
            .into_inner();
        let created_rule = create_response.rule.expect("response carries the rule");
        assert_eq!(created_rule.name, "Urgent Rule");
        assert_eq!(created_rule.priority, 5);
        assert!(created_rule.enabled);
        assert_eq!(created_rule.nsql_text, SAMPLE_RULE_NSQL);
        assert!(created_rule.actions.iter().any(|a| a.contains("MARK READ")));
        let rule_id = created_rule.id.clone();

        // The live `FilterEngine`'s `ArcSwap` rule set was reloaded --
        // proven by evaluating the SAME instance the server holds, not a
        // fresh one.
        let matches = filter_engine.evaluate(&sample_email("Urgent Meeting"));
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].0.id, rule_id);

        // ListRules reflects the persisted rule.
        let list_response = client
            .list_rules(authed_bearer_request(ListRulesRequest {}))
            .await
            .expect("list_rules succeeds")
            .into_inner();
        assert_eq!(list_response.rules.len(), 1);
        assert_eq!(list_response.rules[0].id, rule_id);

        // DeleteRule removes it AND reloads the engine back to empty.
        client
            .delete_rule(authed_bearer_request(DeleteRuleRequest {
                id: rule_id.clone(),
            }))
            .await
            .expect("delete_rule succeeds");

        let list_after_delete = client
            .list_rules(authed_bearer_request(ListRulesRequest {}))
            .await
            .expect("list_rules succeeds")
            .into_inner();
        assert!(list_after_delete.rules.is_empty());
        assert!(filter_engine
            .evaluate(&sample_email("Urgent Meeting"))
            .is_empty());
    }

    #[tokio::test]
    async fn create_rule_rejects_invalid_nsql_and_persists_nothing() {
        let (addr, _handle, _db, filter_engine, _dir) =
            spawn_filters_test_server("correct-token").await;

        let mut client = FiltersClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");

        let err = client
            .create_rule(authed_bearer_request(CreateRuleRequest {
                name: "Broken Rule".to_string(),
                nsql: "THIS IS NOT VALID NSQL AT ALL {{{".to_string(),
                priority: 0,
            }))
            .await
            .expect_err("invalid NSQL must be rejected");
        assert_eq!(err.code(), Code::InvalidArgument);

        let list_response = client
            .list_rules(authed_bearer_request(ListRulesRequest {}))
            .await
            .expect("list_rules succeeds")
            .into_inner();
        assert!(
            list_response.rules.is_empty(),
            "an invalid rule must never be persisted"
        );
        assert!(filter_engine
            .evaluate(&nuncio_core::model::Email {
                id: "msg-1".to_string(),
                account_id: "acct-1".to_string(),
                folder_id: "inbox".to_string(),
                subject: "anything".to_string(),
                sender: "a@b.com".to_string(),
                recipient: "c@d.com".to_string(),
                received_at: 0,
                read: false,
                body_plain: None,
                body_html: None,
                attachments: Vec::new(),
            })
            .is_empty());
    }

    #[tokio::test]
    async fn delete_rule_rejects_empty_id() {
        let (addr, _handle, _db, _engine, _dir) = spawn_filters_test_server("correct-token").await;

        let mut client = FiltersClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");

        let err = client
            .delete_rule(authed_bearer_request(DeleteRuleRequest {
                id: String::new(),
            }))
            .await
            .expect_err("empty id must be rejected");
        assert_eq!(err.code(), Code::InvalidArgument);
    }

    /// `ValidateRule` never fails the RPC itself -- an invalid rule is an
    /// honest `valid: false` result, not a `Status` error (see this RPC's
    /// doc comment). Also proves validation never persists anything.
    #[tokio::test]
    async fn validate_rule_reports_honest_ok_and_error_verdicts_without_persisting() {
        let (addr, _handle, _db, _engine, _dir) = spawn_filters_test_server("correct-token").await;

        let mut client = FiltersClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");

        let valid_response = client
            .validate_rule(authed_bearer_request(ValidateRuleRequest {
                nsql: SAMPLE_RULE_NSQL.to_string(),
            }))
            .await
            .expect("validate_rule call itself succeeds for a valid rule")
            .into_inner();
        assert!(valid_response.valid);
        assert!(valid_response.error.is_empty());

        let invalid_response = client
            .validate_rule(authed_bearer_request(ValidateRuleRequest {
                nsql: "THIS IS NOT VALID NSQL AT ALL {{{".to_string(),
            }))
            .await
            .expect("validate_rule call itself succeeds even for an invalid rule")
            .into_inner();
        assert!(!invalid_response.valid);
        assert!(!invalid_response.error.is_empty());

        // Nothing was persisted by either validation call.
        let list_response = client
            .list_rules(authed_bearer_request(ListRulesRequest {}))
            .await
            .expect("list_rules succeeds")
            .into_inner();
        assert!(
            list_response.rules.is_empty(),
            "validate_rule must never persist a rule"
        );
    }

    /// `PreviewRule` dry-run evaluates an ad hoc rule against a stored
    /// message (proving it reads REAL persisted mail via the same store
    /// `Mail/GetMessage` reads) without ever persisting the rule itself.
    #[tokio::test]
    async fn preview_rule_dry_runs_against_a_stored_message_without_persisting() {
        let (addr, _handle, db, _engine, _dir) = spawn_filters_test_server("correct-token").await;

        db.save_email(&nuncio_core::model::Email {
            id: "msg-preview-1".to_string(),
            account_id: "acct-1".to_string(),
            folder_id: "inbox".to_string(),
            subject: "Urgent Board Meeting".to_string(),
            sender: "alice@nuncio.mx".to_string(),
            recipient: "bob@nuncio.mx".to_string(),
            received_at: 1_700_000_000,
            read: false,
            body_plain: Some("Please review the attached deck".to_string()),
            body_html: None,
            attachments: Vec::new(),
        })
        .await
        .expect("seed message");

        let mut client = FiltersClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");

        let response = client
            .preview_rule(authed_bearer_request(PreviewRuleRequest {
                nsql: SAMPLE_RULE_NSQL.to_string(),
                message_id: Some("msg-preview-1".to_string()),
            }))
            .await
            .expect("preview_rule succeeds")
            .into_inner();
        assert_eq!(response.message_id, "msg-preview-1");
        assert!(response.matched);
        assert!(response
            .actions_evaluated
            .iter()
            .any(|a| a.contains("MARK READ")));
        assert!(!response.condition_traces.is_empty());

        // Preview evaluates a NON-matching message honestly too.
        let non_matching = client
            .preview_rule(authed_bearer_request(PreviewRuleRequest {
                nsql: "WHERE subject CONTAINS 'Nonexistent Term' ACTION DELETE".to_string(),
                message_id: Some("msg-preview-1".to_string()),
            }))
            .await
            .expect("preview_rule succeeds")
            .into_inner();
        assert!(!non_matching.matched);

        // Previewing must never persist the rule.
        let list_response = client
            .list_rules(authed_bearer_request(ListRulesRequest {}))
            .await
            .expect("list_rules succeeds")
            .into_inner();
        assert!(
            list_response.rules.is_empty(),
            "preview_rule must never persist a rule"
        );
    }

    /// When no `message_id` is given (or it does not resolve to a stored
    /// message), `PreviewRule` falls back to a fixed synthetic sample
    /// message rather than failing the RPC.
    #[tokio::test]
    async fn preview_rule_falls_back_to_a_synthetic_message_when_none_is_stored() {
        let (addr, _handle, _db, _engine, _dir) = spawn_filters_test_server("correct-token").await;

        let mut client = FiltersClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");

        let response = client
            .preview_rule(authed_bearer_request(PreviewRuleRequest {
                nsql: "WHERE subject CONTAINS 'Test Subject' ACTION FLAG".to_string(),
                message_id: None,
            }))
            .await
            .expect("preview_rule succeeds with no message_id")
            .into_inner();
        assert!(response.matched);
        assert_eq!(response.message_id, "msg-test");

        let response_missing_id = client
            .preview_rule(authed_bearer_request(PreviewRuleRequest {
                nsql: "WHERE subject CONTAINS 'Test Subject' ACTION FLAG".to_string(),
                message_id: Some("does-not-exist".to_string()),
            }))
            .await
            .expect("preview_rule succeeds with an unresolvable message_id")
            .into_inner();
        assert!(response_missing_id.matched);
        assert_eq!(response_missing_id.message_id, "does-not-exist");
    }

    #[tokio::test]
    async fn preview_rule_and_validate_rule_reject_invalid_nsql_syntax() {
        let (addr, _handle, _db, _engine, _dir) = spawn_filters_test_server("correct-token").await;

        let mut client = FiltersClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");

        let err = client
            .preview_rule(authed_bearer_request(PreviewRuleRequest {
                nsql: "THIS IS NOT VALID NSQL AT ALL {{{".to_string(),
                message_id: None,
            }))
            .await
            .expect_err("invalid NSQL must be rejected");
        assert_eq!(err.code(), Code::InvalidArgument);
    }

    // ---- Export ----

    use nuncio_proto::v1::export_client::ExportClient;

    #[tokio::test]
    async fn export_rpcs_reject_missing_bearer_token() {
        // Confirms `Export` is mounted behind its own `BearerAuthInterceptor`
        // exactly like every other service (no un-intercepted service).
        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle, _dir) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = ExportClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let err = client
            .export_mailbox(ExportRequest {
                scope: None,
                format: ExportFormatProto::Json.into(),
                output_path: "irrelevant.json".to_string(),
            })
            .await
            .expect_err("missing bearer token must be rejected");
        assert_eq!(err.code(), Code::Unauthenticated);
    }

    #[tokio::test]
    async fn export_mailbox_rejects_empty_output_path() {
        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle, _dir) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = ExportClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let err = client
            .export_mailbox(authed_bearer_request(ExportRequest {
                scope: None,
                format: ExportFormatProto::Json.into(),
                output_path: String::new(),
            }))
            .await
            .expect_err("empty output_path must be rejected");
        assert_eq!(err.code(), Code::InvalidArgument);
    }

    #[tokio::test]
    async fn export_mailbox_rejects_unspecified_format() {
        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle, _dir) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = ExportClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let err = client
            .export_mailbox(authed_bearer_request(ExportRequest {
                scope: None,
                format: ExportFormatProto::Unspecified.into(),
                output_path: "irrelevant.json".to_string(),
            }))
            .await
            .expect_err("unspecified format must be rejected");
        assert_eq!(err.code(), Code::InvalidArgument);
    }

    /// Seeds the daemon's real store with messages directly via
    /// `DatabaseEngine::save_email` (the same write path `Mail`'s own tests
    /// use), then proves `ExportMailbox` writes a
    /// REAL file to `output_path` with the expected message/byte counts,
    /// and that the file's actual content contains the seeded messages --
    /// never a fabricated summary.
    #[tokio::test]
    async fn export_mailbox_writes_a_real_file_with_the_expected_seeded_messages() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("connect ephemeral test db");
        db.save_email(&sample_email(
            "msg-export-1",
            "inbox",
            "Export Subject One",
            "Export body one",
        ))
        .await
        .expect("seed message 1");
        db.save_email(&sample_email(
            "msg-export-2",
            "archive",
            "Export Subject Two",
            "Export body two",
        ))
        .await
        .expect("seed message 2");

        let event_bus = Arc::new(EventBus::new());
        let secrets = Arc::new(SecretManager::mock());
        let filter_engine = Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set"));
        let (addr, _handle) = spawn_test_server_with(
            event_bus,
            Arc::new(db),
            filter_engine,
            secrets,
            "correct-token",
        )
        .await;

        let mut client = ExportClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");

        let out_dir = tempfile::tempdir().expect("create temp export dir");
        let out_path = out_dir.path().join("export.json");

        let response = client
            .export_mailbox(authed_bearer_request(ExportRequest {
                scope: None,
                format: ExportFormatProto::Json.into(),
                output_path: out_path.to_string_lossy().to_string(),
            }))
            .await
            .expect("export_mailbox succeeds")
            .into_inner();

        assert_eq!(response.message_count, 2);
        assert!(response.bytes_written > 0);
        assert_eq!(response.output_path, out_path.to_string_lossy().to_string());

        let written = std::fs::read_to_string(&out_path).expect("export file was written");
        assert!(written.contains("Export Subject One"));
        assert!(written.contains("Export Subject Two"));
        assert_eq!(written.len() as u64, response.bytes_written);
    }

    /// Proves `ExportRequest.scope`'s `account_id` case narrows the export
    /// to just that account's messages -- the daemon's `Export` gRPC layer
    /// wiring for `DatabaseEngine::list_messages_for_export`'s account
    /// filter, not just its own already-covered unit tests.
    #[tokio::test]
    async fn export_mailbox_scopes_by_account_id() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("connect ephemeral test db");
        let mut msg_a = sample_email("msg-acct-a", "inbox", "From Account A", "Body A");
        msg_a.account_id = "acct-a".to_string();
        db.save_email(&msg_a).await.expect("seed account a message");
        let mut msg_b = sample_email("msg-acct-b", "inbox", "From Account B", "Body B");
        msg_b.account_id = "acct-b".to_string();
        db.save_email(&msg_b).await.expect("seed account b message");

        let event_bus = Arc::new(EventBus::new());
        let secrets = Arc::new(SecretManager::mock());
        let filter_engine = Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set"));
        let (addr, _handle) = spawn_test_server_with(
            event_bus,
            Arc::new(db),
            filter_engine,
            secrets,
            "correct-token",
        )
        .await;

        let mut client = ExportClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");

        let out_dir = tempfile::tempdir().expect("create temp export dir");
        let out_path = out_dir.path().join("scoped.jsonl");

        let response = client
            .export_mailbox(authed_bearer_request(ExportRequest {
                scope: Some(export_request::Scope::AccountId("acct-a".to_string())),
                format: ExportFormatProto::Jsonl.into(),
                output_path: out_path.to_string_lossy().to_string(),
            }))
            .await
            .expect("export_mailbox succeeds")
            .into_inner();

        assert_eq!(response.message_count, 1);
        let written = std::fs::read_to_string(&out_path).expect("export file was written");
        assert!(written.contains("From Account A"));
        assert!(!written.contains("From Account B"));
    }

    // ---- Audit ----

    use nuncio_proto::v1::audit_client::AuditClient;

    #[tokio::test]
    async fn audit_rpcs_reject_missing_bearer_token() {
        // Confirms `Audit` is mounted behind its own `BearerAuthInterceptor`
        // exactly like every other service (no un-intercepted service).
        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle, _dir) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = AuditClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");

        let err = client
            .list_records(ListRecordsRequest {
                limit: 10,
                offset: 0,
            })
            .await
            .expect_err("missing bearer token must be rejected");
        assert_eq!(err.code(), Code::Unauthenticated);

        let err = client
            .verify_chain(VerifyChainRequest {})
            .await
            .expect_err("missing bearer token must be rejected");
        assert_eq!(err.code(), Code::Unauthenticated);
    }

    /// Seeds the daemon's real WORM audit ledger directly via
    /// `DatabaseEngine::append_worm_audit_record`, then proves
    /// `Audit/ListRecords` returns the REAL seeded records, sequence
    /// ascending, with a non-empty `record_hmac` -- never a fabricated
    /// list.
    #[tokio::test]
    async fn list_records_returns_seeded_worm_audit_records_in_sequence_order() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("connect ephemeral test db");
        db.append_worm_audit_record("system.test", "test.action.one", b"payload-one")
            .await
            .expect("append first audit record");
        db.append_worm_audit_record("system.test", "test.action.two", b"payload-two")
            .await
            .expect("append second audit record");

        let event_bus = Arc::new(EventBus::new());
        let secrets = Arc::new(SecretManager::mock());
        let filter_engine = Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set"));
        let (addr, _handle) = spawn_test_server_with(
            event_bus,
            Arc::new(db),
            filter_engine,
            secrets,
            "correct-token",
        )
        .await;

        let mut client = AuditClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");

        let response = client
            .list_records(authed_bearer_request(ListRecordsRequest {
                limit: 0,
                offset: 0,
            }))
            .await
            .expect("list_records succeeds")
            .into_inner();

        assert_eq!(response.records.len(), 2);
        assert_eq!(response.records[0].sequence, 1);
        assert_eq!(response.records[0].action, "test.action.one");
        assert_eq!(response.records[0].previous_block_hash, "GENESIS");
        assert!(!response.records[0].record_hmac.is_empty());
        assert_eq!(response.records[1].sequence, 2);
        assert_eq!(response.records[1].action, "test.action.two");
    }

    /// Proves `Audit/VerifyChain` reports `valid: true` and the real
    /// record count on an intact, honestly-signed chain -- never a
    /// fabricated `valid`.
    #[tokio::test]
    async fn verify_chain_reports_valid_true_on_an_intact_chain() {
        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("connect ephemeral test db");
        db.append_worm_audit_record("system.test", "test.action.one", b"payload-one")
            .await
            .expect("append first audit record");
        db.append_worm_audit_record("system.test", "test.action.two", b"payload-two")
            .await
            .expect("append second audit record");

        let event_bus = Arc::new(EventBus::new());
        let secrets = Arc::new(SecretManager::mock());
        let filter_engine = Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set"));
        let (addr, _handle) = spawn_test_server_with(
            event_bus,
            Arc::new(db),
            filter_engine,
            secrets,
            "correct-token",
        )
        .await;

        let mut client = AuditClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");

        let response = client
            .verify_chain(authed_bearer_request(VerifyChainRequest {}))
            .await
            .expect("verify_chain succeeds")
            .into_inner();

        assert!(response.valid);
        assert_eq!(response.record_count, 2);
        assert_eq!(response.first_broken_seq, None);
    }

    /// Proves `Audit/VerifyChain` honestly reports `valid: false` (with the
    /// exact broken sequence) when the persisted ledger was signed with a
    /// DIFFERENT WORM HMAC key than the one this engine instance was
    /// provisioned with -- the same cross-vault mismatch scenario
    /// `nuncio_store::db`'s own `worm_audit_key_is_vault_sourced_not_a_shared_default`
    /// / `verify_worm_audit_chain_report_reports_first_broken_seq_on_a_vault_key_mismatch`
    /// tests prove at the `DatabaseEngine` layer, exercised here end-to-end
    /// over the authenticated gRPC API. The ledger's `UPDATE`/`DELETE`
    /// triggers make it impossible to tamper with an existing record
    /// in-place, so a mismatched vault key is the realistic way a
    /// persisted chain becomes "invalid" without bypassing the store's own
    /// immutability guarantees.
    #[tokio::test]
    async fn verify_chain_reports_invalid_and_first_broken_seq_on_a_vault_key_mismatch() {
        let dir = tempfile::tempdir().expect("create persistent temp dir");
        let db_path = dir.path().join("audit_mismatch_test.db");

        let secrets_a = Arc::new(SecretManager::mock());
        let db_a = DatabaseEngine::connect_file(&db_path, &secrets_a)
            .await
            .expect("open db with vault a");
        db_a.append_worm_audit_record("system.test", "test.action.one", b"payload-one")
            .await
            .expect("append audit record signed with vault a's key");
        db_a.close().await;

        // "Restart" against the SAME on-disk database file, but with a
        // DIFFERENT mock vault -- so the WORM HMAC key resolved for this
        // second engine instance does not match the key that actually
        // signed the persisted record.
        let secrets_b = Arc::new(SecretManager::mock());
        let db_b = DatabaseEngine::connect_file(&db_path, &secrets_b)
            .await
            .expect("open db with vault b");

        let event_bus = Arc::new(EventBus::new());
        let filter_engine = Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set"));
        let (addr, _handle) = spawn_test_server_with(
            event_bus,
            Arc::new(db_b),
            filter_engine,
            secrets_b,
            "correct-token",
        )
        .await;

        let mut client = AuditClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");

        let response = client
            .verify_chain(authed_bearer_request(VerifyChainRequest {}))
            .await
            .expect("verify_chain succeeds (a broken chain is a normal result, not an RPC error)")
            .into_inner();

        assert!(!response.valid);
        assert_eq!(response.record_count, 1);
        assert_eq!(response.first_broken_seq, Some(1));
    }

    // ---- Calendar ----

    fn sample_calendar_event(
        id: &str,
        account_id: &str,
        calendar_id: &str,
        start: i64,
        end: i64,
    ) -> nuncio_core::model::CalendarEvent {
        nuncio_core::model::CalendarEvent {
            id: id.to_string(),
            account_id: account_id.to_string(),
            calendar_id: calendar_id.to_string(),
            summary: format!("Summary for {id}"),
            start_time: start,
            end_time: end,
            rrule: None,
            location: None,
        }
    }

    /// `ListEvents` reads only genuinely persisted events via
    /// `DatabaseEngine::list_calendar_events` -- never fabricated data.
    #[tokio::test]
    async fn list_events_returns_only_persisted_events_in_window() {
        use nuncio_proto::v1::calendar_client::CalendarClient;
        use nuncio_proto::v1::ListEventsRequest;

        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("connect ephemeral test db");
        db.save_calendar_event(&sample_calendar_event(
            "evt-in-window",
            "acct-cal-1",
            "cal-work",
            1_700_000_000,
            1_700_003_600,
        ))
        .await
        .expect("save event in window");
        db.save_calendar_event(&sample_calendar_event(
            "evt-out-of-window",
            "acct-cal-1",
            "cal-work",
            1_800_000_000,
            1_800_003_600,
        ))
        .await
        .expect("save event out of window");

        let event_bus = Arc::new(EventBus::new());
        let secrets = Arc::new(SecretManager::mock());
        let filter_engine = Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set"));
        let (addr, _handle) = spawn_test_server_with(
            event_bus,
            Arc::new(db),
            filter_engine,
            secrets,
            "correct-token",
        )
        .await;

        let mut client = CalendarClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let response = client
            .list_events(authed_bearer_request(ListEventsRequest {
                account_id: "acct-cal-1".to_string(),
                calendar_id: "cal-work".to_string(),
                start_window: 1_699_999_000,
                end_window: 1_700_004_000,
            }))
            .await
            .expect("list_events succeeds")
            .into_inner();

        assert_eq!(response.events.len(), 1);
        assert_eq!(response.events[0].id, "evt-in-window");
    }

    /// `ListEvents` filters by `calendar_id` when it's non-empty: an event that overlaps the
    /// window but belongs to a different calendar collection must not appear in the response.
    #[tokio::test]
    async fn list_events_excludes_events_from_a_different_calendar_id() {
        use nuncio_proto::v1::calendar_client::CalendarClient;
        use nuncio_proto::v1::ListEventsRequest;

        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("connect ephemeral test db");
        db.save_calendar_event(&sample_calendar_event(
            "evt-work-cal",
            "acct-cal-1",
            "cal-work",
            1_700_000_000,
            1_700_003_600,
        ))
        .await
        .expect("save work-calendar event");
        db.save_calendar_event(&sample_calendar_event(
            "evt-personal-cal",
            "acct-cal-1",
            "cal-personal",
            1_700_000_000,
            1_700_003_600,
        ))
        .await
        .expect("save personal-calendar event");

        let event_bus = Arc::new(EventBus::new());
        let secrets = Arc::new(SecretManager::mock());
        let filter_engine = Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set"));
        let (addr, _handle) = spawn_test_server_with(
            event_bus,
            Arc::new(db),
            filter_engine,
            secrets,
            "correct-token",
        )
        .await;

        let mut client = CalendarClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let response = client
            .list_events(authed_bearer_request(ListEventsRequest {
                account_id: "acct-cal-1".to_string(),
                calendar_id: "cal-work".to_string(),
                start_window: 1_699_999_000,
                end_window: 1_700_004_000,
            }))
            .await
            .expect("list_events succeeds")
            .into_inner();

        assert_eq!(response.events.len(), 1);
        assert_eq!(response.events[0].id, "evt-work-cal");
    }

    #[tokio::test]
    async fn list_events_rejects_empty_account_id() {
        use nuncio_proto::v1::calendar_client::CalendarClient;
        use nuncio_proto::v1::ListEventsRequest;

        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle, _dir) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = CalendarClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let err = client
            .list_events(authed_bearer_request(ListEventsRequest {
                account_id: String::new(),
                calendar_id: "cal-work".to_string(),
                start_window: 0,
                end_window: i64::MAX,
            }))
            .await
            .expect_err("empty account_id must be rejected");
        assert_eq!(err.code(), Code::InvalidArgument);
    }

    #[tokio::test]
    async fn get_event_returns_persisted_event() {
        use nuncio_proto::v1::calendar_client::CalendarClient;
        use nuncio_proto::v1::GetEventRequest;

        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("connect ephemeral test db");
        db.save_calendar_event(&sample_calendar_event(
            "evt-get-1",
            "acct-cal-1",
            "cal-work",
            1_700_000_000,
            1_700_003_600,
        ))
        .await
        .expect("save event");

        let event_bus = Arc::new(EventBus::new());
        let secrets = Arc::new(SecretManager::mock());
        let filter_engine = Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set"));
        let (addr, _handle) = spawn_test_server_with(
            event_bus,
            Arc::new(db),
            filter_engine,
            secrets,
            "correct-token",
        )
        .await;

        let mut client = CalendarClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let response = client
            .get_event(authed_bearer_request(GetEventRequest {
                event_id: "evt-get-1".to_string(),
            }))
            .await
            .expect("get_event succeeds")
            .into_inner();

        let event = response.event.expect("event present in response");
        assert_eq!(event.id, "evt-get-1");
        assert_eq!(event.summary, "Summary for evt-get-1");
    }

    #[tokio::test]
    async fn get_event_reports_not_found_for_unknown_event_id() {
        use nuncio_proto::v1::calendar_client::CalendarClient;
        use nuncio_proto::v1::GetEventRequest;

        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle, _dir) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = CalendarClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let err = client
            .get_event(authed_bearer_request(GetEventRequest {
                event_id: "does-not-exist".to_string(),
            }))
            .await
            .expect_err("unknown event id must be rejected");
        assert_eq!(err.code(), Code::NotFound);
    }

    /// With no `CalendarEngineOverrides::calendar_backend` injected, `Sync`
    /// returns an honest error -- there is no persisted per-account CalDAV
    /// configuration to build a real backend from -- never a fabricated
    /// `synced_count`.
    #[tokio::test]
    async fn calendar_sync_without_override_reports_honest_error() {
        use nuncio_proto::v1::calendar_client::CalendarClient;
        use nuncio_proto::v1::CalendarSyncRequest;

        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle, _dir) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = CalendarClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let err = client
            .sync(authed_bearer_request(CalendarSyncRequest {
                account_id: "acct-cal-no-config".to_string(),
                calendar_id: "cal-work".to_string(),
                start_window: 0,
                end_window: i64::MAX,
            }))
            .await
            .expect_err("sync with no injected backend and no CalDAV config must fail");
        assert_eq!(err.code(), Code::Internal);
        assert!(err.message().contains("acct-cal-no-config"));
        assert!(err.message().contains("no CalDAV configuration"));
    }

    /// With a [`CalendarEngineOverrides::calendar_backend`] injected, `Sync`
    /// genuinely fetches from it (via `calendar_sync::sync_with_backend`)
    /// and awaits full completion before returning, so the synced events are
    /// immediately visible to `ListEvents`/`GetEvent` -- exactly the
    /// mechanism a full-daemon offline E2E test uses.
    #[tokio::test]
    async fn calendar_sync_with_injected_backend_persists_and_is_immediately_visible() {
        use nuncio_proto::v1::calendar_client::CalendarClient;
        use nuncio_proto::v1::{CalendarSyncRequest, GetEventRequest, ListEventsRequest};

        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("connect ephemeral test db");
        let db = Arc::new(db);
        let secrets = Arc::new(SecretManager::mock());

        let mock_backend = nuncio_cal::MockCalendarBackend::new();
        mock_backend.add_event(sample_calendar_event(
            "evt-sync-injected-1",
            "acct-cal-injected",
            "cal-work",
            1_700_000_000,
            1_700_003_600,
        ));

        let calendar_overrides = CalendarEngineOverrides {
            calendar_backend: Some(Arc::new(mock_backend)),
        };
        let filter_engine = Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set"));
        let (addr, _handle) = spawn_test_server_with_all_overrides(
            Arc::new(EventBus::new()),
            db,
            filter_engine,
            secrets,
            "correct-token",
            MailEngineOverrides::default(),
            calendar_overrides,
            ContactsEngineOverrides::default(),
        )
        .await;

        let mut client = CalendarClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");

        let response = client
            .sync(authed_bearer_request(CalendarSyncRequest {
                account_id: "acct-cal-injected".to_string(),
                calendar_id: "cal-work".to_string(),
                start_window: 1_699_999_000,
                end_window: 1_700_004_000,
            }))
            .await
            .expect("sync succeeds")
            .into_inner();
        assert_eq!(response.synced_count, 1);

        // Immediately visible, no fixed sleep.
        let list_response = client
            .list_events(authed_bearer_request(ListEventsRequest {
                account_id: "acct-cal-injected".to_string(),
                calendar_id: "cal-work".to_string(),
                start_window: 1_699_999_000,
                end_window: 1_700_004_000,
            }))
            .await
            .expect("list_events succeeds")
            .into_inner();
        assert_eq!(list_response.events.len(), 1);
        assert_eq!(list_response.events[0].id, "evt-sync-injected-1");

        let get_response = client
            .get_event(authed_bearer_request(GetEventRequest {
                event_id: "evt-sync-injected-1".to_string(),
            }))
            .await
            .expect("get_event succeeds")
            .into_inner();
        assert_eq!(
            get_response
                .event
                .expect("event present in response")
                .summary,
            "Summary for evt-sync-injected-1"
        );
    }

    // ---- expand_events_for_window (pure, no DB/gRPC) ----

    fn recurring_event(
        id: &str,
        rrule: &str,
        start: i64,
        end: i64,
    ) -> nuncio_core::model::CalendarEvent {
        let mut event = sample_calendar_event(id, "acct-cal-1", "cal-work", start, end);
        event.rrule = Some(rrule.to_string());
        event
    }

    /// A weekly event's later occurrence (well past the stored master's own start/end) is
    /// returned when the query window covers only that later occurrence -- proving the merge
    /// helper materializes real occurrences rather than trusting the master row's own window
    /// membership.
    #[test]
    fn expand_events_for_window_finds_later_occurrence_of_a_recurring_event() {
        let master = recurring_event(
            "evt-weekly",
            "FREQ=WEEKLY;INTERVAL=1",
            1_704_067_200, // 2024-01-01T00:00:00Z
            1_704_070_800,
        );

        // Query a window 4 weeks later, which does not overlap the master's own
        // start/end at all.
        let start_window = 1_706_400_000; // 2024-01-28
        let end_window = 1_706_500_000;

        let result = expand_events_for_window(Vec::new(), vec![master], start_window, end_window);

        assert_eq!(result.len(), 1);
        assert!(result[0].start_time >= start_window && result[0].start_time <= end_window);
        assert!(result[0].id.starts_with("evt-weekly_occ_"));
    }

    /// A non-recurring event passed through `all_in_window` is returned unchanged.
    #[test]
    fn expand_events_for_window_passes_non_recurring_events_through_unchanged() {
        let event = sample_calendar_event(
            "evt-single",
            "acct-cal-1",
            "cal-work",
            1_700_000_000,
            1_700_003_600,
        );

        let result = expand_events_for_window(
            vec![event.clone()],
            Vec::new(),
            1_699_999_000,
            1_700_004_000,
        );

        assert_eq!(result, vec![event]);
    }

    /// A recurring master whose own stored start/end also overlaps the query window must
    /// never appear twice -- once as the raw row from `all_in_window` and again as an
    /// expanded occurrence.
    #[test]
    fn expand_events_for_window_never_double_counts_a_recurring_master() {
        let master = recurring_event(
            "evt-weekly",
            "FREQ=WEEKLY;INTERVAL=1",
            1_704_067_200,
            1_704_070_800,
        );

        // The master's own row also overlaps the window (as `list_calendar_events`
        // would genuinely return it), simulating the exact double-count risk.
        let result = expand_events_for_window(
            vec![master.clone()],
            vec![master],
            1_704_067_200,
            1_704_070_800,
        );

        assert_eq!(result.len(), 1);
        assert!(result[0].id.starts_with("evt-weekly_occ_"));
    }

    /// A `COUNT`/`UNTIL`-bounded recurrence produces no events for a window entirely past
    /// its bound.
    #[test]
    fn expand_events_for_window_honors_until_bound() {
        let master = recurring_event(
            "evt-bounded",
            "FREQ=WEEKLY;INTERVAL=1;UNTIL=20240115T000000Z",
            1_704_067_200,
            1_704_070_800,
        );

        let result = expand_events_for_window(
            Vec::new(),
            vec![master],
            1_709_251_200, // 2024-03-01, well past the UNTIL bound
            1_709_337_600,
        );

        assert!(result.is_empty());
    }

    /// An event with an unparseable `rrule` degrades to the raw stored master rather than
    /// vanishing or failing the whole merge.
    #[test]
    fn expand_events_for_window_degrades_unparseable_rrule_to_raw_master() {
        let master = recurring_event(
            "evt-bad-rrule",
            "NOT_A_VALID_RRULE",
            1_700_000_000,
            1_700_003_600,
        );

        let result = expand_events_for_window(
            Vec::new(),
            vec![master.clone()],
            1_699_999_000,
            1_700_004_000,
        );

        assert_eq!(result, vec![master]);
    }

    fn sample_contact(id: &str, account_id: &str, display_name: &str) -> nuncio_contacts::Contact {
        let mut contact =
            nuncio_contacts::Contact::new(display_name, format!("{display_name}@nuncio.mx"));
        contact.id = id.to_string();
        contact.account_id = Some(account_id.to_string());
        contact
    }

    /// Confirms `Contacts` is mounted behind its own `BearerAuthInterceptor`
    /// exactly like every other service (no un-intercepted service).
    #[tokio::test]
    async fn contacts_rpcs_reject_missing_bearer_token() {
        use nuncio_proto::v1::contacts_client::ContactsClient;
        use nuncio_proto::v1::{ContactsSyncRequest, GetContactRequest, ListContactsRequest};

        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle, _dir) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = ContactsClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");

        let err = client
            .list_contacts(ListContactsRequest {
                account_id: "acct-1".to_string(),
            })
            .await
            .expect_err("missing bearer token must be rejected");
        assert_eq!(err.code(), Code::Unauthenticated);

        let err = client
            .get_contact(GetContactRequest {
                contact_id: "ct-1".to_string(),
            })
            .await
            .expect_err("missing bearer token must be rejected");
        assert_eq!(err.code(), Code::Unauthenticated);

        let err = client
            .sync(ContactsSyncRequest {
                account_id: "acct-1".to_string(),
            })
            .await
            .expect_err("missing bearer token must be rejected");
        assert_eq!(err.code(), Code::Unauthenticated);
    }

    #[tokio::test]
    async fn list_contacts_returns_only_genuinely_persisted_contacts() {
        use nuncio_proto::v1::contacts_client::ContactsClient;
        use nuncio_proto::v1::ListContactsRequest;

        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("connect ephemeral test db");
        db.save_contact(&sample_contact("ct-list-1", "acct-contacts-list", "Alice"))
            .await
            .expect("save contact");
        db.save_contact(&sample_contact("ct-list-2", "acct-contacts-list", "Bob"))
            .await
            .expect("save contact");
        db.save_contact(&sample_contact("ct-list-other", "acct-other", "Carol"))
            .await
            .expect("save contact");

        let secrets = Arc::new(SecretManager::mock());
        let event_bus = Arc::new(EventBus::new());
        let filter_engine = Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set"));
        let (addr, _handle) = spawn_test_server_with(
            event_bus,
            Arc::new(db),
            filter_engine,
            secrets,
            "correct-token",
        )
        .await;

        let mut client = ContactsClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let response = client
            .list_contacts(authed_bearer_request(ListContactsRequest {
                account_id: "acct-contacts-list".to_string(),
            }))
            .await
            .expect("list_contacts succeeds")
            .into_inner();

        assert_eq!(response.contacts.len(), 2);
        let ids: Vec<&str> = response.contacts.iter().map(|c| c.id.as_str()).collect();
        assert!(ids.contains(&"ct-list-1"));
        assert!(ids.contains(&"ct-list-2"));
        assert!(!ids.contains(&"ct-list-other"));
    }

    #[tokio::test]
    async fn get_contact_reports_not_found_for_unknown_contact_id() {
        use nuncio_proto::v1::contacts_client::ContactsClient;
        use nuncio_proto::v1::GetContactRequest;

        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle, _dir) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = ContactsClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let err = client
            .get_contact(authed_bearer_request(GetContactRequest {
                contact_id: "does-not-exist".to_string(),
            }))
            .await
            .expect_err("unknown contact id must be rejected");
        assert_eq!(err.code(), Code::NotFound);
    }

    /// With no `ContactsEngineOverrides::contacts_backend` injected, `Sync`
    /// returns an honest error -- there is no persisted per-account CardDAV
    /// configuration to build a real backend from -- never a fabricated
    /// `synced_count`.
    #[tokio::test]
    async fn contacts_sync_without_override_reports_honest_error() {
        use nuncio_proto::v1::contacts_client::ContactsClient;
        use nuncio_proto::v1::ContactsSyncRequest;

        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle, _dir) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = ContactsClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");
        let err = client
            .sync(authed_bearer_request(ContactsSyncRequest {
                account_id: "acct-contacts-no-config".to_string(),
            }))
            .await
            .expect_err("sync with no injected backend and no CardDAV config must fail");
        assert_eq!(err.code(), Code::Internal);
        assert!(err.message().contains("acct-contacts-no-config"));
        assert!(err.message().contains("no CardDAV configuration"));
    }

    /// With a [`ContactsEngineOverrides::contacts_backend`] injected, `Sync`
    /// genuinely fetches from it (via `contacts_sync::sync_with_backend`)
    /// and awaits full completion before returning, so the synced contacts
    /// are immediately visible to `ListContacts`/`GetContact` -- exactly the
    /// mechanism a full-daemon offline E2E test uses.
    #[tokio::test]
    async fn contacts_sync_with_injected_backend_persists_and_is_immediately_visible() {
        use nuncio_proto::v1::contacts_client::ContactsClient;
        use nuncio_proto::v1::{ContactsSyncRequest, GetContactRequest, ListContactsRequest};

        let (db, _dir) = DatabaseEngine::connect_ephemeral()
            .await
            .expect("connect ephemeral test db");
        let db = Arc::new(db);
        let secrets = Arc::new(SecretManager::mock());

        let mock_backend = nuncio_contacts::MockContactsBackend::new();
        mock_backend.add_contact(sample_contact(
            "ct-sync-injected-1",
            "acct-contacts-injected",
            "Injected Contact",
        ));

        let contacts_overrides = ContactsEngineOverrides {
            contacts_backend: Some(Arc::new(mock_backend)),
        };
        let filter_engine = Arc::new(FilterEngine::new(Vec::new()).expect("empty rule set"));
        let (addr, _handle) = spawn_test_server_with_all_overrides(
            Arc::new(EventBus::new()),
            db,
            filter_engine,
            secrets,
            "correct-token",
            MailEngineOverrides::default(),
            CalendarEngineOverrides::default(),
            contacts_overrides,
        )
        .await;

        let mut client = ContactsClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");

        let sync_response = client
            .sync(authed_bearer_request(ContactsSyncRequest {
                account_id: "acct-contacts-injected".to_string(),
            }))
            .await
            .expect("sync succeeds")
            .into_inner();
        assert_eq!(sync_response.synced_count, 1);

        let list_response = client
            .list_contacts(authed_bearer_request(ListContactsRequest {
                account_id: "acct-contacts-injected".to_string(),
            }))
            .await
            .expect("list_contacts succeeds")
            .into_inner();
        assert_eq!(list_response.contacts.len(), 1);
        assert_eq!(list_response.contacts[0].id, "ct-sync-injected-1");

        let get_response = client
            .get_contact(authed_bearer_request(GetContactRequest {
                contact_id: "ct-sync-injected-1".to_string(),
            }))
            .await
            .expect("get_contact succeeds")
            .into_inner();
        let contact = get_response.contact.expect("contact present in response");
        assert_eq!(contact.display_name, "Injected Contact");
    }

    #[tokio::test]
    async fn create_contact_persists_and_is_immediately_visible() {
        use nuncio_proto::v1::contacts_client::ContactsClient;
        use nuncio_proto::v1::{
            ContactEmail as ContactEmailProto, CreateContactRequest, GetContactRequest,
        };

        let event_bus = Arc::new(EventBus::new());
        let (addr, _handle, _dir) = spawn_test_server(event_bus, "correct-token").await;

        let mut client = ContactsClient::connect(format!("http://{addr}"))
            .await
            .expect("client connects");

        let create_response = client
            .create_contact(authed_bearer_request(CreateContactRequest {
                account_id: "acct-contacts-create".to_string(),
                display_name: "New Contact".to_string(),
                organization: Some("Kof22".to_string()),
                emails: vec![ContactEmailProto {
                    email: "new.contact@kof22.com".to_string(),
                    label: "work".to_string(),
                    is_primary: true,
                }],
                phones: vec![],
            }))
            .await
            .expect("create_contact succeeds")
            .into_inner();
        let created = create_response
            .contact
            .expect("created contact present in response");
        assert_eq!(created.display_name, "New Contact");
        assert_eq!(created.account_id.as_deref(), Some("acct-contacts-create"));
        assert_eq!(created.emails.len(), 1);

        let get_response = client
            .get_contact(authed_bearer_request(GetContactRequest {
                contact_id: created.id.clone(),
            }))
            .await
            .expect("get_contact succeeds")
            .into_inner();
        let fetched = get_response.contact.expect("contact present in response");
        assert_eq!(fetched.display_name, "New Contact");
        assert_eq!(fetched.organization.as_deref(), Some("Kof22"));
    }
}
