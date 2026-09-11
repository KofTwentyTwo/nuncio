# Recovery

Task 13 remains in progress. Encrypted backup/inspection/new-profile restore and projection repair work through the engine, authenticated API and CLI. Schema22 recovery has44 actual migration SIGKILL cases and five restore crash boundaries, with original keys/data and independently observed provider effects preserved. Explicit restored-operation reconciliation is implemented across Google mail/calendar and IMAP/SMTP; pending work stays held unless an explicit safe reconciliation or duplicate-risk decision permits progress. Latest startup/relative-path gate passes engine126, recoveryE2E4, IMAPE2E13, release-isolation2 and both Clippy configurations. Earlier upload/export crash cleanup, ordinary failed-job retirement, broader storage/error checks and final acceptance remain open. See [SESSION-STATE.md](SESSION-STATE.md) and [VERIFICATION.md](VERIFICATION.md) for exact evidence and remaining scope.

## Commands and passphrase input

Use the daemon's current profile and endpoint with the CLI. The following are command suffixes; include the usual `--data-dir` and `--endpoint` options for a nondefault profile. All three commands read one JSON object containing only `passphrase` from stdin. They refuse an interactive terminal on stdin to prevent echoed secrets. Supply JSON through a protected pipe or a private file outside the repository; never put the phrase in shell history, argv, an environment variable, or ordinary configuration.

```text
backup create --output /private/path/snapshot.nuncio
backup inspect --file /private/path/snapshot.nuncio
backup restore --file /private/path/snapshot.nuncio --new-profile recovered
```

For a hidden local prompt without storing the phrase in a file, pipe Python's `getpass` output directly to the CLI (the prompt reads from the controlling terminal):

```sh
python3 -c 'import getpass,json; print(json.dumps({"passphrase":getpass.getpass("Recovery passphrase: ")}))' | nuncio-cli --json backup inspect --file /private/path/snapshot.nuncio
```

The example assumes the binary is already available; it does not install anything. Test-harness binaries require their synthetic secret/config flags as described in [RUNNING.md](RUNNING.md). Use only synthetic accounts for current acceptance.

Create requires a new output file and refuses existing files, hard links, dangling symlinks, or aliases to the active profile's protected database/configuration paths, including future journals. It writes a private temporary file, validates stream order, size and ciphertext SHA-256, then fsyncs and publishes without replacement. No partial stream is reported as a completed backup. A directory-sync failure after publication reports `backup_output_uncertain`; preserve and inspect the completed file before retrying.

Inspection reports format/schema version, snapshot time/revision, account/draft/operation counts, ciphertext length and SHA-256. It does not expose credentials or content. Restore accepts only a new sibling name of 1–64 ASCII letters, digits, hyphens or underscores. It returns the new profile UUID, canonical absolute directory, migrated schema/revision, backup inspection and number of held operations. It does not start the restored daemon.

There is one active backup/inspection/restore transfer per profile. Returned backup artifacts and incomplete/completed uploads retain the source profile lock alongside admission, including after Engine shutdown; reopening stays locked until their last owner releases them. Cancelling a restore caller does not cancel its blocking work; that worker retains the source-profile lock through completion, even if the original engine shuts down. A second engine receives a locked-profile error until the work finishes. Startup recovery retains that same lock inside its blocking cleanup worker after caller cancellation. Relative source paths and ancestor aliases are normalized with directory-identity checks; a symlink replacing the source directory is rejected. Uploads declare a positive size up to 1 TiB, use ordered nonempty chunks of at most 256 KiB, and must match the declared hash and length. Upload idle timeout is 30 seconds. Backup CLI RPCs allow up to one hour. Available-space preflight now estimates one upload copy, two backup copies or three restore copies, plus 8 MiB working margin. It is an estimate, not a disk reservation; other processes can consume free space afterward. Actual copy/export/fsync errors still prevent publication/activation. SQLCipher export also enforces a 1 TiB page bound. Restore rejects oversized inputs before copying and reads only the declared length plus a one-byte growth probe. Process-crash cleanup and broader migration/error injection remain in progress. A cancelled caller does not release admission while its blocking filesystem/keystore work is still active.

