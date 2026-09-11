# Implementation progress

## SMTP schema17 work in progress

Client SMTP is now dispatched through the engine/API worker. Schema17 captures SMTP endpoint/principal, Sent policy and local mailbox/UIDVALIDITY/UIDNEXT with frozen send intent. Durable phases prepared→started→accepted→appending→copied enforce no repeat DATA after uncertainty/acceptance and no repeat APPEND without proof. Complete negative DATA replies have a typed reset; generic rejection cannot reset started/accepted SMTP. Arbitrary sent_copy receipts can no longer mark a send applied; exact frozen Sent bytes, placement proof and positive mailbox observation publish atomically. Server Sent policy still returns visible uncertainty and requires implementation.

New locations: store/smtp{.rs,/progress.rs,/apply.rs}, smtp_schema.sql; providers/imap/{smtp/submission.rs,sent.rs}; accounts/imap.rs::smtp_session; operations/smtp.rs and worker readiness. SMTP validates envelope/CRLF/extensions and dot stuffing; strict APPEND collector preserves validated single UID/epoch proof, checks tags and bounds. Configured Sent folder must already be discovered. Production artifacts are still old/schema12; feature artifacts are being rebuilt.

Commands: focused storage compile initially101 due new unconsumed SMTP methods (WIP dead_code); those methods are now consumed with no suppressions. Unit run without escalation101 due denied localhost binds (1pass2fail); scoped localhost rerun `cargo test --locked -p nuncio-engine --lib providers::imap -- --nocapture` exit0,11pass/1filtered,0ignored. Updated `cargo test --locked -p nuncio-engine --test smtp_operation -- --nocapture` exit0,1pass, strengthening DATA marker, independent delivery/copy, foreign placement, byte mismatch rollback and reopen invariants. Focused full API `... --test imap_system smtp_submission -- --nocapture` exit0,1pass/13filtered,0ignored; independent Mailpit delivery and Dovecot Sent exact bytes/counts. Engine Clippy initially101 items_after_test_module; moved open_smtp before tests; rerun exit0.

Currently actual `python3 scripts/verify.py --suite imap_e2e` running shell22349, fresh feature build and six SMTP process-death boundaries with Bcc/binary MIME checks. Added system lost DATA/APPEND ACK and explicit pre-DATA rejection regression, not yet run. Storage negative DATA transitions and server-Sent support still need focused coverage. No full-current gate claim.

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
- [ ] 12 Synology IMAP/SMTP — full local independent mocks and mail reads/flags/archive/move/copy/trash/restore verified offline; latest schema16 gate9commands0 (engine69/proto-cli4/IMAPsystem13/E2E4/Google21+26). SMTP delivery and independent Sent-copy bookkeeping next.
- [ ] 13 Backup/restore/export/repair
- [ ] 14 Adversarial acceptance and bounds
- [ ] 15 Local production artifacts and CI
- [ ] 16 Separately authorized provider acceptance and final matrix

Follow the detailed plan at `../docs/superpowers/plans/2026-09-10-google-first-rebuild.md`. None of R01–R16 is complete yet.

Nine-command Trash/restore gate complete: `test-results/task12-trash/results.json` all9commands0. Rustfmt, normal and feature workspaceClippy, proto-cli4, engine69, IMAPsystem13, actualIMAPE2E4, Googlesystem21, actualGoogleE2E26. Zero failed/ignored. Fresh feature builds and copied nested suite logs/results retained. Native and hidden-MOVE fallback Trash/restore both pass, including before-publication SIGKILL, no-op and full-resync origin retention. Source/test schema16; production artifacts still Task11/schema12. No process running; no live compatibility or remote CI claim. SMTP is next.

Next: establish actual draft→SMTP→Sent system regression against independent services, then implement the durable submission/Sent phases and subprocess crash tests. Accepted SMTP must never be resent for APPEND failure; unknown outcomes cannot be blindly retried. See SESSION-STATE for exact locations and remaining defects. Historical intermediate TODO entries preserved in history/2026-09-11-transfer-todo-history.md.


SMTP work in progress, newer than schema16 gate: added full system regression `imap_write_system/smtp.rs` (client-Sent policy). First compile101 unavailable hex helper; replaced with standard hash formatting. Focused rerun101 reaches durable queued send and times out because IMAP send dispatch is disabled. Test independently requires SMTP DATA count/envelope/wire hash, actual Mailpit capture, Dovecot Sent bytes/count, local Sent projection and separate smtp_accepted/sent_copy receipts. Do not weaken it or claim all-current-tests-green.

New bounded SMTP component `providers/imap/smtp/submission.rs` with typestate Session→Prepared: envelope/RCPT/DATA354 first without content, caller must commit a dispatch marker, then dot-stuffed bounded64KiB DATA write and complete final2xx/4xx/5xx classification; missing/malformed ACK unknown. Canonical CRLF/1000-byte lines, payload bounds, SMTPUTF8 checks for every parsed MIME header including nested messages, 8BITMIME for body. Existing auth now returns a Session internally and probe still reports capability tuple without sending. New submission_tests initially compile101 missing types, implementation unit run pending exec16098. Production caller/journal integration not yet written; normal build may report unused submission methods until wired. Full product SMTP system remains intentionally red.

Next immediately: poll16098/fix component failures; implement schema17 immutable SMTP/Sent intent + durable phases and worker consumer. Preserve separate SMTP accepted and APPEND proof. Prepared(before DATA bytes) can retry after crash; Started without SMTP receipt stays unknown; accepted SMTP never repeats for Sent failure; SentStarted without APPEND proof never blindly repeats; known APPENDUID permits positive target observation/publication. Add negative-reply evidence to reset only proven unaccepted SMTP, and independently test those invariants. Server AutoSent and broader E2E remain required, not waived.

Primary references read September11: RFC5321 sections4.2.5/4.5.2 (2xx after terminator accepts responsibility;4xx/5xx prohibit subsequent delivery; add one dot at each line start), RFC6531 section3.2 (SMTPUTF8 required for internationalized envelope or headers at any MIME depth), RFC4315 APPENDUID/UIDNOTSTICKY. URLs https://www.rfc-editor.org/rfc/rfc5321.html , https://www.rfc-editor.org/rfc/rfc6531.html , https://www.rfc-editor.org/rfc/rfc4315.html . Missing APPENDUID is not acceptance proof of identity; no guessed repeated copies.
