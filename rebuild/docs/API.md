# Local API contract

Nuncio's engine is accessed through the `nuncio.v2` gRPC contract. The daemon owns
storage, credentials, synchronization and durable operations. The CLI is a client
of generated protobuf types; other clients should use the same contract and keep
engine/storage dependencies out of their applications.

The source of truth is [`crates/nuncio-proto/proto/nuncio/v2`](../crates/nuncio-proto/proto/nuncio/v2).
`nuncio-proto::DESCRIPTOR` embeds the generated six-file descriptor set. The
current source has 49 RPCs, including seven additive account-management methods;
the previous `6ff9bb9` package has 42. The current `164b021` package
and full offline gate passed, including previous-wire compatibility and all
196 invalid-auth cases. Current-source hosted verification also passed at164b021. The independent [generated-client smoke](../clients/smoke/README.md)
has its own workspace/lockfile and no engine or Nuncio client-library dependency.
Its baseline local and hosted checks passed; see [VERIFICATION.md](VERIFICATION.md).
The [API publication/SemVer plan](API-PUBLICATION-PLAN.md) is a proposal: no
standalone API release, generated reference site, registry, or new public endpoint
has been published.

## Connection and identity

The normal daemon listens on `http://127.0.0.1:9421` by default. Both server and
reference client require numeric loopback addresses; the client rejects remote
hosts, credentials in the URL, query strings and non-root paths. An explicit
profile/data directory selects a separate encrypted store and its authorization.
Every RPC requires the profile's authorization metadata, including status,
shutdown and streaming methods. Obtain authorization through the profile's secure
keystore setup, never an argument containing a token. [RUNNING.md](RUNNING.md)
covers first-run configuration and the separate synthetic test profile.

Use the opaque local account, message, calendar, event, draft and operation IDs
returned by the API. Provider IDs are separate fields, scoped by provider and
account. An address is not an account identity. Page tokens are opaque and belong
to the query that produced them; synchronization cursors are a different type of
provider state. A changed query snapshot requires restarting pagination.

## Services

| Service | Responsibility | Contract |
|---|---|---|
| System | Health/status, shutdown, mail sync runs, cancellation and committed-change stream | [system.proto](../crates/nuncio-proto/proto/nuncio/v2/system.proto) |
| Accounts | Google OAuth begin/status/cancel, IMAP/SMTP connection and updates, local details/name/version, credential checks/disconnect, pause/resume/archive/restore, and previewed local purge | [accounts.proto](../crates/nuncio-proto/proto/nuncio/v2/accounts.proto) |
| Mail | Cached mail/search, original byte streams, drafts/attachments and queued mail changes/send | [mail.proto](../crates/nuncio-proto/proto/nuncio/v2/mail.proto) |
| Calendar | Calendar catalog, agenda/events, refresh, free/busy and queued event changes | [calendar.proto](../crates/nuncio-proto/proto/nuncio/v2/calendar.proto) |
| Operations | Durable operation state, attempts/receipts, cancellation, explicit resolution and reconciliation | [operations.proto](../crates/nuncio-proto/proto/nuncio/v2/operations.proto) |
| Maintenance | Streamed encrypted backups, inspection, new-profile restore and projection repair | [maintenance.proto](../crates/nuncio-proto/proto/nuncio/v2/maintenance.proto) |

Account records distinguish lifecycle (`state`) from credential status
(`auth_state`). `ListAccounts` omits archived accounts unless `include_archived`
is true; `GetAccount` can still inspect a retained archived ID. Name/configuration
edits require the saved account version. Archive retains downloaded data and
durable records while removing credentials; restore requires separate
reauthentication. `PreviewAccountPurge` supplies the version and profile revision
required by `PurgeAccount`, along with an exact account-ID confirmation. Purge
requires an archived account and no unresolved remote outcome; it deletes that
account's local data only. See [account management](ACCOUNT-MANAGEMENT.md) for
CLI examples, interrupted cleanup, and lifecycle limits.

## Durable work and observations

An accepted mutation normally returns queued durable intent. Inspect the operation
until it reaches a successful terminal state; an enqueue response alone does not
prove a provider effect. Reuse the same account-scoped request UUID and identical
payload to recover an enqueue acknowledgement. Changing the payload under that
UUID is a conflict. Expected versions and Calendar ETags protect against stale
edits. Do not generate a new send UUID merely because a request timed out.

`uncertain` means a remote action may have happened. Provider reads, positive
receipts and explicit user decisions determine safe continuation. SMTP delivery,
Sent-folder copies and Calendar notification effects are tracked independently.
The recovery procedures and their limits are in [RECOVERY.md](RECOVERY.md).

A stopped CLI or expired client deadline does not cancel an already queued daemon
operation. Use the explicit cancellation method where supported. `WatchChanges`
reports committed revisions; it is not an exactly-once delivery service. A revision
outside retained history requires a new local snapshot before resubscribing.

Cached reads remain available offline, with coverage/body availability reported
explicitly. Missing or oversized bodies cannot become successful empty downloads.
HTML is inert data; clients must provide their own rendering sandbox. Explicit
original-byte downloads preserve content and are separate from terminal-safe text.

Status resource counters are process-local diagnostics and reset on restart.
Their scopes and queue/byte/page semantics are documented in
[Resource status](RUNNING.md#resource-status). They do not replace durable receipts.

## CLI representation

The CLI's versioned JSON output and versioned action-file schemas are separate
from the protobuf package version. See [RUNNING.md](RUNNING.md) for command and
payload examples, JSON/streaming conventions, binary output and exit statuses.
Secrets use private files or standard input as documented. Automated examples and
acceptance tests use the independent local providers; live acceptance remains a
separate, explicitly authorized procedure in [MANUAL-ACCEPTANCE.md](MANUAL-ACCEPTANCE.md).

## Reviewed v2 contract baseline

`crates/nuncio-proto/proto/nuncio.v2.bin` is the reviewed local descriptor baseline,
including field numbers/types, reservations, RPC names and streaming modes.
`cargo test --locked -p nuncio-proto --test contract` compares the generated
descriptor with that checked-in baseline. The exact freeze intentionally rejects
additions too: review any contract change before regenerating it. Never reuse a
field number, change a field wire type, or silently replace a released RPC; reserve
removed fields and introduce a new API version for incompatible behavior.
The local package verifies its freshly built descriptor against the same freeze.
The retained `nuncio.v2.pre-account-management.bin` separately checks that prior
fields and RPCs survive the account extension; it is not a second current API.
The package name `nuncio.v2`, application Cargo version `0.1.0`, database schema
23, and CLI JSON schema version 1 identify different contracts. An independent
API artifact SemVer and broader compatibility gate are proposed in the
[publication plan](API-PUBLICATION-PLAN.md), not implemented by this freeze.

The independent [smoke client](../clients/smoke/README.md) generates its own status/
watch bindings in a separate Cargo workspace. Its normal/build dependency tree
contains no engine, storage, daemon, mock, CLI, or Nuncio client-helper dependency.
Its subprocess test authenticates, reads status and a replayed change, rejects a
bad token, and independently checks that no send/notification occurred.
