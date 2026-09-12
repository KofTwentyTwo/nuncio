# Current delivery and acceptance state

## Active account-management follow-up

- [x] Post-engine PO/PM roadmap delivered in POST-ENGINE-ROADMAP.md; future scope only.

- [x] AM01–AM02 account details/editing and usable Google/IMAP add/reauth flows.
- [x] AM03–AM05 pause/resume, archive/restore and separately confirmed permanent deletion.
- [x] AM06–AM07 concurrency, recovery, migration and engine/API/CLI parity.
- [ ] AM08 independent system/subprocess checks, full gate, fresh artifacts and alpha handoff.
- [x] API publication and semantic-version research/plan delivered; proposed tooling and publication are separate future work.
- [x] All 11 open PRs reviewed; relevant fixes tested and explicit later merge/closure dispositions recorded in OPEN-PR-REPORT.md. Follow-up lock/label checkpoint and hosted rerun remain pending.
- [ ] Dependency remediation implemented and checked (790 tests and fresh-index audit); checkpoint the final yanked-lock fix and verify corrected hosted scan. Default branch/PR closure requires authorized integration.
- [ ] Development security CI: Rust/Python/Actions/JS CodeQL and development/PR triggers, advisory coverage, actual hosted verification; remote protections unchanged.
- [ ] Current README and operating/API/implementation docs: delegated maintenance, with root providing final gate/artifact facts.
- [ ] Repo-run prebuilt testing installer: delegated by James to `testing_installer`; root must integrate its tested script/workflow with fresh verified artifacts and authorized checkpoint push.

All account commands are now implemented through engine/API/CLI. Focused Google/IMAP system and actual subprocess checks, account crash boundaries, encrypted storage and recovery checks pass. The full updated 34-command gate also passed, including schema23 migration, 196 authorization cases and old-wire compatibility. AM08 remains open for fresh artifacts, hosted verification, installer download and final handoff. Exact commands and failures are in VERIFICATION and test-results/account-management.

The earlier completed implementation below predates this newly authorized scope. Follow ACCOUNT-MANAGEMENT-PLAN.md; do not use the old package as evidence for new account-management changes.

- [x] Tasks 01–14 software implementation and independent mock/system/actual daemon-CLI coverage, including mail/calendar writes, recovery, security, and resource bounds.
- [x] Full 34-command offline gate passed on 6ff9bb9: 308 tests per workspace configuration, all separate/Python/dependency/client checks passed, zero failed/ignored, 399 source hashes unchanged.
- [x] Signed software checkpoint 6ff9bb9 pushed; signature and exact remote head verified.
- [x] Fresh clean production package pair passed: identical b26d2c93 archives/binaries, 22 extracted checks and 458 manifest entries each, 237 notices; prior artifacts preserved.
- [x] Actual hosted run 34684158773 passed all ten jobs; every job artifact was downloaded and checked. Prior failures remain recorded.
- [x] Final implementation report, R01–R16 matrix, operating/recovery/compatibility guides, and manual worksheet prepared with actual evidence and precise limits.
- [ ] Named Google live acceptance G01–G10: deferred, unapproved, unverified.
- [ ] Named Synology/MailPlus acceptance S01–S05: deferred, unapproved, unverified.
- [ ] Native-keystore behavior and final approved cleanup/sign-off X01: unverified; full goal remains incomplete.

Documentation delivery receipts and any resulting CI handle are in
`test-results/checkpoint-final/`; [SESSION-STATE.md](SESSION-STATE.md) gives the
conditional next action. If the receipt already records the documentation push,
do not create another equivalent checkpoint. Report-only changes do not require
another production package. Continue existing verification, then honor the live
deferral and blocked-state rules. Native apps, merge/release/installation, and
remote settings changes are outside the authorization.

## Earlier task ledger

# Rebuild TODO

Full unbudgeted goal is active; inline execution; full offline Google/Synology mocks now. James authorized the initial checkpoint and regular commits/pushes to `feature/nuncio-google-first-rebuild` after relevant checks pass. Merges, releases, installation, remote settings and live acceptance remain unapproved. The executive report email was separately authorized and sent. Exact state, command statuses, paths and next action are in SESSION-STATE.md; dated evidence in VERIFICATION.md.

