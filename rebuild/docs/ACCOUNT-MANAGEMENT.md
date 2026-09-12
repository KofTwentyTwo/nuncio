# Account management

The engine, authenticated API and CLI support account inspection, local naming,
Google consent, IMAP/SMTP configuration and credentials, pause/resume,
archive/restore and confirmed permanent local deletion. This guide describes the
schema 23 implementation. The current verification and selected package are
identified in [SESSION-STATE.md](SESSION-STATE.md); an older package does not gain
these commands when its documentation is updated.

Use the daemon and CLI from the same testing installation, with the same explicit
`--data-dir` and `--endpoint`. See [TESTING-INSTALL.md](TESTING-INSTALL.md) for the
prebuilt download path and [PACKAGING.md](PACKAGING.md) for startup. Examples below
omit those global options. Provider setup remains subject to the separately
authorized [manual acceptance worksheet](MANUAL-ACCEPTANCE.md); automated tests
use synthetic local providers only.

## Inspect and name accounts

```sh
nuncio-cli --json account list
nuncio-cli --json account list --include-archived
nuncio-cli --json account show --account ACCOUNT_ID
nuncio-cli --json account edit --account ACCOUNT_ID --version CURRENT_VERSION --name 'Personal mail'
```

`show` returns `result.account`; list returns `result.accounts`. Each entry contains
the opaque local `id`, provider, address, stable provider `identity`, local
`display_name`, configuration `version`, effective `state` and separate
`auth_state`. These are local reads. `account check --account ACCOUNT_ID` is an
explicit online credential/identity probe and requires a connected account.

Use the version from the latest `show` when editing. A stale version fails with
exit 5; reread and make an intentional edit. Names are at most 256 UTF-8 bytes,
contain no control characters or leading/trailing whitespace, and may be empty.
Changing a name changes no provider identity or remote display name. Google
addresses and stable subjects are provider-owned identities, not editable names.

## Add or reauthenticate Google

After account access is authorized, supply the downloaded Desktop OAuth client
registration from a private regular file outside the repository, mode 0600:

```sh
nuncio-cli account add-google --client-config /PRIVATE/PATH/google-desktop.json
nuncio-cli account reauth-google --account ACCOUNT_ID --client-config /PRIVATE/PATH/google-desktop.json
```

These commands open the system browser and wait up to five minutes for terminal
consent status. `--no-browser` prints the consent URL for manual opening;
`--no-wait` returns the pending session immediately. A waiting command exits 0
only after authentication succeeds. Timeout or failed/cancelled consent exits 5
and includes recovery/session information. Use these commands to inspect,
continue waiting or cancel explicitly:

```sh
nuncio-cli account auth-status --session SESSION_ID
nuncio-cli account auth-wait --session SESSION_ID --timeout-seconds 120
nuncio-cli account auth-cancel --session SESSION_ID
```

The timeout option accepts 1–300 seconds; it does not extend the session's
five-minute lifetime. An expired session requires a new flow. Interrupting a
waiting CLI requests cancellation; if consent already completed, the successful
account remains. Killing the daemon loses pending in-memory sessions: inspect
the account list after restart before starting again.

Reauthentication requires the saved stable Google identity. Choosing another
Google user fails. Ordinary add reuses a known subject's local ID, but an archived
entry must be restored explicitly first. Archive/purge cancel pending flows for
that account and untargeted flows whose identity is not known yet; a pending
untargeted add for another account may therefore need restarting. The previous
`connect-google` command remains available and returns immediately by default;
its optional `--wait` enables the same bounded wait.

