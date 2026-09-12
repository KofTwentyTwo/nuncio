# Account management before laptop alpha

James authorized design and implementation on September 12, 2026, before alpha distribution. His laptop is Apple Silicon macOS. Removal archives by default; permanent local deletion is a separate explicit option. Continue inline on the existing feature branch. Existing commit/push authorization applies after relevant checks; publication, installation and live account actions still require separate authorization.

## Intended behavior

- **AM01 — Inspect and edit:** account show/list expose a bounded display name, stable identity, credential/lifecycle state and configuration version. Version-checked edits prevent stale clients overwriting changes. Provider address/identity is not a user-editable Google identity. IMAP public settings can be edited and probed before atomic publication; changing the IMAP host/port/TLS/username principal requires a new account so unrelated server data cannot inherit old UID/operation identity.
- **AM02 — Add and authenticate:** clear Google/IMAP add and reauthentication commands, retained compatibility with existing connect commands, bounded wait for browser consent with terminal success/failure, and explicit auth-session cancellation. Passwords remain protected stdin input; Google registration remains a private local file. No fake connected state or bundled credentials.
- **AM03 — Pause/resume:** durable local pause retains credentials and cached data, prevents scheduled/manual provider work and operation dispatch, and survives restart. Resume retains provider backoff and outstanding operation uncertainty. Credential authentication state and user pause are separate.
- **AM04 — Archive/restore:** default removal archives the entry, disconnects credentials through durable cleanup, hides it from ordinary lists, and preserves cached mail/calendar, drafts and operation evidence. Include-archived listing and explicit restore make recovery discoverable. Reauthentication alone must not silently unarchive an entry.
- **AM05 — Permanent local deletion:** preview affected account data and unresolved operations; require an already archived account and explicit account-ID confirmation. Preserve all other accounts and global profile keys. Refuse to erase unresolved remote-effect evidence until explicitly resolved. Delete the current profile's account rows/content transactionally and leave durable credential cleanup retryable across failure/restart. Existing exports/backups and remote provider objects are outside deletion; logical deletion is not a secure-erasure claim.
- **AM06 — Concurrency and recovery:** account lifecycle changes coordinate with in-flight provider work; stale OAuth completions cannot revive archived/deleted accounts. Queued jobs and uploads must recheck account existence/lifecycle at publication. Local change replay remains valid after deletion, and schema migration/backup/restore preserve the new metadata.
- **AM07 — API/CLI parity:** authenticated additive v2 RPCs expose every operation. CLI provides clear text/JSON outcomes, nonzero failures, confirmation/preview for purge, and no credentials in errors/logs. Existing commands and generated clients remain compatible.
- **AM08 — Evidence and delivery:** independent Google and strict IMAP/SMTP system tests plus actual daemon/CLI E2E independently check zero unexpected provider effects, account isolation, cancellation, retained data, deletion and crashes. Extend migration/security coverage, refresh descriptor and production artifact, run relevant/full gates, update reports and signed checkpoints. First live OAuth/native-keystore acceptance remains separate.

## Implementation sequence

1. [x] Capture an actual CLI regression for missing account show/edit/lifecycle behavior (actual CLI exits 2 at `account show`; runner exits 101).
2. [x] Add schema 23 metadata, transactional lifecycle/edit/preview/purge storage and focused SQLCipher tests.
3. [x] Add engine coordination, edit/probe/credential handling, archive/restore and auth cancellation.
4. [x] Extend authenticated v2 account API and CLI, including add/reauth/wait UX and retained aliases.
5. [x] Add independent system and subprocess tests for both providers, queued/uncertain work, crashes, isolation and credential cleanup failures.
6. [x] Extend historical migration/restore, RPC-auth coverage and frozen descriptor verification; run formatting, both Clippy modes and affected suites.
7. [x] Run full offline gate, build and verify fresh production packages, checkpoint/push after checks, retain actual CI results and refresh alpha handoff.

Use existing dependencies and durability patterns. Do not expand into native apps, OAuth-console automation, provider grant revocation, public release infrastructure or unrelated audits.
