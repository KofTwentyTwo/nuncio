# Manual provider acceptance worksheet

Status: **deferred and not authorized**. James currently requires full local Google and Synology mock services. No live account, recipient, calendar, read, send, invitation, configuration change or cleanup is authorized by this worksheet. Complete all independent implementation and offline verification first. The named-resource approval below is a later, separate step; no credentials belong in this document or chat.

Use verified extracted production artifacts in a new disposable profile. Current feature artifacts and passing mocks do not prove Google or Synology compatibility. Record the production artifact SHA-256 and actual provider versions; never infer that remote CI ran from local results.

Identify the selected artifact using its adjacent archive checksum and extracted-check
receipt, then copy its exact archive and binary hashes into the approval record.
`BUILD-METADATA.json` and `SHA256.json` inside that same archive identify its source
state and contents. In the checkout, `dist/final-candidate/EVIDENCE.json` selects
the current verified candidate and records any remaining conditions. Follow
[PACKAGING.md](PACKAGING.md) to extract a new acceptance copy. Historical report
hashes are not a substitute for verifying the selected archive; all live rows below
remain unapproved and unverified.

## Approval record to complete later

| Field | Required value before live work |
|---|---|
| Approval text, author, date and expiry | Pending James's explicit approval of the completed worksheet |
| Production daemon and CLI paths + SHA-256 | Record the selected verified archive hash, exact newly extracted binary paths and their hashes before approval; use that archive’s metadata and receipt. |
| New disposable data directory and endpoint | Exact absolute directory; numeric loopback endpoint |
| Google account address and OAuth registration path | Disposable identity; local Desktop-client JSON path only |
| Google calendars | Exact provider IDs for owned scratch calendar and separately authorized RSVP fixture |
| Google recipients | Exact address for send, reply and any invitation; each action approved separately |
| Google notification policy | Default proposed acceptance is `none`; RSVP/invitation policy must be named explicitly |
| Synology server | Exact hostname, IMAP/SMTP ports, TLS modes, DSM/MailPlus versions and trusted CA path if needed |
| Synology account and recipients | Exact login names, sender address and allowed destination addresses |
| Synology folders and Sent policy | Existing Inbox/Archive/Trash/Sent names; approved `client_append` or `server` |
| Authorized fixture preparation | Exact synthetic messages/events/folders and which account owner creates them |
| Allowed reads | Exact accounts, folders/calendars and date windows; full sync implies access throughout that scope |
| Allowed writes and maximum counts | Per-row actions below, including cleanup; blanks mean no authorization |
| Recovery and cleanup | Exact object IDs to retain/remove and whether local disposable profile cleanup is permitted |

A provider policy change, OAuth consent change, new recipient/calendar, additional resend or different Sent policy requires a new explicit authorization. Do not infer consent from credentials being available. Enter passwords through hidden terminal input or protected pipes as described in [RUNNING.md](RUNNING.md); OAuth secrets remain in the local registration file and OS keystore.

## Execution record

For each row record: command/action-file hash, start/end time, process exit status, operation/request/run IDs, local result, **separate provider observation**, cleanup result, and pass/fail/unverified. Store any accepted private evidence outside source control. Redact access/refresh tokens, passwords, API bearer tokens and OAuth codes. A transport error or lost acknowledgement is an uncertain outcome until positive evidence resolves it; do not automatically resend.

Commands below are CLI suffixes. Every invocation uses the approved extracted binary with `--json --data-dir APPROVED_PROFILE --endpoint APPROVED_LOOPBACK`. Uppercase names are fields to fill from the approval record; none are executable credentials. Preserve each request UUID for idempotent local command replay.