Google project/consent setup, requested scopes and refresh-token limitations are
documented in [RUNNING.md](RUNNING.md#production-preparation-for-later-authorized-acceptance).
The mock flow passing does not establish live Google or macOS Keychain acceptance.

## Add, edit or reauthenticate IMAP/SMTP

Prepare the version 1 public settings file described in
[Synology account setup](RUNNING.md#synology-account-setup). Both endpoints require
verified TLS. Passwords are accepted only as bounded JSON through nonterminal
stdin; never put them in command arguments, public settings or shell history.
For an authorized account, this prompt keeps entered passwords off the terminal:

```sh
python3 -c 'import getpass,json; print(json.dumps({"imap_password":getpass.getpass("IMAP password: "),"smtp_password":getpass.getpass("SMTP password: ")}))' |
  nuncio-cli account add-imap --config imap-public.json --credentials-stdin
```

`add-imap` is an alias for `connect-imap`. To replace passwords using the saved
settings, pipe the same prompt into
`nuncio-cli account reauth-imap --account ACCOUNT_ID --credentials-stdin`.

```sh
nuncio-cli account imap-config --account ACCOUNT_ID
nuncio-cli account edit-imap --account ACCOUNT_ID --version CURRENT_VERSION --config imap-public.json
```

An edit without `--credentials-stdin` reuses the secured credentials. Supply that
flag and the protected pipe to change passwords with settings. Both endpoints
are authenticated before new settings are published; failed probes or stale
versions preserve the previous settings and credentials. These explicit setup
actions contact the server even when the account is paused, and preserve its
pause flag. They do not send mail or prove configured folder capabilities.

The canonical IMAP host, port, TLS mode and exact username identify the principal.
Changing that principal requires adding a separate account. For the same
principal, edits may change the sending address, SMTP endpoint/username, trust
bundle, folders and Sent policy. Existing queued work retains its captured
configuration; an incompatible endpoint change can hold it with
`identity_mismatch`. Inspect its receipt before changing configuration again.

## Pause, disconnect, archive and restore

| Action | Credentials | Cached data and history | Provider work |
|---|---|---|---|
| `pause` | Retained | Retained, visible | Scheduled/manual work and operation dispatch stop after coordinated in-flight work finishes |
| `resume` | Retained | Retained, visible | Allowed only if authentication is connected; existing backoff/uncertainty remains |
| `disconnect` | Removed through durable cleanup | Retained, visible | Requires reauthentication |
| `remove` / `archive` | Removed through durable cleanup | Retained; hidden from ordinary list | Archived; new content/remote-work intents and reauthentication rejected |
| `restore` | Not recreated | Retained, visible | Remains disconnected until explicit reauthentication |

```sh
nuncio-cli account pause --account ACCOUNT_ID
nuncio-cli account resume --account ACCOUNT_ID
nuncio-cli account remove --account ACCOUNT_ID
nuncio-cli account list --include-archived
nuncio-cli account restore --account ACCOUNT_ID
```

Pause and authentication are separate: an entry can report `state=paused` with
`auth_state=connected`, `needs_auth` or `disconnected`. Archive takes precedence
in the effective state. Reauthentication preserves pause; resuming an account
does not erase provider backoff or authorize a duplicate uncertain operation.
Cached reads remain available by account ID while archived. Archive blocks draft
writes, attachment-upload publication and new send/mail/calendar/resend or
reconciliation intents. Existing operation observations and explicit non-sending
resolution remain available for recovery.

Archive/purge commit their local result before attempting obsolete-credential
cleanup. A successful response with `credential_cleanup_pending=true` means the
local transition completed and secure-store deletion remains queued. Restart
after the keystore becomes available to retry cleanup, even if the account was
purged. These actions do not revoke Google's remote grant or delete server mail.

## Permanent local deletion and recovery

```sh
nuncio-cli account purge --account ACCOUNT_ID --dry-run
nuncio-cli account purge --account ACCOUNT_ID --confirm ACCOUNT_ID
```

The account must already be archived. The preview reports its version, profile
revision, mail/calendar/event/draft/blob counts and bytes, queued operations and
unresolved operations. Confirmation must exactly repeat the account ID. The CLI
obtains a fresh preview, then submits that version/revision to the API; a
concurrent change fails instead of deleting against a stale preview.

An unresolved remote effect prevents purge. Inspect `operation show` and
`operation attempts`; reconcile only after restore/reauthentication when provider
access is appropriate, or use a deliberate supported confirmation/abandonment.
Abandonment discards local responsibility for an outcome; it does not prove that
delivery, a copy or a notification did not occur. Resend is new remote work and
is rejected while archived.

Purge transactionally deletes the account's current-profile rows, cached content,
drafts and operation history, and redacts old account change records while
preserving contiguous global revisions. Other accounts and profile keys remain.
A global purge record retains the removed local ID. Before-commit crashes leave
the original archive; after-commit crashes leave it deleted. Pending credential
cleanup remains retryable. Purge is logical deletion, not secure erasure of disk
blocks, WAL history, backups or exports, and never removes remote provider data.

Archive is the default recoverable removal. After purge, recover retained content
only from a prior encrypted backup into a new profile, following
[RECOVERY.md](RECOVERY.md). Backup/restore preserves account names, versions and
pause/archive state but does not restore credentials or automatically resume
pending remote writes. An older binary must refuse schema 23; swapping binaries
is not a database downgrade.

## API and evidence

The additive account methods are `GetAccount`, `UpdateAccount`,
`UpdateImapAccount`, `SetAccountLifecycle`, `PreviewAccountPurge`, `PurgeAccount`
and `CancelGoogleAuth`. Existing account methods remain. The account service is
part of the authenticated [v2 contract](API.md); CLI JSON remains schema version 1.
All command errors retain the documented nonzero exit/status mapping.

[ACCOUNT-MANAGEMENT-PLAN.md](ACCOUNT-MANAGEMENT-PLAN.md) maps requirements AM01–AM08.
Storage/backup/rollback tests, independent Google/IMAP system tests and real
daemon/CLI subprocess tests cover the implementation, including lost
acknowledgements, crash boundaries and independently observed remote effects.
Exact current commands, exit statuses and package limits are in
[VERIFICATION.md](VERIFICATION.md). Live Google, Synology and native-keystore
acceptance remain separate and unverified.