- [x] Google mail/calendar engine/API/CLI writes and independent mocks, system/E2E at prior checkpoints.
- [x] IMAP full/delta/fetch/flags, explicit folder copy/move/archive, Trash/restore, exact placement and safe UID-scoped expunge.
- [x] Client SMTP/Sent with independent delivery/copy receipts, immutable wire/privateBcc, seven process-death cases and negative APPEND copy-only retry.
- [x] Server Sent unique positive read, fresh durable pre-DATA UID floor, versioned content comparison, separate receipts; storage4 and actual IMAPE2E6 pass. Strict SEARCH10cases pass in1test.
- [x] Schema18 server Sent full12-command offline gate: engine78/proto4/IMAPcontract2(system18Python)/IMAPsystem17/E2E6/Google21+26, allzero failed/ignored. IndependentAutoSent tag ownership bug fixed using committed authenticatedUsername.
- [x] Explicit SMTP resend captures new immutable SMTP/Sent intent atomically; fullengine79/IMAPsystem18/Googleoperations6/GoogleE2E26 and bothClippy pass. ActualIMAPE2E7 independently passed; bothSentpolicies, deleted drafts, binary/frozenbody, duplicate-risk and restart evidence.
- [x] SMTP manual confirmation and abandonment preserve transport history and independent remote effects. The nine-command gate passed: engine 80; proto/CLI 8; API 19; actual IMAP E2E 8; Google suites 6, 21 and 26. Both Sent policies are verified.
- [x] Missing-UIDPLUS client/server Sent policies and queued SMTP endpoint-change profiles: all5gatecommands0, IMAPsystem21/actualCLI9.
- [x] Task12 provider paths and strict independent gates verified offline. Cross-cutting legacy-schema, resource and adversarial coverage continues under Tasks13–14.
- [x] Task 13 projection repair through engine/API/CLI, with independent Google/IMAP system and subprocess checks, transactional rollback and queued-operation retention. Preceding 13-command gate passes.
- [x] Historical schemas 1–20 migrate and restore with durable payloads; old backup-count bug fixed. Damaged-store API/CLI refusal preserves originals. Eight-command gate passes: full engine 105, proto/CLI/daemon 19, repair E2E 2, recovery E2E 1, IMAP E2E 11; zero failed/ignored.
- [x] Later historical suite: four tests pass, including injected failures at all 19 upgrade boundaries, followed by fmt/both Clippy. Exact original encrypted bytes/catalog/rows survive each failure and retry succeeds.
- [x] Actual daemon SIGKILL before/after every migration commit: all40 cases and complete eight-command gate passed; fresh schema20 production binaries exclude test controls.
- [x] Rich send history across schemas10–20: all operation states and explicit decisions, positive receipts/SMTP state, retained restoration provenance and dispatch holds. Four-command gate6historicaltests passed.
- [x] Schema21 durable reconciliation through engine/API/CLI, scoped request replay, restoration guards and positive/negative send recovery after lost acknowledgements/SIGKILL. All14 gatecommands pass; fresh production isolation verified.
- [x] Restored Gmail/calendar mutations: 28 system scenarios and3 actual CLI crash cases, explicit-request retry fix, stable attempt ordinals, notification uncertainty/abandonment. Full13-command gate passes and production rebuilt.
- [x] Restored IMAP writes:28 system scenarios and2 actualCLI crash cases, independent bytes/copy/flag/expunge/delivery evidence. Six-command gate all0: system24/actualCLI12/Google reconciliation3 and bothClippy.
- [x] Positive client-Sent/restored accepted-SMTP recovery: full13-command offline gate passes, engine112/IMAPsystem26/actualCLI13; separate proof, atomic publication,12storagecases/7systemscenarios/2newCLIcrashcases. Production artifacts refreshed.
- [x] Restore path identity: replacement regression reproduced and fixed; six path cases and engine key rollback verified. All7 gatecommands0 (8restore/3Google recovery/13IMAP/2release), production refreshed.
- [x] Schema 22 streamed-restore crash cleanup: all 12 commands passed. Engine 119; API 20; recovery E2E 4 (five restore crash points); 44 migration SIGKILL cases; Google 21/26; IMAP 26/13; production isolation 2. Encrypted ownership journal, owned key/stage/upload cleanup, activated-key retention, backup authority exclusion and cleanup-failure reporting are verified. Production refreshed.
- [x] Maintenance artifact profile leases: all7 gatecommands0, engine122/recoveryE2E4/IMAPE2E13/release2, three new lifecycle regressions; production refreshed.
- [x] Startup cleanup cancellation and relative restore: all7 gatecommands0, engine126/recoveryE2E4/IMAPE2E13/release2 and fmt/bothClippy. Original profile ownership, relative/aliased paths, independent restored keys, symlink and cross-owner rejection verified. Production refreshed.
- [x] Task13 restore-retry fix: full7-command gate passed; engine128, actual recoveryE2E5, IMAPE2E13, release-isolation2. Signed checkpoint57dd648 pushed and exact remote hash verified. Task14 is now underway. Further hardening requires a demonstrated R01–R16 failure; reconcile older TODO items with existing evidence before reopening work.
- [x] Task13 original-operation reconciliation, owned stage/new-key cleanup and activation races have existing system/subprocess evidence. Historical schemas, backup WAL consistency and repair preservation are covered by the named suites in VERIFICATION. Retain precise limitations; avoid treating completed work or speculative audit ideas as new prerequisites.
- [x] Task14 multi_engine_system and shared-harness gate: all6 commands passed (shell43253); multi-engine1, Google21, operations6, formatting/bothClippy. No production/dependency changes.
- [x] Task14 security suites: security_system3 and actual securityE2E2 pass, including all42 RPCs/168 invalid-auth cases, pre-redaction canary/encryption checks and independently counted hostile-HTML resource traps. Relevant9-command gate and strengthened4-command follow-up pass; no production change.
- [x] Task14 resource workloads:10,000 provider/API-paginated messages; configured body refusal; actual8-cycle16MiB attachment/RSS checks and64MiB+1 input refusal. All6 relevant gatecommands0 (resource system2/E2E2/securityE2E2 plusfmt/bothClippy).
- [x] Task14 account-request admission/memory fix:14-command gate98949 passed with original RSS bound; new worker saturation failure60230 reproduced/fixed and focused49015 plus9-command81153 gate passed. Engine129/resource_system4/operations6/GoogleE2E26/IMAPE2E13/release2; fresh artifacts verified. Signed checkpoint a29812f pushed; exact remote hash/signature verified.
- [x] Task14 shared job/network limits and numeric resource status through engine/API/CLI. Budget unit91489, actual resource-status85084 and mixed Google/Dovecot79577 pass;15-command gate57763 all0: engine130, API/CLI20, resources4/3, security3/2, Google21/26, operations6, IMAP27/13, release2 plusfmt/bothClippy. Counter/page/backoff/Synology assertions and fresh production artifacts verified; checkpoint next.
- [x] Task14 final gate: all25 commands passed92959/0,304 tests per workspace run; preserved task14-all evidence. Historical context: resource instrumentation/security/convergence verified. Limited-writer Calendar fix passes independent mock26, engine4, API1 and actualCLI1; full --all92959 passed. Preserve all original assertions and provider observations.
- [x] Task03/15: egress denial, fresh production packages, frozen contract/external client, and actual CI verified on 6ff9bb9; see the current delivery evidence.
- [x] Task15 dependency findings resolved: removed rustls-pemfile/derivative/instant paths, reviewed0BSD/CDLA notices; cargo-deny0errors/warnings and8-command affected gate pass. Original99059/5 retained.
- [x] Task16 documentation: manual worksheet, final matrix and operating/recovery/compatibility guides prepared. Task16 itself remains incomplete: named live/native checks are the unchecked items in the current section above.

The matrix distinguishes passed offline checks from pending hosted/live acceptance; the full goal remains incomplete. Original approved plan remains authoritative. A provider milestone is progress, not completion.