| ID | Authorized check and exact command/action | Independent acceptance evidence | Status |
|---|---|---|---|
| G01 | `account connect-google --client-config OAUTH_FILE --login-hint GOOGLE_ADDRESS`; inspect `account auth-status --session SESSION_ID` and `account check --account ACCOUNT_ID` | Browser/account owner verifies exact disposable identity, requested consent and stable subject. Confirm no mail/event mutation. | Unverified |
| G02 | `sync --account ACCOUNT_ID --wait`; `mail list --account ACCOUNT_ID --page-size 1` and every returned `--page-token`; `calendar refresh --account ACCOUNT_ID --from YYYY-MM-DD --to YYYY-MM-DD --wait` | Compare fixture IDs/counts with Google UI or a separately authorized provider client. Record bounded date coverage and all pages. | Unverified |
| G03 | External authorized fixture edit/delete/label change, then repeat sync/refresh; restart disposable daemon and repeat reads | Compare changed/deleted remote fixtures with local projection. Local drafts remain intact. Confirm no outgoing messages/notifications. | Unverified |
| G04 | `mail raw --account ACCOUNT_ID --message MESSAGE_ID --output NEW_FILE.eml`; `mail attachment --account ACCOUNT_ID --message MESSAGE_ID --attachment ATTACHMENT_ID --output NEW_FILE.pdf` | Compare SHA-256 and bytes with separately obtained Google originals and fixture PDF. Never hash a second local projection as provider evidence. | Unverified |
| G05 | Save the approved draft file with `mail draft save --account ACCOUNT_ID --file APPROVED_DRAFT.json`; attach approved PDF; `mail send --account ACCOUNT_ID --draft DRAFT_ID --version VERSION --request-id REQUEST_UUID`; inspect `operation wait --account ACCOUNT_ID --operation OPERATION_ID` | Approved recipient verifies exactly one received message, sender/envelope, body, PDF bytes and Message-ID. Google Sent shows one copy. Replay only the same command/request UUID, then verify no second delivery. | Unverified |
| G06 | Prepare `mail reply --account ACCOUNT_ID --message MESSAGE_ID --body-file APPROVED_REPLY.txt`, inspect draft recipients, then send using a separate approved request UUID | Original approved recipient verifies exactly one reply and expected References/In-Reply-To/thread. Stop if preparation introduces an unapproved recipient. | Unverified |
| G07 | `mail change --account ACCOUNT_ID --message MESSAGE_ID --request-id REQUEST_UUID --file APPROVED_LABEL_ACTION.json --wait` using an existing label action from RUNNING | Google UI independently shows the requested label/read/star change. Reverse only the specifically approved fixture changes. | Unverified |
| G08 | `calendar change --account ACCOUNT_ID --calendar LOCAL_CALENDAR_ID --request-id REQUEST_UUID --file APPROVED_EVENT_ACTION.json --wait`; separate create/update/delete requests using current ETags | Owned scratch calendar independently shows the exact event/times/time zone and update/delete. Default proposed notification policy is `none`; no unintended attendee mail. | Unverified |
| G09 | Separately authorized RSVP fixture: same calendar-change command with a `respond` action, current ETag, exact event/scope and named notifications policy | Organizer independently verifies attendee response and exact expected notification count. This is not authorized by G08 or by ordinary calendar reads. | Unverified |
| G10 | Restart and exercise an expired-access-token refresh through an approved `account check`/sync at natural expiry | Same stable account remains usable; no exposed credentials. Do not change system time, revoke grants or inspect token contents to force a result. Missing opportunity remains unverified. | Unverified |
| S01 | Hidden-input `account connect-imap --config APPROVED_IMAP_PUBLIC.json --credentials-stdin`; `account check --account ACCOUNT_ID`; local `account imap-config --account ACCOUNT_ID` | Record server certificate/hostname, configured TLS modes and actual capability snapshot; credentials accepted only after TLS. No mail sent by setup. | Unverified |
| S02 | `sync --account ACCOUNT_ID --wait`; paginate `mail list`; approved external flags/deletion/new-message changes and another sync | Independent MailPlus/IMAP client records mailbox names/encoding, UIDVALIDITY, UIDs and flags; local projection converges without deleting unrelated messages. | Unverified |
| S03 | Raw/attachment export commands from G04 for the approved Synology fixtures | Compare exact downloaded bytes to independent IMAP originals and the original PDF. | Unverified |
| S04 | One approved SMTP draft/PDF submission through the send and operation-wait commands from G05 | Independent recipient verifies one delivery and PDF; independent IMAP client verifies exact Sent UID/UIDVALIDITY/body and one expected copy under the approved policy. Receipt/history distinguishes SMTP acceptance from Sent placement. | Unverified |
| S05 | Approved fixture-only copy/move/trash/restore action files through `mail change`; restart and resync | Independent IMAP client verifies source/destination UID sets, exact bytes, retained unrelated flags/messages and no extra copies. Never use blanket expunge. | Unverified |
| X01 | Review operation history and independent remote observations; perform only approved object-specific cleanup | Record which sent messages, events, invitations and local files remain. A received email cannot be undone by deleting a local record. | Unverified |

The proposed write count is one Google send, one Google reply, one Synology SMTP send, and separately enumerated scratch-calendar and mailbox mutations. Each remains unauthorized until the approval record names the exact recipient/resource and approves that row. Invitation and RSVP counts must be explicitly stated; do not silently choose an external recipient or a notification policy.

## Action files and decision points

Use the versioned examples in [RUNNING.md](RUNNING.md) for drafts, mail changes, calendar changes and free/busy. Draft save, reply preparation, and event action-file creation are review steps; dispatch is a separate command. Before each live dispatch, verify the prepared recipient list, attachment hash, local/provider identity mapping, calendar scope, ETag, notification policy and maximum expected effects against the approved row.

For calendar edits/deletes/responds, retrieve current local state with `calendar get --account ACCOUNT_ID --calendar LOCAL_CALENDAR_ID --event LOCAL_EVENT_ID`. An ETag conflict requires inspection; do not erase the precondition or replace it with an unconditional write. A recurring occurrence requires its explicit occurrence scope; a series change needs separate approval.

Record the actual Sent policy rather than assuming all MailPlus installations behave alike. Changing between `server` and `client_append` is a different acceptance case. Test a second policy only if the account's real configuration and additional send are explicitly approved.

Do not inject production crashes or network failures during these minimal live checks unless separately authorized. The independent local mock suites own automated lost-acknowledgement, crash, repeated-copy and notification tests. Passing those suites cannot substitute for the provider observations above.

## Sign-off

- Software/offline gate and local package evidence: full34-command gate passed; clean6a324f9 package pair passed22checks each with identical hashes. Hosted34679663120 ended8passed/2failed; diagnostic verification is in progress; see current selected receipt.
- Named live authorization: absent; user deferred live scope.
- Google compatibility: unverified.
- Synology/MailPlus compatibility and server versions: unverified.
- Remaining defects, operating limits and precise follow-up: record in VERIFICATION and SESSION-STATE.
- Final acceptance by James: pending. Do not mark the full goal complete from this worksheet or from mock results alone.