## Snapshot format

`Store::create_backup` uses the serialized store connection and a transaction to export the committed database, including WAL-visible rows, into a separately encrypted SQLCipher database. Drafts, attachments, operation payloads and attempt history are included. An encrypted metadata table records format version 1, schema version, snapshot revision and creation time. The backup has application ID `0x4e554e42`. The source store is unchanged.

The recovery passphrase is separate from the profile key. It uses SQLCipher's passphrase derivation; raw-key literal forms are rejected, including the bundled SQLCipher encryption-key/HMAC-key/salt form. Current input validation requires at least 12 characters, at most 4096 UTF-8 bytes, no NUL, and a value containing non-whitespace text. Use a strong unique phrase. It will be needed for restore, with no reset path. No passphrase belongs in argv, configuration, logs or source control.

SQLCipher export does not copy `user_version`, so creation preserves it explicitly. Inspection opens the file read-only, performs a real schema read after keying, checks the backup identity/version and metadata, and runs cipher integrity, SQLite integrity and foreign-key checks. A SHA-256 of the encrypted file and its byte length support later stream verification. [SQLCipher API](https://www.zetetic.net/sqlcipher/sqlcipher-api/)

Temporary artifacts live in a private directory beneath the source profile and are removed when the library artifact is dropped. The backup database itself is mode 0600 on Unix. Creation verifies the exported database before returning it. The authenticated Maintenance stream exports the completed ciphertext to the CLI; filesystem destinations remain client concerns.

## Restore staging

`stage_restore` copies a regular, non-symlink single-file backup into a private stage. Existing WAL/SHM/journal companions are rejected. Inspection and SQLCipher export operate on that owned copy; the caller's source remains untouched. Export uses a new 32-byte database key and explicitly restores schema metadata. The copied database migrates to the current schema, clears provider credential references, the old credential-cleanup queue and source restore-cleanup authority, and disconnects every account. This prevents cleanup in a restored profile from deleting secrets belonging to the original profile.

Every pending operation is held as uncertain with automatic reconciliation disabled, including work that was queued at snapshot time. That old state cannot prove the operation was never sent afterward. Source state/version and backup hash are recorded in `restored_operations`; unfinished attempts retain their identity and become interrupted/uncertain, without invented delivery/copy receipts. Drafts, PDF attachments, frozen MIME, request identity and existing receipts survive. Terminal/resolved operations are preserved. Reconnection and ordinary sync do not release these holds. The existing explicit resend resolution creates a separate operation after duplicate-risk acknowledgement; it does not resume the original operation. Use explicit `operation reconcile` to observe or safely continue the original operation, subject to the provider-specific proof rules below.

Activation uses an atomic no-replace directory rename on macOS/Linux. It refuses existing targets, including empty directories and symlinks. Before activation, dropping an unchanged stage cleans only its private directory. Parent and stage identities are retained and checked; moved/replaced paths are refused, and cleanup preserves replacement directories. If another process moved a stage or its parent, the original encrypted stage is retained for inspection. After successful rename, an fsync error preserves the target and returns an activation-uncertain error; inspect that target before retrying. `Engine::restore_profile` generates fresh profile/database/API identities, writes private profile metadata and new keystore entries, then activates without starting the restored engine. It rolls back only the new key references if initialization or activation fails. `RestoreCleanupIncomplete` identifies the new profile whose key deletion failed; retain the original profile and remove only those named database/API entries when secure storage becomes available. An activation-uncertain error retains the new keys because the target already exists.

## Interrupted restore cleanup

The engine's streamed restore records a cleanup job in the encrypted source store before exporting data into its private stage. The job contains the generated profile UUID, relative stage/upload/target names, source and parent directory identities, and owned directory identities. It contains no keys. Activation intent is committed before writing either new key. A detached restore retains the source profile lock until its filesystem work finishes, even when the caller cancels and the engine shuts down.

On source-profile startup, recovery checks recorded device/inode identities. A matching activated target with the expected profile manifest retains both new keys. An abandoned stage and upload are validated before any key deletion; cleanup uses retained directory handles, an exact bounded file allowlist, and no recursive traversal. Changed paths, unexpected contents or ambiguous activation retain the cleanup record for inspection. Key-store deletion failures retain the record for retry. Successfully removed directories are synced before retiring the record. Normal successful restore also requires explicit upload cleanup; a cleanup or fsync error after activation reports activation uncertainty and keeps the target and new keys for inspection/retry. Cleanup records are excluded from backup exports and cleared again during restore so another profile cannot inherit deletion authority.

The verified schema22 gate includes five subprocess crash boundaries: owned stage before export, completed stage, first key, second key and activated target. All12 local gate commands pass, including44 migration SIGKILL cases and independent Google/IMAP regressions. Directory creation before its identity can be recorded, earlier upload/export artifact lifetimes, and the low-level caller-owned `Engine::restore_profile` helper are separate remaining checks; this is not a claim that every maintenance crash window is covered. Unrecognized directories are retained, never removed merely because their names start with `.restore-` or `.maintenance-`.

Failed restores do not require restarting the source daemon before correcting the
passphrase. The next valid restore request first retires prior owned cleanup jobs
while retaining exclusive maintenance admission. Uncertain cleanup still fails
closed; it does not permit deletion of unrelated paths or original keys.

## Operating a restored profile

Keep the original daemon stopped before starting the restored profile. Its original queued work and credentials remain intact; running both copies could dispatch original work independently of the restored holds. Start the new daemon using the returned directory, then point the CLI at that directory and the new daemon's endpoint. The old bearer token cannot authenticate the new profile. Reconnect each account to its saved stable identity through the ordinary credential flow; the backup contains no provider tokens.

Inspect drafts and operation history before authorizing any new outbound work. Never treat absence in a provider search as proof of no earlier delivery. `operation resolve` supports the existing explicit confirmation, abandonment and duplicate-risk resend decisions; consult [RUNNING.md](RUNNING.md) for their provider-specific evidence requirements. Resend preserves the original uncertain operation and records a separate replacement operation with a new Message-ID. For the original operation, use `operation reconcile` as described below; ambiguous delivery or copying stays held.

On a lost restore acknowledgement, timeout or cancelled connection, the CLI reports `restore_outcome_uncertain` (exit 5) and the requested sibling name. Inspect the target before retrying; an existing target is preserved and returns `restore_target_exists`. `restore_cleanup_incomplete` identifies only the new profile UUID whose database/API key cleanup failed. Never delete the source profile's keys. Activation uncertainty preserves the new target and its keys.

## Reconciling an original operation

Read `operation show --account ACCOUNT_UUID --operation OPERATION_UUID` for the current version. Then request a provider observation with a new reconciliation request UUID:

```sh
nuncio-cli --json operation reconcile --account ACCOUNT_UUID --operation OPERATION_UUID --request-id REQUEST_UUID --version CURRENT_VERSION --wait
```

The request is durable and account-scoped. If the response is lost, repeat the identical command with its original request UUID, version and mode. A changed payload with that UUID conflicts. A later independent attempt uses a fresh UUID and the latest operation version. Each request has a bounded retry allowance; earlier explicit observations do not exhaust a later safe-resume request. Global attempt ordinals remain unchanged in the audit history. The original operation and its original write request ID remain unchanged. `operation show` includes the latest reconciliation request, its expected version, mode and first attempt ordinal. A later backup restore retains that request as history with `active=false`; it does not re-enable it. Actual running/held status is shown by the operation state and `needs_reconciliation`.

The default mode observes provider state and records evidence locally. Add `--resume-safe` only to permit safe continuation after the required positive evidence or version check. It does not authorize ambiguous SMTP delivery, IMAP copying or recreation of a missing restored calendar creation. An old “prepared” snapshot cannot establish what happened after the backup. A unique positive Gmail Sent observation can resolve the original send; a negative search leaves it uncertain. SMTP acceptance and Sent-copy evidence remain separate. For a restored SMTP send with a retained acceptance acknowledgement, explicit reconciliation can find a unique current Sent message without repeating delivery or APPEND. A client-managed Sent message must match the exact frozen bytes, including its private Bcc copy, and have a UID at or above the saved pre-DATA floor in the same mailbox epoch. Missing, duplicate, changed, or stale-epoch copies remain uncertain. An automatically held unknown APPEND requires an explicit reconciliation request for this search. Server-managed Sent uses its documented content fingerprint comparison. Existing confirmation, abandonment, and new-send decisions remain available.

`--wait` exits5 when the outcome remains uncertain or conflicts; inspect the operation and its attempts. A lost/unknown RPC outcome exits4 with `request_outcome_unknown`: it does not prove that admission failed. Disconnected accounts retain the admitted request until reconnection. Current schema21 send and Gmail/Calendar mutation system/subprocess tests cover these behaviors with independent delivery, write and notification observations. A Calendar notification outcome stays unknown after event state is observed; explicit abandonment records the decision and preserves that uncertainty without another write. Restored IMAP flag and COPYUID-based transfer continuation now has independent system/subprocess evidence. Queued transfer snapshots stayheld; a retainedCOPYUID permits observation and explicit UID-scoped source removal. Positive client-managed Sent discovery is verified in the schema21 follow-up system/subprocess gate; exact frozen bytes, UID floor/epoch and independent copy/delivery effects are checked.

## Rebuilding provider projections

Preview cached row counts and preserved local draft/operation history before starting a full repair:

```sh
nuncio-cli --data-dir /path/to/profile --endpoint http://127.0.0.1:9421 --json repair --account ACCOUNT_UUID --scope mail --dry-run
nuncio-cli --data-dir /path/to/profile --endpoint http://127.0.0.1:9421 --json repair --account ACCOUNT_UUID --scope mail --wait
nuncio-cli --data-dir /path/to/profile --endpoint http://127.0.0.1:9421 --json repair --account ACCOUNT_UUID --scope calendar --from 2026-10-01 --to 2026-11-01 --wait
```

Use the endpoint printed by your daemon. `--scope mail` supports Google and IMAP; `calendar` supports Google only. Calendar dates must be supplied together; omitting them uses the daemon's rolling window. Preview is entirely local, includes a database integrity check, and starts no sync run. Counts include retained cached rows, including retired collections/calendars. The reported revision is a snapshot, not a reservation; normal background work may advance state afterward. `--dry-run` and `--wait` cannot be combined.

An actual repair requires a connected account and reads the provider. Mail stages a complete mailbox projection and rebuilds its scoped search index. Calendar stages the entire discovered account catalog, full canonical events and provider-expanded occurrences for the requested window. Publication is atomic: failure or cancellation before publication preserves the old visible generation. Durable drafts, attachments, operation intent, attempts and receipts survive. Calendar permission changes invalidate coverage/cursors; unavailable event access retains cached events with unavailable coverage. Retired calendars retain their cache and lose active cursors. Repair never resolves or resends queued/uncertain outbound work. Already-enabled background writes can still dispatch according to their existing state.

Without `--wait`, retain the returned `run.id`. Inspect it with `system sync-status --account ACCOUNT_UUID --run RUN_UUID`, or request cancellation using `system cancel-sync` with those same arguments. Cancellation that arrives after publication cannot undo it. A crashed unfinished run becomes failed/interrupted on restart; rerun repair to fetch a fresh generation. Compatible in-progress full scans may be joined; an incompatible active scan reports busy. A corrupt/unreadable store returns a recovery error; repair does not replace the database or reset keys. Preserve originals and use a verified backup for new-profile recovery.

## Work still required

- Durable process-crash cleanup for backup/upload artifacts created before restore staging and retirement of ordinary failed restore jobs.
- Historical payload checks for other write kinds, disconnect retention/same-identity reconnection and broader storage/WAL/export failure coverage.
- Full security/resource/multi-engine acceptance, packages and extracted-artifact verification, and separately authorized live-provider acceptance.

Current local production executables are `target/production/release/{nunciod,nuncio-cli}`; the startup-cleanup gate records their hashes in `test-results/task13-startup-cleanup/artifacts.json`. The rebuild is checkpointed on its feature branch. These local artifacts and offline tests do not establish completed packaging, installation, live compatibility or remote CI results.
