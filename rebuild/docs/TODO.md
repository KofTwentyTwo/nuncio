# Current release-preparation actions

- [x] Diagnose repeated hosted IMAP cursor failure from actual mailbox-state evidence; optional interest-hint regression red44102/101, green50258/0 (all7projection tests). Preserve original assertions and stored provider hints.
- [x] Full34-command offline gate88082 passed:308tests per workspace, allseparate/Python/dependency/client checks0,399source hashes unchanged; original IMAP/resource assertions retained.
- [ ] After full pass, checkpoint/push the cursor correction, verify a fresh clean production package pair and actual hosted run; prior bfc7c8e artifact predates this fix.
- [x] Collect terminal34681263133 artifacts (9jobs passed/1IMAP failure); bothlint154tests/8commands0, bothrelease8commands0/isolation2/external1/package0. Prior failures retained; no hosted archive hash claim.


- [x] Calendar limited-writer fix: full25-command gate passed, checkpoint720081f signed/pushed/verified.
- [x] Dependency upgrade and affected8-command gate passed; cargo-deny errors/warnings0.
- [x] Frozen v2 descriptor and independent generated-client smoke verified; normal dependency boundaries checked.
- [x] Clean local archive80508 verified with22 extracted-binary checks and237 third-party notices; no installation/release.
- [x] macOS egress probes/client smoke and Linux namespace0/17/counter/cleanup checks passed.
- [x] Fixed pinned ingress relay: independent internal-only topology/TLS checks and all6 Python service scripts pass under egress denial. Strict Rust suites run in the full gate.
- [x] Independent Python suite and six CI job groups integrated; YAML/binding/failure-propagation checks pass.
- [x] Native numeric resource sampler passes macOS/Linux allocation tests; actual3resourceE2E pass under egress denial with original bounds/timeouts.
- [x] Complete34-command integrated egress gate58495 passed;305tests in each workspace, allnamed/Python/dependency/client checks0;396source hashes unchanged. Original98403 retained.
- [x] Packaged README fixed; two corrected fresh archives/binaries are byte-identical,22extracted checks each, all links valid. Canonical dist/final-candidate and exact proof recorded.
- [x] Operating/recovery/package/compatibility docs and working R01–R16 matrix recorded; signed7f81b73 checkpoint pushed/exact remote verified.
- [x] Affected gate91079: all6outer/17nested commands passed; all56 subprocess tests passed, no failures/ignored. Production244files unchanged.
- [x] Signed resource-fixture checkpoint2e4d30e pushed; exact remote head verified.
- [x] Retain terminal hosted34672641999 failure:4macOS passes/6Linux control-connection failures; same-user controller regression reproduces cause.
- [x] Verify cgroup egress fix locally, strict denial/controller continuity/exit/cleanup, script/style/workflow gates; signed1a0d954 checkpoint pushed/exact remote verified.
- [x] Reproduce and correct hosted10k status starvation with one transactional FTS reset; focused red/green and full34-command gate91408 pass with307tests per workspace and all399source hashes unchanged. Original deadlines/workloads/assertions retained.
- [x] Signed6a324f9 promotion correction pushed/exact remote verified; clean package pair14220 passed22checks each with identical bfc7c8e archives and unchanged inputs. Prior failures/artifacts retained.
- [x] Retain actual hosted34679663120 failures and prepare numeric phase/mailbox-state diagnostics; focused Linux/macOS/IMAP comparisons and seven-command affected gate92835 pass without deadline/assertion changes.
- [x] Signed275eb42 diagnostic checkpoint pushed/exact remote verified after seven-command affected gate.
- [x] Observe terminal hosted34681263133 (9passed/1failed): cursor cause confirmed and focused correction passes; full gate remains above. Prior Linux transfer timeout cause remains unexplained.
- [x] Final guide correction and refreshed local candidate45789 passed two identical fresh builds/22checks each; selected8996085 archive and prior candidates preserved. No product/package-code changes.
- [x] Implementation report, requirement matrix, artifact hashes and operating/recovery limits prepared;17doc links/fences and whitespace pass. Update hosted results when available.
- [ ] Live acceptance remains deferred/unapproved/unverified; goal stays active and incomplete.

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
- [ ] Task03/15: explicit test egress denial, fresh production artifacts/packages, contract/CLI external-client verification and CI files.
- [x] Task15 dependency findings resolved: removed rustls-pemfile/derivative/instant paths, reviewed0BSD/CDLA notices; cargo-deny0errors/warnings and8-command affected gate pass. Original99059/5 retained.
- [ ] Task16: manual acceptance worksheet prepared (allliveunapproved/unverified); final requirement-to-evidence matrix, operating/recovery/compatibility docs; named live actions only after separate authorization when user resumes live scope.

The matrix distinguishes passed offline checks from pending hosted/live acceptance; the full goal remains incomplete. Original approved plan remains authoritative. A provider milestone is progress, not completion.
