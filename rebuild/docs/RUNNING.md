# Running the rebuild

This workspace currently provides daemon lifecycle, Google account connection, Gmail/Calendar synchronization, local mail queries/search, original-content downloads, local drafts with attachments/reply/forward, and durable Google send and label mutations with operation history and resolution. Google Calendar writes and free/busy, Synology mail reads/flags, explicit folder transfers, and SMTP with client/server Sent policies are verified against local independent services. Encrypted backup creation/inspection/new-profile restore are implemented; broader recovery and resilience/release work remain in progress. These are local development artifacts, not a finished release or a claim of live compatibility. All acceptance so far uses synthetic local providers. Google and Synology live access is deferred by the user's instruction.

## Offline development

Use `python3 rebuild/scripts/verify.py --suite google_e2e` from the isolated worktree root. It builds `rebuild/target/test-harness/debug/nunciod` and `nuncio-cli`, runs the stateful mock, creates a private profile and synthetic keystore, and exercises actual processes. See [TESTING.md](TESTING.md) for system/conformance suites and failure artifacts. No real OAuth registration or account is needed.

For interactive mock work, start the unshipped `mock-google --ready-file FILE` binary and use its announced numeric loopback URL. Keep its stdin open; JSON controls and shutdown are documented in [MOCK-GOOGLE.md](MOCK-GOOGLE.md). Start the test daemon with a new data directory, `--bind 127.0.0.1:0 --ready-file FILE --test-secrets-file FILE --test-config FILE`. The test configuration's `google_base_url` must name that local mock. Each CLI invocation uses the announced endpoint, same data directory and synthetic secret file. None of these test flags exists in a production build.

The mock registration file is `{"installed":{"client_id":"nuncio-test-client"}}`, mode 0600. The optional test client secret is exactly `synthetic-client-secret`. Run `account connect-google --client-config FILE --login-hint alpha@example.test --no-browser`. It returns a session ID, browser URL, and expiry. Visit that local URL to complete the mock redirect, then inspect `account auth-status --session ID`. Repeat for `beta@example.test`. Commands:

```text
account list
account check --account ID
account disconnect --account ID
account connect-google --client-config FILE --account ID --no-browser
system status
system shutdown
sync --account ID --wait
system sync-status --account ID --run RUN_ID
system cancel-sync --account ID --run RUN_ID
mail collections --account ID
mail list --account ID --page-size 100
mail search --account ID --query searchable
mail read --account ID --message MESSAGE_ID
mail raw --account ID --message MESSAGE_ID --output NEW_FILE.eml
mail attachment --account ID --message MESSAGE_ID --attachment ATTACHMENT_ID --output NEW_FILE.pdf
mail body --account ID --message MESSAGE_ID --kind html --output NEW_FILE.html
mail fetch --account ID --message MESSAGE_ID --wait
```

Account list is local. Check explicitly contacts the configured provider to validate the credential and saved stable identity. Disconnect durably pauses the account, deletes its credential reference/secret, and retains its cached data and durable records. It does not call Google's grant-revocation endpoint. Reconnect checks Google's stable `sub` identifier; an email address alone is insufficient. Choosing another Google account for an existing ID fails. A normal connect to an already-known subject reuses its local ID.

## Production preparation for later authorized acceptance

The normal daemon uses a separate profile under `~/.nuncio-rebuild/<profile>`, SQLCipher, and the OS keychain service `mx.nuncio.rebuild`. It binds `127.0.0.1:9421` by default. Google endpoints are fixed HTTPS endpoints; redirects and ambient proxy configuration are disabled for provider HTTP. The only plaintext HTTP paths are authenticated local RPC and the short-lived loopback OAuth callback. Do not point normal tests at live accounts.

