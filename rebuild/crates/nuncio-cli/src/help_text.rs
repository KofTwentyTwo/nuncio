pub const ROOT: &str = r#"Getting started (keep nunciod running in another terminal):
  nuncio-cli --profile laptop-qa system status
  nuncio-cli --profile laptop-qa account add
  nuncio-cli --profile laptop-qa account list
  nuncio-cli --profile laptop-qa mail list --account ACCOUNT_ID

Replace ACCOUNT_ID with the local id shown by account list. Use the same
--profile as the daemon on every command. Results are readable text by default;
add --json for the stable machine format. Run COMMAND --help for details."#;

pub const MAIL: &str = r#"Read mail:
  nuncio-cli account list
  nuncio-cli sync --account ACCOUNT_ID --wait
  nuncio-cli mail list --account ACCOUNT_ID
  nuncio-cli mail read --account ACCOUNT_ID --message MESSAGE_ID
  nuncio-cli mail search --account ACCOUNT_ID --query 'meeting notes'

ACCOUNT_ID comes from account list; MESSAGE_ID comes from mail list/search.
Use the same --profile as your daemon. List, read, search and exports use the
local cache; sync and fetch download from the provider. An empty list can mean
the account has not synchronized yet. Draft commands save locally; send is an
explicit separate action. Run 'nuncio-cli mail COMMAND --help' for its options."#;

pub const SYNC: &str = r#"Example:
  nuncio-cli --profile laptop-qa sync --account ACCOUNT_ID --wait

Find ACCOUNT_ID with account list. This synchronizes mail; use calendar refresh
for calendars. Without --wait, inspect the returned run ID with system sync-status.
--wait waits up to five minutes. --full reconciles remote mail without deleting
local drafts or operation history."#;

pub const CALENDAR: &str = r#"Examples:
  nuncio-cli calendar refresh --account ACCOUNT_ID --wait
  nuncio-cli calendar list --account ACCOUNT_ID
  nuncio-cli calendar agenda --account ACCOUNT_ID --from 2026-10-01 --to 2026-10-08
  nuncio-cli calendar get --account ACCOUNT_ID --calendar CALENDAR_ID --event EVENT_ID

Account IDs come from account list, calendar IDs from calendar list, and event
IDs from agenda. Use local IDs, not provider IDs. Date windows are YYYY-MM-DD
with an exclusive end date. List/get/agenda read the cache and report coverage;
refresh/free-busy contact Google. Calendar writes require an explicit action,
edit scope and notification policy. Synology calendar is not supported."#;

pub const SYSTEM: &str = r#"Examples:
  nuncio-cli system status
  nuncio-cli system sync-status --account ACCOUNT_ID --run SYNC_RUN_ID
  nuncio-cli system cancel-sync --account ACCOUNT_ID --run SYNC_RUN_ID
  nuncio-cli system watch --after REVISION
  nuncio-cli system shutdown

Use the same --profile and --endpoint as your daemon. Run IDs come from sync,
fetch, refresh or repair. Watch always emits JSONL; obtain an initial revision
from status and keep the last observed revision to resume after disconnecting."#;

pub const DRAFT: &str = r#"Examples:
  nuncio-cli mail draft save --account ACCOUNT_ID --file draft.json
  nuncio-cli mail draft list --account ACCOUNT_ID
  nuncio-cli mail draft show --account ACCOUNT_ID --draft DRAFT_ID
  nuncio-cli mail draft attach --account ACCOUNT_ID --draft DRAFT_ID --version VERSION --file report.pdf

These commands change local drafts only. Copy the latest version from draft show
before updating, attaching or deleting. Send separately with mail send. Use
'mail draft save --help' for the JSON format."#;

pub const OPERATIONS: &str = r#"Examples:
  nuncio-cli operation list --account ACCOUNT_ID
  nuncio-cli operation show --account ACCOUNT_ID --operation OPERATION_ID
  nuncio-cli operation attempts --account ACCOUNT_ID --operation OPERATION_ID
  nuncio-cli operation wait --account ACCOUNT_ID --operation OPERATION_ID

Use IDs returned by write commands or operation list. Queued is not delivered;
inspect the state and provider receipts. A timeout does not cancel the operation.
An uncertain send may already have been delivered: inspect/reconcile its evidence
before making an explicit recovery decision. Use show for the current version."#;

pub const REPAIR: &str = r#"Examples:
  nuncio-cli repair --account ACCOUNT_ID --scope mail --dry-run
  nuncio-cli repair --account ACCOUNT_ID --scope mail --wait

Preview first. Repair rebuilds a provider cache and retains drafts and operation
history; it does not erase the account. Calendar date bounds require
--scope calendar and paired --from/--to dates. The old complete cache remains
available if a rebuild fails. This is not an immediate cache eviction command."#;

pub const BACKUP: &str = r#"Examples (read the passphrase from an owner-only file or secure pipe):
  nuncio-cli backup create --output backup.nuncio < /private/path/recovery.json
  nuncio-cli backup inspect --file backup.nuncio < /private/path/recovery.json
  nuncio-cli backup restore --file backup.nuncio --new-profile recovered < /private/path/recovery.json

