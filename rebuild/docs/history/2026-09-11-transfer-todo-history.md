# Implementation progress

- [x] 01 Encrypted workspace, store, daemon, CLI lifecycle — verified
- [x] 02 Independent stateful mock Google — verified
- [ ] 03 System/subprocess E2E harnesses — runtime verified; CI egress denial pending with 12/15
- [x] 04 Google OAuth and accounts — offline verified
- [x] 05 Gmail initial sync, MIME, queries — offline verified; normal/feature gates 59 tests each
- [x] 06 Gmail delta and crash reconciliation — offline verified, including prepared draft/queued operation after source deletion
- [x] 07 Google Calendar sync and agenda — offline verified; normal/feature gates 70 each
- [x] 08 Background sync and change stream — offline verified, including operation/resolution transitions in system and actual CLI
- [ ] 09 Drafts and operation journal — draft CRUD/uploads/reply/forward verified in system + actual E2E; drafts and frozen send enqueue/get/cancel verified through system + actual E2E; list/history verified; audited resolution storage and API/CLI guards implemented; Google send/recovery/resolution verified in actual E2E; complete typed mutation intents and full gates pending
- [x] 10 Gmail send and mutations — offline verified; task10 full gate12 and action-version follow-up11 commands all exit0
- [x] 11 Calendar writes and free/busy — offline verified; full12-command gate0; normal/feature142 each; independent mock25/system21/E2E26/operation6/release1
- [ ] 12 Synology IMAP/SMTP — active; independent Dovecot/Mailpit TLS services + controls + standalone process + Rust harness; Python17 pass; prior nine-command gate and Google25/21/26 pass; final six-command mock follow-up passes (Rust2 wraps Python17); account config/TLS/auth/storage/API/CLI implemented; account/transport/completion gates pass (system2/actualE2E1); schema14 staged mailbox/placement projection3 tests pass; network full/delta and actual CLI reads/three crash boundaries implemented; eleven-command read gate all0 (engine64/IMAPsystem5/actualE2E1/Google21+26); explicit fetch/sparse UID paging seven-command gate all0 (engine66/IMAPsystem6/actualE2E1/Google21); read/star journal and dispatcher implemented with focused system2, storage1 and actual CLI three flag-crash cases passing; eight-command flags gate all0 (engine67/IMAPsystem8/actualIMAPE2E1/Google21+26); independent native MOVE/COPYUID contract added, five-command follow-up all0 (Rust2 wraps Python18); schema15 Archive executor/projection/native and fallback system1passed, no-op IDs preserved, COPYUID protocol1passed, subprocess archive1passed after stale barrier fix; five new transfer crash boundaries pass actualE2E2, lost-ACK/unsupported system regression1passes across3profiles; eight-command archive gate all0; explicit move/copy/API/CLI E2E3/system1 pass, nine-command folder gate running exec86339; then explicit folder transfers/trash/restore/SMTP
- [ ] 13 Backup/restore/export/repair
- [ ] 14 Adversarial acceptance and bounds
- [ ] 15 Local production artifacts and CI
- [ ] 16 Separately authorized provider acceptance and final matrix

Follow the detailed plan at `../../docs/superpowers/plans/2026-09-10-google-first-rebuild.md`. None of R01–R16 is complete yet.

Current transfer progress and latest evidence supersede historical entries below the checklist: see SESSION-STATE immediate action. Schema15 Archive is wired; full Task12 and goal remain incomplete.


Eight-command archive gate complete: `test-results/task12-archive/results.json`, all8commands exit0. Rustfmt/both workspaceClippy, engine69, IMAPsystem10, actualIMAPE2E2, Googlesystem21, actualGoogleE2E26; zero failed/ignored. Fresh feature builds and copied nested logs/results retained; test counts overlap. Source/test schema15; production artifacts still Task11/schema12. No live compatibility or remote CI claim. No running process. Next explicit move/copy API/CLI using captured destination collection identity, then trash/restore origin metadata and SMTP.


Explicit folder move/copy now implemented after actual CLI red101 (2existingpassed/1newfailed, unsupported action exit2). Added MailAction Move/Copy and additive proto oneof fields9/10 with MailFolderChange; daemon/CLI mapping, account-scoped captured destination UUID/epoch and IMAP move_copy capability. Gmail explicitly rejects folder actions. Schema remains15; existing Archive/flag intents unchanged. Capture shares the proven transfer executor/projection. New actual E2E `imap_folder_e2e.rs` proves copy to quoted/Unicode/backslash folder, copy in same folder with distinct placements, explicit move back and same-folder no-op, exact request replay/raw bytes and remote COPY2/MOVE1. Fresh `python3 scripts/verify.py --suite imap_e2e` build0/suite0,3passed. Separate system `folder_actions`1passed0 proves foreign/empty/missing destination rejection, missing remote folder conflict and zero COPY/MOVE/STORE/EXPUNGE. Feature workspaceClippy0. Nine-command full folder gate running exec86339; poll before changing source. No full-current-gate claim until result. Next trash/restore with original folder metadata, then SMTP and Tasks13–16.


Next concrete Task12 implementation after folder gate: add a subprocess test that trashes a placement from a non-INBOX folder, restarts/full-syncs, then restores it to the original folder with identical bytes and exactly two remote moves. Cover an already-Trash no-op, unknown origin (external trash), missing/retired original folder, UIDVALIDITY changes and crashes before local publication. Store original folder identity/epoch separately from reconstructible message content, scoped to the exact trashed placement; never infer an origin from Message-ID or silently choose INBOX. Extend captured transfer intent and atomically publish origin metadata with the receipt, using a schema migration if needed. Existing Trash(true/false) proto/CLI fields already exist for Gmail. Explicit Move provides a destination when origin metadata is unavailable. Native/fallback/unknown-ACK rules remain unchanged.


Nine-command folder gate complete: `test-results/task12-folders/results.json` all9commands0; Rustfmt/bothClippy, proto-cli4, engine69, IMAPsystem11, actualIMAPE2E3, Googlesystem21, actualGoogleE2E26. No failed/ignored. Fresh feature builds and copied nested evidence retained. Schema15 source/test; production remains Task11/schema12. No running process. Next trash/restore with original placement metadata, then SMTP.


Newest Task12: schema16 Trash/restore implemented. Native subprocess E2E4passed0; separate origin-negative system1passed across3profiles and copy-origin inheritance system1passed0; featureClippy0 after adding new optional field to test fixture. Actual Trash test now adds hidden-MOVE fallback; full9-command trash gate pending. SMTP production submission and Sent bookkeeping are next. Prior task12 entries are chronological evidence, not current blockers.
