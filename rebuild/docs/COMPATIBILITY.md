# Provider and platform compatibility

Automated evidence covers independent local Google HTTP services and pinned
Dovecot/Mailpit servers. Live Gmail/Calendar, Synology DSM/MailPlus versions and
native OS-keychain operation remain unverified. The separate
[manual worksheet](MANUAL-ACCEPTANCE.md) identifies the exact checks and approvals
required. A passing mock or local CI-equivalent run cannot fill those evidence gaps.
The latest qualified testing source is bb70f95; its actual CI, public installation
and local packaging receipts are recorded in [VERIFICATION.md](VERIFICATION.md).
The independently reproduced IMAP defects are corrected in this build; laptop confirmation and native startup diagnosis remain pending. See [the current state](SESSION-STATE.md).

| Area | Implemented behavior and current limits |
|---|---|
| Google identity | Desktop OAuth with PKCE/state, stable subject identity, refresh and account isolation. Organization consent policies and actual token rotation need live acceptance. No service-account/domain-wide delegation flow. |
| Account lifecycle | Current source implements local details/name/version, settings/credential replacement, pause/resume, archive/restore, and separate confirmed local purge. Archive keeps cached data; purge requires an archived account and no unresolved remote outcomes. Neither deletes provider data or existing backups/exports. See [account management](ACCOUNT-MANAGEMENT.md) for current verification and cleanup limits. |
| Gmail | Paginated/full and history sync, cached queries/search, MIME/raw/attachments, labels/flags/trash/archive, local drafts/reply/forward/send and durable uncertainty. Drafts remain profile-local; they are not Google Drafts synchronization. Permanent provider-message purge is excluded. |
| Google Calendar | Catalog, canonical events/occurrences, agenda refresh, explicit single/series writes, RSVP and free/busy. Reader writes are denied; limited writers can change non-private events while private details/actions are restricted. “This and following,” ownership transfer, sharing administration and native reminder delivery are excluded. Notification acknowledgement loss remains uncertain even when event content is observed. |
| Synology mail | Strict IMAP/SMTP over implicit TLS or STARTTLS, SASL/password credentials, folder discovery, UID/flag/body sync, copy/move/trash/restore and SMTP with explicit Sent policy. No plaintext/opportunistic TLS fallback. UIDVALIDITY and server capabilities constrain safe actions; unsupported semantics fail explicitly. |
| Sent policy | SMTP acceptance and Sent-folder copy have independent receipts. `client_append` needs supported UIDPLUS behavior and configured Sent identity; `server` relies on separately verified server-side Sent behavior. An accepted SMTP submission is never repeated to repair a missing copy. |
| Synology calendar | Calendar functionality in this goal uses Google Calendar. Synology Calendar/CalDAV is not part of the approved MailPlus IMAP/SMTP adapter. |
| IMAP servers | The adapter is exercised against local independent implementations; compatibility with arbitrary IMAP/SMTP servers is not inferred. Renamed/retired folders and UID epochs retain separate identities. A message trashed by another client has no provable local original folder; choose an explicit Move instead of guessed restore. |
| Payload/resource limits | Default64MiB per payload, bounded streams/queues,2 concurrent network exchanges and64 active/waiting admissions. Metadata/RSS workloads have machine-specific results; these are not universal latency or memory guarantees. |
| Native credentials/platforms | Native macOS Keychain and Linux Secret Service are the selected production stores; automated tests use synthetic stores. Current Ubuntu 24.04 and macOS 15 CI and packaging commands passed at `bb70f95`. The retained local archive is macOS ARM64; the hosted ARM64 archive was independently downloaded, verified and installed into a temporary prefix. Native-keystore acceptance, Windows, and additional package architectures are not established. The [testing installer](TESTING-INSTALL.md) supports native Apple Silicon macOS 15+ only. |
| API clients | Versioned authenticated `nuncio.v2`, opaque local IDs, scoped page/revision tokens, byte streams and committed-change replay. A separately generated status/watch client passes baseline actual-daemon tests without engine/storage dependencies. New account methods require the current source; publication/SemVer tooling remains a [proposal](API-PUBLICATION-PLAN.md). Native apps remain outside scope. |

Polling and provider reads establish eventual local convergence; this tool does
not offer exactly-once remote sends or notifications when a provider acknowledgement
is lost. It retains durable intent, observations and uncertainty instead of silently
repeating potentially completed actions. Check [RECOVERY.md](RECOVERY.md) before
resolving such work or restoring an older profile snapshot. Original application
data is preserved; rebuild-schema migrations are not a legacy-data importer.

## Active mailbox synchronization

The catch-up correction under verification retries a changed mailbox at most
three times in the same run, retaining already downloaded bodies only within
that mailbox’s unchanged UIDVALIDITY. Flag notifications are validated separately
from requested metadata/body responses. An expunged staged message is omitted
from the final snapshot. Published mail stays unchanged until the entire run
commits; an empty cache with unavailable coverage is not evidence of an empty
remote mailbox. Repeated changes or an identity reset report `mailbox_changed`;
transport loss and changes that interrupt bounded inventory/body reads can still
fail the run. Retry when the connection/mailbox is stable. This is eventual
convergence, not a transaction spanning remote mailboxes.