Recovery input is a JSON object with one field: passphrase. Use at least twelve
characters. Keep it private and out of shell history, chat and Git. The daemon
must be running. Restore creates a new sibling profile, preserves the source,
and holds pending remote writes for explicit recovery. It never replaces the
current profile. Full procedure: the package's docs/RECOVERY.md."#;

pub const MAIL_CHANGE: &str = r#"Example action.json:
  {"schema_version":1,"action":"read","read":true}

Queue it:
  nuncio-cli mail change --account ACCOUNT_ID --message MESSAGE_ID --request-id REQUEST_ID --file action.json --wait

Other actions: star (starred boolean), archive (no extra fields), trash (trashed
boolean; false restores), label (collection_id and present boolean), move/copy
(destination_collection_id). IDs come from mail collections. Check mail
capabilities for provider support. Use a UUID request ID (create once with uuidgen) for each new intent;
reuse it only with the identical account/action/payload after a failed response."#;

pub const DRAFT_SAVE: &str = r#"Example draft.json:
  {"to":[{"address":"recipient@example.test"}],"subject":"Hello","text":"A short message."}

Create a local draft:
  nuncio-cli mail draft save --account ACCOUNT_ID --file draft.json

For an update, add --draft DRAFT_ID --version VERSION from draft show. Recipient
fields to/cc/bcc are arrays of address objects. Optional html, in_reply_to and
references fields preserve message content/threading. Attach files separately
with draft attach. Saving does not send; mail send requires an explicit account,
draft and UUID request ID (create once with uuidgen)."#;

pub const CALENDAR_CHANGE: &str = r#"Example create.json (a one-day event; end date is exclusive):
  {"schema_version":1,"action":"create","scope":"single","notifications":"none","event":{"summary":"Test event","start":{"date":"2026-10-01"},"end":{"date":"2026-10-02"}}}

Queue it:
  nuncio-cli calendar change --account ACCOUNT_ID --calendar CALENDAR_ID --request-id REQUEST_ID --file create.json --wait

Actions: create/update/delete/respond. Scope: single or series. Notifications:
none/all/external_only. Update/delete/respond require event_id and expected_etag
from calendar get. REQUEST_ID must be a UUID. Update uses patch and optional clear_fields. Respond uses
response: accepted/declined/tentative/needs_action. Timed values use date_time:
{"rfc3339":"2026-10-01T09:00:00-05:00","time_zone":"America/Chicago"}.
Details and supported fields: the package's docs/RUNNING.md."#;

pub const FREE_BUSY: &str = r#"Example query.json:
  {"schema_version":1,"from":"2026-10-01T00:00:00Z","to":"2026-10-02T00:00:00Z","time_zone":"UTC","provider_calendar_ids":["primary"]}

Run: nuncio-cli calendar free-busy --account ACCOUNT_ID --file query.json

This is an online Google query. Unlike other calendar commands, the file uses
provider calendar IDs (from calendar list), not Nuncio's local calendar IDs.
Check each calendar's returned coverage and errors before treating it as free."#;

pub const RESOLVE: &str = r#"Example decision.json:
  {"decision":"abandon","reason":"Reviewed independently; do not attempt this again"}

Run: nuncio-cli operation resolve --account ACCOUNT_ID --operation OPERATION_ID --version VERSION --file decision.json

Read show/attempts first. Abandon prevents further attempts but cannot undo a
remote effect. confirm_applied requires independently verified evidence;
resend requires a new UUID request_id, reason and --accept-duplicate-risk. A resend
can deliver a second copy. Detailed evidence formats: docs/RUNNING.md in the
package. Never infer delivery from a local timeout."#;

pub const GOOGLE_SETUP: &str = r#"Example (private Desktop OAuth JSON downloaded from Google Cloud):
  nuncio-cli account add-google --client-config /private/path/nuncio-google.json

Protect the JSON with chmod 600 and keep it outside Git and chat. The same
--client-config option is available on account add and reauth-google. Use the
same --profile as your daemon. First-time registration and browser consent:
docs/GOOGLE-SETUP.md in the installed package. For an existing account, reauth
must sign in as that saved identity; account list shows its local account ID."#;

pub const IMAP_SETUP: &str = r#"For interactive setup with hidden password entry, use account add.
For scripts, keep public settings and private credentials separate:
  nuncio-cli account connect-imap --config mailplus.json --credentials-stdin < /private/path/credentials.json

Public config (schema_version 1): address, imap and smtp endpoints, sent_policy,
and sent_folder. Each endpoint needs host, port, tls (implicit or start_tls),
and username. Optional fields: archive_folder, trash_folder, trusted_ca_pem.
The two credential JSON fields are imap_password and smtp_password; read them
from a protected file or secure pipe, never command arguments or shell history.
Config is limited to 256 KiB and credentials to 16 KiB. Use account imap-config
--json for an existing account's public settings; edit-imap also requires its
current --version from account show. Reauth-imap reads only credentials and
uses saved settings. Complete examples and Sent-copy policy selection:
docs/ACCOUNT-SETUP.md and docs/RUNNING.md in the installed package."#;
