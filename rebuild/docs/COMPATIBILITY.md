# Provider and platform compatibility

Automated evidence covers independent local Google HTTP services and pinned
Dovecot/Mailpit servers. Live Gmail/Calendar, Synology DSM/MailPlus versions and
native OS-keychain operation remain unverified. The separate
[manual worksheet](MANUAL-ACCEPTANCE.md) identifies the exact checks and approvals
required. A passing mock or local CI-equivalent run cannot fill those evidence gaps.

| Area | Implemented behavior and current limits |
|---|---|
| Google identity | Desktop OAuth with PKCE/state, stable subject identity, refresh and account isolation. Organization consent policies and actual token rotation need live acceptance. No service-account/domain-wide delegation flow. |
| Gmail | Paginated/full and history sync, cached queries/search, MIME/raw/attachments, labels/flags/trash/archive, local drafts/reply/forward/send and durable uncertainty. Drafts remain profile-local; they are not Google Drafts synchronization. Permanent purge is excluded. |
| Google Calendar | Catalog, canonical events/occurrences, agenda refresh, explicit single/series writes, RSVP and free/busy. Reader writes are denied; limited writers can change non-private events while private details/actions are restricted. “This and following,” ownership transfer, sharing administration and native reminder delivery are excluded. Notification acknowledgement loss remains uncertain even when event content is observed. |
| Synology mail | Strict IMAP/SMTP over implicit TLS or STARTTLS, SASL/password credentials, folder discovery, UID/flag/body sync, copy/move/trash/restore and SMTP with explicit Sent policy. No plaintext/opportunistic TLS fallback. UIDVALIDITY and server capabilities constrain safe actions; unsupported semantics fail explicitly. |
| Sent policy | SMTP acceptance and Sent-folder copy have independent receipts. `client_append` needs supported UIDPLUS behavior and configured Sent identity; `server` relies on separately verified server-side Sent behavior. An accepted SMTP submission is never repeated to repair a missing copy. |
| Synology calendar | Calendar functionality in this goal uses Google Calendar. Synology Calendar/CalDAV is not part of the approved MailPlus IMAP/SMTP adapter. |
| IMAP servers | The adapter is exercised against local independent implementations; compatibility with arbitrary IMAP/SMTP servers is not inferred. Renamed/retired folders and UID epochs retain separate identities. A message trashed by another client has no provable local original folder; choose an explicit Move instead of guessed restore. |
| Payload/resource limits | Default64MiB per payload, bounded streams/queues,2 concurrent network exchanges and64 active/waiting admissions. Metadata/RSS workloads have machine-specific results; these are not universal latency or memory guarantees. |
| Native credentials/platforms | The selected dependency uses native macOS Keychain and Linux Secret Service; only the local macOS build is verified so far, and automated tests use synthetic stores. The verified archive is macOS ARM64. Hosted Linux/macOS CI and additional package architectures remain separate evidence. |
| API clients | Versioned authenticated `nuncio.v2`, opaque local IDs, scoped page/revision tokens, byte streams and committed-change replay. A separately generated status/watch client passes actual-daemon tests without engine/storage dependencies. Native apps remain outside scope. |

Polling and provider reads establish eventual local convergence; this tool does
not offer exactly-once remote sends or notifications when a provider acknowledgement
is lost. It retains durable intent, observations and uncertainty instead of silently
repeating potentially completed actions. Check [RECOVERY.md](RECOVERY.md) before
resolving such work or restoring an older profile snapshot. Original application
data is preserved; rebuild-schema migrations are not a legacy-data importer.