When live acceptance is separately authorized, prepare a Google Cloud project with Gmail and Calendar APIs enabled, a consent configuration, and an OAuth client of type **Desktop app**. Save the downloaded `installed` client registration JSON outside the repository, mode 0600, and supply its path with `--client-config`. The client ID and optional client secret travel in the authenticated RPC body, never shell arguments or log output. Refresh credentials are stored only in the OS keystore under profile/account-scoped references. OAuth uses the system browser, random state, and PKCE S256 with a numeric loopback redirect; `--no-browser` returns the URL for manual opening. [Google installed-app OAuth](https://developers.google.com/identity/protocols/oauth2/native-app).

The requested scopes are `openid email`, `gmail.modify`, and `calendar` (the latter two use their full Google scope URLs). Required permissions must all be granted; partial consent returns an inspectable scope-denied status and does not create a partially usable account. Google's canonical `userinfo.email` scope spelling is accepted as equivalent to `email`. Stable identity comes from the HTTPS userinfo endpoint, not an unverified ID-token payload or the selected login hint. [Google OpenID Connect](https://developers.google.com/identity/openid-connect/openid-connect).

For a consent configuration with External user type and Testing publishing status, add the disposable account as a test user. Google documents a seven-day refresh-token lifetime for Testing applications requesting scopes beyond basic identity, so reauthorization can be expected during manual acceptance. Organizational policy can also prevent consent. No configuration has been created or changed by this rebuild. [Google OAuth token expiration](https://developers.google.com/identity/protocols/oauth2#expiration).

## Account recovery

Mail list/read/search/download commands read the local encrypted projection, including while the provider is stopped. HTML is returned as inert text; the CLI never opens it. Downloads use a new destination path, preserve exact bytes, and verify length/SHA-256 before publishing the file. They never overwrite existing files. `mail read` identifies missing, too-large, or unparsed content; `mail fetch` explicitly requests another download. The default per-payload cap is 64 MiB; API chunks are at most 256 KiB. Query responses include revision/coverage and an optional next-page token; reuse the same query and page size with `--page-token`. A changed revision requires restarting pagination.

`sync` returns the durable run receipt immediately; `--wait` polls up to five minutes and exits successfully only after the run succeeds. Inspect a run with `system sync-status`. `--full` requests authoritative reconciliation. Full scans capture history before listing and catch up changes before transactional promotion. Failed or interrupted scans leave the last complete projection/cursor visible. Startup marks interrupted read-only syncs failed, retains the complete projection/cursor, and schedules another attempt. Use an explicit sync to request recovery sooner; it still honors provider retry deadlines. Gmail history expiry triggers a fresh staged scan, with one bounded restart if history expires during that scan. [Google synchronization contract](https://developers.google.com/workspace/gmail/api/guides/sync).

Access-token refresh is serialized per account. A returned rotated refresh token is saved before using the new access token. A revoked or unusable credential moves only that account to `needs_auth`; reconnect it with its saved account ID. A lost acknowledgement during refresh-token rotation can require fresh consent because the provider may have invalidated the old refresh token. No successful reconnection is fabricated from a local cache.

Credential replacement uses a durable SQLCipher cleanup intent before the keystore write, then atomically publishes the new reference. Disconnect first persists the paused state and cleanup intent. If secret removal fails, the command reports failure but the account remains paused; retry disconnect or restart after the keystore becomes available. Startup completes pending cleanup without deleting account data. A crash during browser consent loses the in-memory authorization session: inspect account list after restart and start a new consent flow if it did not finish. Sessions admit at most 16 pending flows, retain at most 64 recent statuses, and expire after five minutes.

Keep original profiles untouched. If the database key is missing, SQLCipher rejects the file, or the schema is newer than the binary, startup fails without replacing the data. For encrypted backup creation, inspection, new-profile restore and `repair --scope mail|calendar` with a local `--dry-run` preview, follow [RECOVERY.md](RECOVERY.md). Broader recovery and release checks remain in progress.

When credential replacement has committed but deletion of the previous secret fails, auth status reports success with `warning_code=credential_cleanup_pending`. The new account is usable; the obsolete secret remains queued for cleanup on restart. This differs from a failed initial credential write, which creates no connected account. Profiles currently admit up to 100 connected or retained accounts.

## Local drafts and attachments

`mail draft save --account ACCOUNT --file draft.json` creates a durable draft. The JSON fields are `to`, `cc`, `bcc` (arrays of `{ "address": "person@example.test", "name": "Optional name" }`), `subject`, and optional `text`/`html`. The explicit account supplies the eventual sending identity. A draft can be incomplete; saving it never sends mail. Use `mail draft list`, `show --draft ID`, or `delete --draft ID --version VERSION`, always with `--account`. To edit, add `--draft ID --version VERSION` to save. A stale version exits 5; reread before editing.

`mail draft attach --account ACCOUNT --draft ID --version VERSION --file attachment.pdf --mime-type application/pdf` hashes a regular local file and streams it to encrypted staging. Optional `--charset windows-1252` preserves a text attachment's charset; `--inline --content-id logo@example.test` marks an inline part. The default MIME type is application/octet-stream. Limits: 256 KiB chunks, 64 MiB aggregate attachment bytes, 256 attachments per draft, four uploads per profile and 30 seconds per upload RPC. Failed/incomplete uploads leave the draft unchanged. Restart removes staging abandoned by process death. After a lost acknowledgement, reread the draft version and attachment hash before trying again.

`mail reply` and `mail reply-all` take `--account ACCOUNT --message MESSAGE --body-file UTF8_FILE`. They create editable drafts with reply headers/context; reply-all deduplicates visible recipient arrays and excludes the selected account's verified address. Additional aliases are not inferred. `mail forward --account ACCOUNT --message MESSAGE --to ADDRESS` accepts repeated `--to` and optional `--body-file`. It preserves original plain/HTML bodies, transfer-decoded attachments, MIME type parameters and inline Content-ID/disposition. Forwarding starts a new thread. These commands use downloaded originals and work offline; an unavailable original returns an offline miss. Header injection, invalid recipients and drafts exceeding 1 MiB combined text/HTML are rejected. Preparation never transmits mail. Use the durable sending commands below when ready to submit a draft.

## Durable sending and operation recovery

`mail send --account ACCOUNT --draft DRAFT --request-id UUID [--version VERSION]` freezes the current draft and atomically queues its immutable MIME. `--version` requires that draft version on first enqueue. Retry the same command and UUID after a lost local response: it returns the original operation even if the draft was edited or deleted. A changed request under that UUID is rejected. Keep the operation ID. Queued work for a disconnected account waits for reconnection.

Add `--wait` to wait up to 30 seconds, or later use `operation wait --account ACCOUNT --operation ID`. Success means a recorded Google submission acknowledgement or matching positive Sent evidence. It does not establish final recipient delivery; Google's sending pipeline can fail after a successful API response. [Gmail error and sending-limit guidance](https://developers.google.com/workspace/gmail/api/guides/handle-errors).

Inspect `operation show`, `operation list`, and `operation attempts`, always with `--account`; show/attempts take `--operation`. Attempts are newest first, with receipt source, provider ID and observation timestamp. List/history pages are bound to account, query, page size and global revision. Restart pagination after a revision conflict. `operation show` includes frozen sender, recipients, Message-ID and draft version in desired_state_json, plus audited resolutions. Wait failures retain the last operation under JSON `error.operation`; a timeout or stopped CLI leaves the daemon's durable work running.

`operation cancel` succeeds only while work is queued and undispatched. An interrupted running send becomes uncertain and is reconciled without resending. Automatic reconciliation searches the selected account's Sent messages for the frozen Message-ID, verifies sender/recipient/subject/MIME headers and exact body bytes, and requires a unique match. Provider MIME normalization or missing/ambiguous evidence can leave the result uncertain. Absence never proves non-delivery. Automatic request retries are disabled in the HTTP library; the journal owns each attempt. Positive rejection permits bounded retries with backoff and persisted provider guidance. A post-submission server failure or lost/malformed acknowledgement enters reconciliation. [Gmail message/search contract](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages).

Resolve uncertain work with `operation resolve --account ACCOUNT --operation ID --version VERSION --file decision.json`. The file is a regular JSON file of at most 16 KiB. Keep the exact decision and expected version when retrying after a lost response; the audit and any replacement request are idempotent. Decisions:

- `{"decision":"abandon","reason":"Keep the delivery outcome unresolved"}` stops further automatic work while retaining uncertainty.
- `{"decision":"confirm_applied","evidence":{"provider_id":"OBSERVED_GOOGLE_ID","message_id":"FROZEN_RFC_MESSAGE_ID","observed_at_ms":1772895600000,"note":"How I positively established acceptance"}}` records the explicit human observation as manual_confirmed. It never rewrites provider attempts. Google sends use this form. SMTP sends also require the separate acceptance evidence described below. Confirmation applies to sends; other operation kinds use their own conflict handling.
- `{"decision":"resend","request_id":"NEW_UUID","reason":"Why another submission is intended"}` also requires CLI `--accept-duplicate-risk`. It warns about possible duplicate delivery, leaves the original uncertain, and atomically links a new queued operation. Only the outer Message-ID/Date change; the frozen body, attachments and recipients remain intact. Use `operation wait` on the returned replacement_id.

The worker shares account sequencing and the global two-account limit with synchronization, bounds MIME preparation with one shared composition slot, and participates in daemon shutdown. `system status` exposes operation_worker_error if it stops unexpectedly. SMTP acceptance and Sent-copy journal substeps exist, but the Synology transport is still being implemented. All automated sending above is verified against local synthetic providers; no live account actions have been authorized.

## Calendar reads and refresh

`calendar refresh --account ACCOUNT --wait` synchronizes canonical objects and the rolling agenda (30 days before today through 180 days after, inclusive). Use `--from YYYY-MM-DD --to YYYY-MM-DD` to refresh a different window; `--to` is exclusive and windows are limited to 366 days. Refresh returns a durable sync run, inspectable/cancellable using the System commands. `calendar list --account ACCOUNT` discovers local calendar IDs. `calendar agenda --account ACCOUNT --from YYYY-MM-DD --to YYYY-MM-DD` reads cached occurrences; `calendar get --account ACCOUNT --calendar CALENDAR --event EVENT` reads a cached canonical event when available, otherwise its expanded instance. An instance's `recurring_event_id` links to the local recurring master. List/agenda support `--page-size` and `--page-token`; a changed revision requires restarting pagination.

All-day values remain dates with exclusive ends; timed values retain RFC3339 offsets and effective IANA zones. Raw provider JSON preserves recurrence, attendees, reminders, and unknown fields. Read commands make no provider requests. Inspect coverage: current is current at its recorded refresh, stale indicates a newer canonical version, out_of_window means the requested dates are not covered, unavailable means no occurrence refresh exists, and retired identifies a removed calendar. A failed canonical reset retains complete prior objects/cursor; a failed occurrence refresh exposes old occurrences as stale. Background refresh and Calendar writes are implemented.

## Change notifications

`system watch --after REVISION` emits one versioned JSON object per line, including revision, change kind, account, and optional resource ID. Save the last revision and resume from it. `--limit N` exits successfully after N changes; Ctrl-C also stops the watch. Daemon disconnect exits 4. A future or expired revision exits 5 with resnapshot_required: read a fresh snapshot and resume at its revision. The engine retains 10,000 committed changes. Progress shares the global revision sequence, so a revision change can describe a sync run without changing the visible mail projection. Native clients should inspect kind/resource ID and re-query as needed.


## Background synchronization and diagnostics

Connected Google accounts poll Gmail and Calendar independently every 60 seconds after successful completion. The scheduler runs at most two accounts concurrently, with one conflicting sequence per account. Repeated equivalent sync requests return the active run; a different active request returns a conflict. Cancel a run with `system cancel-sync --account ACCOUNT --run RUN`. Cancellation applies to that run; background polling can start another later. Disconnect an account to pause ongoing provider access until reconnecting.

Transient failures use exponential backoff with jitter, bounded at five minutes unless the provider requests a longer delay. Numeric and HTTP-date Retry-After values are persisted; restarting the daemon or issuing an explicit sync does not bypass them. Gmail/Calendar API deadlines are scoped separately; OAuth throttling covers both because credentials are shared. Revoked credentials pause the account and require reconnecting. Monotonic scheduling reconciles overdue wall deadlines after wake and retries connection failures without discarding cached data. Very large valid provider delays remain visible rather than overflowing or being shortened. [HTTP Retry-After semantics](https://www.rfc-editor.org/rfc/rfc9110.html#name-retry-after).

`system status` includes a `sync` entry per account/scope: phase, run ID, processed count, last success, age, next scheduled attempt, error code, and coverage freshness. An active run is queued/running; inactive entries are waiting, backoff, or paused. `manual` appears only when an isolated test disables automatic scheduling. Coverage is unavailable before the first complete sync and stale after errors or two missed polling intervals; inspect Mail/Calendar query coverage for payload availability and the precise agenda window. A scheduler storage failure is visible as `scheduler_error`.

`sync --wait` and `calendar refresh --wait` exit successfully only on a completed run. A failed wait, timeout, or RPC disconnection includes the last known `error.sync_run` receipt in JSON; use its account/id with `system sync-status` to inspect the durable result. A timeout or stopped CLI does not cancel the daemon's work. Plain `sync` returns a queue receipt, whose state is distinct from success.

## Resource status

`system status` includes a process-local `resources` object. Counters contain only
numbers and reset when the daemon restarts; they are diagnostics, not durable
operation receipts. The existing `sync` and `operation` results remain authoritative
for completed work. A status snapshot can span concurrent changes in these counters.

| Field | Meaning |
|---|---|
| requests_active / requests_waiting | Provider exchanges holding one of two network slots / admitted exchanges waiting for a slot. At most64 exchanges may be active or waiting. |
| requests_peak / requests_started | Highest observed simultaneous exchanges / cumulative exchanges started by this process. |
| request_limit | Two globally shared provider-network slots. |
| bytes_received | HTTP response-body bytes and decrypted IMAP/SMTP protocol bytes read, including partial or rejected responses; excludes TLS framing and outbound bytes. |
| background_jobs / background_job_limit | Spawned mail/calendar/operation jobs retained, including retry waits; global limit64. Idle connections do not consume a network slot. |
| account_requests / account_request_limit | Active or waiting account-sequenced requests; limit64, with at most two active account sequences. |
| store_queue_depth / store_queue_limit | Requests currently buffered for the SQLCipher worker / buffer capacity64; excludes the request executing on that worker and callers awaiting channel capacity. |
| storage_page_batches | Completed Gmail message/history pages, Calendar catalog/event pages, and IMAP UID batches staged in storage. Staging is not final projection promotion. |

A Google exchange is one HTTP request through its consumed response body. IMAP
counts its initial handshake/authentication and each tagged command; SMTP counts
connection/TLS handshakes, command/reply exchanges and DATA transmission/final reply.
Idle SMTP readiness and an idle IMAP connection do not reserve slots: a server-Sent
pre-DATA check can therefore run without waiting on its own SMTP connection.
Admission refusal before an operation attempt leaves durable intent queued for a
later worker tick. Explicit sync requests can be refused before creating a run
when background-job admission is full; cancel or await existing work, then retry.

## Gmail message changes

Use `mail capabilities --account ID` to inspect supported actions. Google exposes label membership; arbitrary folder move/copy and permanent purge are not available. Obtain local message IDs with `mail list` and local user-label IDs with `mail collections`.

`mail change --account ID --message MESSAGE_ID --request-id UUID --file ACTION.json --wait` commits a durable operation before network access. Retrying the same UUID and action returns that original operation; changing its payload is a conflict. Omit --wait to return after enqueue. `operation show/list/attempts/cancel/resolve` also work for these operations; abandon can release a conflict that is blocking later changes to that message. Manual send confirmation/resend is not a label-mutation action.

Every action file requires `schema_version: 1`; missing or unknown versions and unknown fields are rejected before the mutation RPC. Use one of these typed forms:

```json
{"schema_version":1,"action":"read","read":true}
{"schema_version":1,"action":"star","starred":false}
{"schema_version":1,"action":"archive"}
{"schema_version":1,"action":"trash","trashed":true}
{"schema_version":1,"action":"trash","trashed":false}
{"schema_version":1,"action":"label","collection_id":"LOCAL_USER_LABEL_ID","present":true}
```

These are separate example files, not one JSON document. The action file is limited to 8 KiB. False reverses read/star/label membership; archive removes INBOX. Trash and restore call the specific Gmail endpoints. Restore uses the provider's returned memberships instead of guessing where the message should appear. Existing-label actions accept active user labels from the chosen account; system labels use their dedicated typed actions. Unrelated labels are preserved. [Gmail modify](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages/modify), [trash](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages/trash), [untrash](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages/untrash).

Queued desired state is visible in `operation show`; normal message queries continue to show observed labels. A validated acknowledgement or positive reconciliation commits the complete observed memberships and operation receipt in the same transaction. Labels discovered in that response retain their provider IDs even before the next catalog refresh. Mutation receipts do not advance the mailbox sync cursor. A deleted/recreated local projection does not delete the durable provider identity in the intent.

Pending changes to one provider message execute in insertion order, including equal timestamps and delayed retries. This prevents a later inverse action from being undone by recovery of an older action. Crashed work first reads the provider: matching desired labels establish success; otherwise these explicit label states can be retried within the bounded journal policy. Missing remote messages become conflicts. Repeated uncertainty eventually requires inspection; sending remains subject to the stricter no-blind-resend policy.

## Calendar changes

Get local calendar/event IDs and the current ETag with `calendar list`, `calendar agenda` and `calendar get`. Submit an action with `calendar change --account ACCOUNT --calendar CALENDAR --request-id UUID --file ACTION.json --wait`. The same account/request ID and identical action returns the same durable operation. A changed request under that ID conflicts. `--wait` exits0 only for applied work; conflicts or unresolved notification outcomes exit5 and include the operation in JSON. Cancelling a CLI does not cancel daemon work.

Each file is one JSON object, at most1MiB, with integer schema_version1, explicit scope (`single` or `series`) and notifications (`none`, `all` or `external_only`). Missing/unknown versions, unknown fields, duplicate version fields and explicit null field values are rejected. Use clear_fields for removal of summary, description, location, recurrence, reminders or color_id. A field cannot be set and cleared together. Example files:

```json
{"schema_version":1,"action":"create","scope":"single","notifications":"none","event":{"summary":"Personal work","start":{"date":"2026-10-02"},"end":{"date":"2026-10-03"}}}
```

```json
{"schema_version":1,"action":"update","event_id":"LOCAL_EVENT_ID","expected_etag":"\"PROVIDER_ETAG\"","scope":"single","notifications":"all","patch":{"summary":"Updated title"},"clear_fields":["description"]}
```

```json
{"schema_version":1,"action":"respond","event_id":"LOCAL_EVENT_ID","expected_etag":"\"PROVIDER_ETAG\"","scope":"single","notifications":"external_only","response":"accepted","comment":"I will attend"}
```

```json
{"schema_version":1,"action":"delete","event_id":"LOCAL_EVENT_ID","expected_etag":"\"PROVIDER_ETAG\"","scope":"single","notifications":"all"}
```

Timed start/end values use `{"date_time":{"rfc3339":"2026-10-02T10:00:00-05:00","time_zone":"America/Chicago"}}`; date-only events have an exclusive end date. Other supported fields are location, recurrence (array of RRULE/RDATE/EXDATE strings), attendees (array with email and optional display_name/optional/resource), reminders (use_default plus overrides containing method/minutes), guests_can_modify, guests_can_invite_others, guests_can_see_other_guests, transparency, visibility and color_id. Only explicitly named fields change. An attendees array explicitly replaces that list; RSVP changes only the selected account's response/comment using Google's partial-response support.

Use scope series with a recurring master; use scope single with an ordinary event or one occurrence. The engine rejects an accidental single edit of a master and an accidental series edit of an occurrence. “This and following,” organizer transfer, sharing administration and local reminder delivery are unsupported. Reader calendars cannot be written. Guest permission/list changes require the organizer copy; the attendee response must match exactly one selected-account email. Current supported write roles are owner/writer. [Google event fields and partial responses](https://developers.google.com/workspace/calendar/api/v3/reference/events).

ETag mismatch records a conflict and preserves intent; if a subsequent read succeeds, the observed version appears in a calendar_conflict receipt and the canonical event projection. A crash triggers reconciliation before any new write. A repeat requires an unchanged ETag or absence of the stable create ID. Remote event content cannot prove invitation delivery: an acknowledgement establishes acceptance of the requested policy; a lost acknowledgement with notifications requested leaves calendar_notifications_unconfirmed even when event content is confirmed. The engine does not repeat invitations to eliminate that uncertainty. Use operation show/attempts to inspect the evidence; an audited abandon decision can stop unresolved work and release later changes to that event.

A successful DELETE returns no event body. Its local deletion marker retains the last known snapshot with cancelled status and no invented ETag. A later sync/read can supply the provider's tombstone. All mutation receipts invalidate agenda coverage and leave the sync cursor unchanged; refresh the agenda for new occurrence expansion. [Google delete response](https://developers.google.com/workspace/calendar/api/v3/reference/events/delete).


## Calendar free/busy

`calendar free-busy --account ACCOUNT --file QUERY.json` explicitly queries the provider. The file is a strict JSON object of at most128KiB, with all the following fields required:

```json
{"schema_version":1,"from":"2026-10-02T00:00:00Z","to":"2026-10-04T00:00:00Z","time_zone":"America/Chicago","provider_calendar_ids":["primary","someone@example.test"]}
```

Use provider calendar IDs from calendar list (or another known calendar ID), not local UUIDs. This build supports1–50 distinct calendars and windows up to366days; group expansion is unsupported. Both bounds require RFC3339 offsets; the upper bound is exclusive. `time_zone` is an IANA zone. Unknown/duplicate fields, missing/unknown versions, invalid dates/zones and duplicate IDs fail with exit2.

The result includes the requested bounds, query timestamp, aggregate complete flag, and each calendar's complete flag, busy periods and provider errors. An empty busy list establishes free time only for a calendar with complete=true and only within the returned window. Unknown future provider error reasons remain visible. HTTP/malformed/missing coverage fails the query instead of returning free time. Partial provider results return exit0 with complete=false; consumers must check coverage. Results are an online observation, not a reservation or cached agenda guarantee. Queries share authenticated account sequencing and persisted provider backoff; they make no event or notification changes. [Google freeBusy contract](https://developers.google.com/workspace/calendar/api/v3/reference/freebusy/query).

## Synology account setup (current Task12 implementation)

Account setup, full/delta mailbox sync, local queries, search and MIME/attachment downloads are available through the engine/API/CLI. Durable read/unread, star/unstar and archive changes are available. Explicit folder move/copy is available. Trash/restore and SMTP submission with client/server Sent policies are available and verified offline. Automated testing uses only the local composition in [MOCK-MAILPLUS.md](MOCK-MAILPLUS.md). Real accounts remain unapproved and untested.

`account connect-imap` reads a version1 public JSON configuration. Both endpoints require `implicit` or `start_tls`; there is no plaintext or opportunistic fallback. Choose the actual provider's ports, exact login names, existing folder names and Sent policy. `trusted_ca_pem` may contain an additional CA certificate bundle; verification of the certificate chain and hostname remains enabled. Omit it to use the bundled public trust roots. A production configuration has this shape (the example domain is deliberately non-routable):

```json
{
  "schema_version": 1,
  "address": "person@example.invalid",
  "imap": {"host": "mail.example.invalid", "port": 993, "tls": "implicit", "username": "person"},
  "smtp": {"host": "mail.example.invalid", "port": 587, "tls": "start_tls", "username": "person"},
  "sent_policy": "client_append",
  "sent_folder": "Sent",
  "archive_folder": "Archive",
  "trash_folder": "Trash"
}
```

Passwords must not appear in that file, command arguments or shell history. The CLI accepts a bounded JSON credential object only through a nonterminal stdin pipe. For an explicitly authorized manual account, Python's standard `getpass` can collect both passwords without echo and pipe them directly:

```sh
python3 -c 'import getpass,json; print(json.dumps({"imap_password":getpass.getpass("IMAP password: "),"smtp_password":getpass.getpass("SMTP password: ")}))' |
  target/production/release/nuncio-cli --json account connect-imap --config imap-public.json --credentials-stdin
```

The production artifact paths above are the final intended operating paths; the existing Task11 production binaries predate these new commands until the next verified production rebuild. Current automated tests build separate feature artifacts under `target/test-harness/debug` with synthetic keystore files.

Connection authenticates IMAP SASL PLAIN and SMTP PLAIN or LOGIN after TLS, then saves credentials in the profile's SecretStore. It sends no mail. The response reports advertised capabilities and any obsolete credential cleanup still pending. Configured folder existence and mail operation support are not established by this account probe yet. `account imap-config --account UUID` reads the saved configuration and last authenticated capability snapshot locally; `account check --account UUID` contacts both endpoints. `account list` stays local. `account disconnect --account UUID` removes access credentials while preserving local data. Reconnect with `--account UUID`; changing the canonical IMAP host/port/TLS or exact username fails identity matching. Address and SMTP configuration may be updated for that same principal.

A temporary IMAP rejection without explicit authentication-failure evidence, or an SMTP4xx authentication response, is unavailable and preserves credentials. Explicit authentication failure pauses the account as `needs_auth`; reconnect after correcting its credentials. TLS trust failures preserve existing credentials and require correcting configuration or trust. All responses/logs use fixed error categories rather than echoing server text or passwords.


After a separately authorized account connection, `sync --account UUID --wait` discovers folders and ingests mail. `--full` refetches message bodies; ordinary delta sync checks the complete UID/flag inventory and reuses cached immutable MIME. UID discovery uses bounded message-sequence pages, so sparse high UIDs do not require scanning the numerical UID space. `mail list`, `mail search`, `mail read`, `mail raw`, `mail attachment` and collection queries use the same local API as Google. Reading mail uses IMAP only; it does not authenticate SMTP or send mail. The background scheduler also dispatches connected IMAP accounts.

Every message placement belongs to one account, a local mailbox identity, UIDVALIDITY and UID. Identical Message-IDs in separate folders remain distinct. A disappeared or renamed folder retires its old identity; a UIDVALIDITY reset replaces that folder's message identities. Saved drafts survive those replacements. Incomplete syncs leave the previous published messages/checkpoint available, and restart discards interrupted staging. Inspect sync status for failures and retry after the underlying connection problem is resolved.

Current read compatibility uses IMAP4rev1 LIST, EXAMINE, UID SEARCH and UID FETCH with BODY.PEEK, strict modified UTF-7 and quoted names. Special-use attributes are preserved when advertised. Optional MOVE, UIDPLUS, CONDSTORE and QRESYNC may be absent; no QRESYNC/CONDSTORE optimization or IDLE-driven polling is claimed. The reader bounds command responses, inventories and payloads; bodies over the configured limit are explicitly unavailable as `too_large`. `mail fetch --account UUID --message UUID --wait` refreshes one existing placement without advancing global coverage or replacing other messages; a changed UIDVALIDITY requires a normal sync to discover the new identities. Independent Dovecot tests and local process crash tests establish offline behavior; live MailPlus compatibility remains unverified.


IMAP flags use the existing `mail change` command. Supply a version1 JSON action file such as `{"schema_version":1,"action":"read","read":true}` or `{"schema_version":1,"action":"star","starred":false}`:

```sh
nuncio-cli --json mail change --account UUID --message UUID --request-id REQUEST_UUID --file action.json --wait
```

Request IDs are account-scoped and idempotent: retrying the same request returns the same operation; changing its intent rejects the reused ID. The journal captures the original placement and folder identity. The daemon checks UIDVALIDITY before UID STORE and changes only the requested flag, preserving other flags and keywords. An already satisfied desired state needs no write. Lost acknowledgements trigger an authenticated positive read; unavailable reconciliation remains visibly uncertain with bounded retry. A reused UID after an epoch reset conflicts without writing. Inspect the operation and sync to discover current placements before submitting a new intent. Applied receipts and local flags commit together; interrupted attempts recover after restart. Three actual daemon/CLI SIGKILL cases and independent remote counters cover before-write, before-local-receipt and withheld-remote-acknowledgement boundaries. These tests establish local Dovecot behavior, not live MailPlus compatibility.


IMAP archive uses the same command with `{"schema_version":1,"action":"archive"}`. Sync first to discover the configured Archive folder. The operation captures the source placement and the current destination folder identity/UIDVALIDITY; it does not create missing folders or redirect a queued operation when configuration changes. An already-Archive message is a positive-read no-op with stable local message and attachment IDs. A real transfer produces a separate destination placement; the original raw bytes remain downloadable under its new local ID.

The daemon uses UID MOVE when advertised. Otherwise it uses UID COPY followed by UID STORE of only the source's Deleted flag and UID EXPUNGE of only the source UID. The fallback requires UIDPLUS before sending COPY; it never issues broad EXPUNGE that could remove other clients' Deleted messages. Validated COPYUID mapping is stored durably before separate deletion. Recovery with that proof observes the destination and resumes remaining source removal without another COPY. Missing or changed source/destination identity produces a conflict; inspect the operation before planning new work.

A lost final acknowledgement can leave a server-side copy without a durable mapping. In that case `--wait` returns exit5 and `operation show --account UUID --operation UUID` reports `uncertain` with `imap_copy_identity_unknown`. Restart and reuse of the same request UUID do not resend it. Identical raw bytes or Message-ID alone do not prove which copy this request created. Inspect `operation attempts`, independently inspect both remote folders, and sync to refresh the cache before deciding whether additional work is needed. Do not generate a new request UUID merely to clear uncertainty: that represents a new transfer and can duplicate the copy. An unresolved transfer does not silently publish a guessed destination into the local projection.

Offline verification covers native and fallback archive, unchanged unrelated Deleted messages, identical-MIME distinct placements, strict COPYUID response validation, and SIGKILL at dispatch, copy-proof, deletion, expunge and local-publication boundaries. A server's early COPYUID can support reconciliation after disconnect if the mapping reached durable storage; killing the process while it is still collecting the reply can lose that evidence and correctly leave uncertainty. These are compatibility limits established against independent local Dovecot, not live Synology acceptance.


For an explicit IMAP folder target, get its local collection UUID from `mail collections --account UUID` and use `{"schema_version":1,"action":"copy","destination_collection_id":"COLLECTION_UUID"}` or the same object with `"action":"move"`. The account-scoped destination UUID is resolved and frozen at enqueue; a folder name or a different account's collection UUID is rejected. Copy preserves the source and creates a distinct placement, including copying into the same folder. Move into the current folder is a no-op. These actions use the same durable proof/recovery rules as archive. Gmail exposes existing-label changes instead and rejects these folder actions. Native MOVE/fallback support still depends on server extensions; inspect operation failures and capabilities.


IMAP `{"schema_version":1,"action":"trash","trashed":true}` moves a placement into the configured Trash folder and retains its original folder identity/UIDVALIDITY. An explicit Move or Copy into configured Trash also records origin; copying within Trash preserves an already-known origin. `trashed:false` restores to that original folder with a new destination UID. Already-Trash is a no-op. Already outside Trash with no pending origin is also a restore no-op. Origin metadata is encrypted and survives process death and full resync; it commits with the destination placement and operation receipt.

Restore rejects an unknown origin, a retired/renamed original folder, or a changed original UIDVALIDITY. Mail put into Trash by another client has no provable original folder here; use an explicit Move to choose its destination. Folder names are not reused as identity after retirement, and neither Message-ID nor matching MIME proves an origin. Restore metadata was introduced in schema16; production binaries still predate Task12 until the next release verification.

## SMTP submission and the client-managed Sent copy

With `sent_policy` set to `client_append`, synchronize first so the configured Sent folder has a known local identity and UIDVALIDITY. The folder must exist and the server must support UIDPLUS. Create and attach files to a local draft using the commands above, then run:

```sh
nuncio-cli --json mail send --account ACCOUNT_UUID --draft DRAFT_UUID --request-id REQUEST_UUID --wait
nuncio-cli --json operation show --account ACCOUNT_UUID --operation OPERATION_UUID
nuncio-cli --json operation attempts --account ACCOUNT_UUID --operation OPERATION_UUID
```

Reusing the same request UUID returns the original operation. It does not send again. The journal retains the frozen MIME and SMTP endpoint, recipients, Sent policy, folder identity and UID epoch. Subsequent draft edits do not change that send. Bcc recipients are in the SMTP envelope and the private Sent copy; the submitted message headers omit Bcc. SMTP framing is frozen before enqueue, including a final CRLF. The transport checks SMTPUTF8/8BITMIME support as needed and applies dot stuffing once.

For client-append policy, the operation is applied only after two separately evidenced effects: `smtp_accepted` records the final SMTP success response; `imap_append` records a validated APPENDUID mapping; `sent_copy` records positive observation and atomic publication of the exact frozen Sent MIME. SMTP acceptance is the server taking responsibility for delivery, not proof that the final recipient received or read the mail. These receipts survive daemon restarts.

An accepted SMTP submission is never repeated to fix a Sent problem. A crash after acceptance but before starting APPEND resumes the copy. A durable APPENDUID allows the worker to observe that placement and publish it without appending again. A complete matching-tag APPEND rejection with no positive copy mapping records `imap_append_rejected` and retries only the copy, preserving SMTP acceptance. Missing or contradictory replies do not grant permission to append again. Complete SMTP 4xx DATA rejections can back off and retry; 5xx failures require review. A partial recipient rejection sends no message body to any recipient in that transaction.

If the daemon cannot prove acceptance after DATA began, `smtp_acceptance_unknown` remains visible. If APPEND began but its placement proof was lost, the error is `imap_sent_copy_identity_unknown`. Neither is retried blindly, even when remote state happens to contain matching MIME. Use operation history and independently inspect the server before deciding on new work; a new request UUID represents another submission and may duplicate delivery. Cache sync may discover remote messages independently of the unresolved operation. `operation resolve` supports abandoning unresolved SMTP work or creating an explicit resend. Abandon records a disposition while preserving the unknown delivery outcome. A resend needs a separate request UUID, the current operation version and `--accept-duplicate-risk`; it reuses frozen content/recipients after draft deletion but assigns a new Message-ID/date and captures a new SMTP/Sent configuration snapshot in the same transaction. Repeating the identical resolution returns the same replacement operation. A new send may duplicate delivery even if the original acknowledgement was lost. Manual `confirm_applied` for SMTP requires positive independent evidence for both acceptance and the exact Sent placement. Copy the canonical `provider_id` from the observed Sent message (the versioned account/mailbox UUID/UIDVALIDITY/UID identity), include the frozen RFC Message-ID without angle brackets, and record when the observation was made. In addition to `note` describing the Sent MIME/placement checks, supply `smtp_acceptance_note` describing the independently established SMTP acceptance. A Sent match alone does not establish delivery. Do not supply a Google ETag for SMTP. The engine rejects mismatched placement, account, epoch, Message-ID, a UID below the saved floor, or confirmation before DATA started. It records `manual_confirmed`, preserving actual attempt history and receipts; it does not manufacture an SMTP acknowledgement or copy receipt. An explicit sync can cache the independently found Sent message without resolving the operation. Confirmation and abandonment do not submit or copy mail; identical repeated decisions remain idempotent after restart.

For `sent_policy: "server"`, the worker never issues client APPEND. It records the current Sent UIDNEXT immediately before transmitting DATA and requires a successful SMTP acknowledgement followed by a unique matching copy in that account/folder/UIDVALIDITY at or above that floor. It compares selected identity headers and exact body bytes, permits added trace headers and omission of the private Bcc header, and rechecks the candidate set. `server_sent_observed` records that positive placement observation before `sent_copy` publishes it locally. A changed body or missing copy remains unconfirmed and receives bounded observation retries; duplicate matching copies or a changed folder epoch remain uncertain. A Sent copy alone never proves delivery when the SMTP acknowledgement was lost.

The current source schema is18. Older server-Sent operations lacking a stored observation floor/content fingerprint cannot gain invented evidence during migration. Provider capability/configuration profiles are verified offline, including server Sent without UIDPLUS and rejection before SMTP DATA for client Sent without UIDPLUS. Queued sends preserve the captured SMTP endpoint; changing it pauses dispatch with identity_mismatch until restored. Recovery/migration and cross-cutting adversarial/resource checks, final production artifacts and live MailPlus acceptance remain. Consult SESSION-STATE.md for current verification rather than treating development binaries as a packaged release. All automated SMTP tests use independent local Dovecot/Mailpit services, including actual daemon/CLI crash tests; no live provider compatibility is implied.

For later explicitly authorized live checks, use [MANUAL-ACCEPTANCE.md](MANUAL-ACCEPTANCE.md). It is currently deferred and unapproved.
