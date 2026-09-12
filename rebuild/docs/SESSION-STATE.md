# Rebuild session state

Updated September 12, 2026. Full Tasks01–16/R01–R16 goal is active and unbudgeted. Continue inline from this worktree; do not recreate the goal or restart planning. Full local Google and Synology mocks are the current provider scope. Live acceptance remains deferred/unapproved/unverified; it does not block remaining offline work. Native apps are excluded.


## Authorization and workspace

Worktree: `/Users/james.maes/Git.Local/KofTwentyTwo/nuncio/.worktrees/nuncio-google-first-rebuild`; branch `feature/nuncio-google-first-rebuild`, baseline `a268fd5a004977b65fd0347ceeb1c69755529aa3`. Cargo cwd: its `rebuild/`. Original checkout remains on dev; original source/data/unrelated changes are preserved. James explicitly authorized an initial checkpoint commit and regular commits/pushes after coherent changes pass relevant checks, on feature/nuncio-google-first-rebuild at github.com/KofTwentyTwo/nuncio. Initial signed checkpoint `2480bf94cdcff15fcf0da886691bd872e38edf31` was committed and pushed; git ls-remote verified the identical GitHub branch head. Gitleaks staged scan, whitespace check and byte equality for all389 staged files passed. Details are in VERIFICATION and test-results/checkpoint-initial/. Merges, releases, installation into the normal environment, remote settings changes, and live acceptance actions remain unapproved. The executive report was separately emailed to james@kof22.com using connected Gmail on explicit request; this is not rebuild-provider acceptance evidence.

Read AGENTS→CLAUDE, shared personal rules/style, rebuild/AGENTS, this file, TODO and newest VERIFICATION entries after reset. Approved spec/plan and September10 engineering audit live in the worktree's `docs/`, one above Cargo cwd. User instructions override issue/replanning/delegation/commit defaults.

## Current execution — September 12, 2026, 08:44 UTC

- **Active unbudgeted goal:** full Tasks01–16/R01–R16, inline; engine/API/CLI, native apps excluded. Full local mocks remain provider scope. Google/Synology/native-keystore acceptance is deferred/unapproved/unverified; do not ask for accounts now.
- **Cursor correction full gate complete:** shell88082 exited0; all34commands passed,308tests in each workspace configuration, zero failed/ignored, all399captured source files unchanged. All separately named/Python/dependency/client checks0. Exact commands/counts/logs: `test-results/ci-imap-interest-all/{final-summary.json,exit.json,source-verification.json,egress.json,current-all/}`. No local verifier remains active.
- **Confirmed cause and fix:** hosted34681263133 empty-folder snapshots differed only by optional Unmarked LIST hints. `engine/src/store/imap_mailboxes/promotion.rs` excludes only case-insensitive Marked/Unmarked from coverage hashing while retaining full provider state. Hash domainv2 causes a one-time opaque local token change on next promotion; no schema/API/remote-cursor change. RFC9051section7.3.1 rationale is documented. Existing assertions/deadlines are intact.
- **Regression proof:** new `engine/tests/imap_projection.rs` test failed44102/101 before correction; all7projection tests passed50258/0 afterward. Original IMAPsystem27/actualE2E13 also passed in both workspace configurations and the separate gate. Real role/delimiter/UIDNEXT/HIGHESTMODSEQ/UIDVALIDITY changes still alter coverage; stored hints and folder identity are preserved.
- **Resource proof:** original10k normal/feature/separate sync12,675/12,950/12,526ms; list1,509/1,508/1,497ms, each100API/remote pages. Original8x16MiB and64MiB+1refusal checks passed unchanged limits. No universal speed or live-compatibility claim.
- **Latest completed hosted run:** [34681263133](https://github.com/KofTwentyTwo/nuncio/actions/runs/34681263133) on signed275eb42 ended9passed/1IMAP failure, before this production correction. All10job artifacts retained. Bothlint154tests/8commands0; bothrelease8commands0/isolation2/external1/package0; bothE2E40, mocks26each, Linuxsystem37. Earlier transfer timeout did not recur but its cause remains unexplained. No watcher is active.
- **Next concrete action:** signed cursor correction checkpoint/push after final documentation/secret/whitespace checks; verify signature/exact remote head. Then run prepared `test-results/task16-interest-package/run.py` for a fresh clean package pair and observe the resulting actual hosted run. `test-results/checkpoint-interest/current-execution.json`, package active.json and remote-ci receipts carry later handles/head during package input freeze. Read those if present before starting anything again.
- **Preserve prior verified artifact:** clean6a324f9 package pair14220 passed22checks each, archive SHA256bfc7c8e70d8a0e68bd843a811e7dc9a49018f052c204fa4128991ab695ff1784. It predates the cursor correction. Keep it and prior receipts; select a fresh candidate only after actual pair/extracted verification. Freeze all tracked inputs, including docs, during packaging; ignored sidecars may record current work.
- **Hourly report:**02:57Central SENT1a094a25abd21306 independently verified with183972-byte inline16-task PNG. Next08:57UTC/03:57Central. Exact body/chart/payload/receipt in status-emails/2026-09-12-0257*. Current estimate2–5offline active hours plus3–6deferredlive; the correction/full regression is now complete.
- **Goal incomplete, progressing:** remaining offline work is checkpoint/fresh artifacts/actual hosted verification/final report. No blocked audit while these concrete steps remain. No merge/release/install/live actions; preserve original checkout and unrelated work.

## September 12 chronology — historical snapshots

Entries below record earlier observations; the current execution section above governs the next action.

Final CI-fix preflight: configured Ruff check/fmt24files, actionlint and whitespace
all0;13changed/current docs have valid local links and balanced fences. Exact
commands/statuses: ci-runner-egress/final-checks.json. Linux explicit-path/cleanup
regression11985/0 and macOS8script tests0 complete the applicable local gate.

Signed checkpoint1a0d954d90cdc5094891587ba57cfb2ad74a7e8f committed/pushed after
applicable local checks.19staged files/92633diff bytes, Gitleaks/whitespace0;
signature and exact remote-head verification0. Receipt: checkpoint-cgroup/.
Actual hosted run34676397291 started05:45UTC on that head and is in progress:
https://github.com/KofTwentyTwo/nuncio/actions/runs/34676397291.
Next: observe this exact run, verify each Linux controller regression and required
job outcome, retain evidence and resolve demonstrated failures. No doc-only push
while it runs. Selected8996085 archive and244production source files unchanged;
no installation/publication/live acceptance.

At05:47UTC all6Linux controller checks passed on hosted1a0d954; all10jobs still
in progress. first-controller-results.json retains the exact job/step outcomes.
Watcher33838 ended1 on a local GitHub TLS handshake timeout at05:53UTC; its
log is retained. Fresh job API succeeded0 and still shows the workflow running,
so this observation failure is not a job failure. Resume a watcher on the same run.
Worksheet now references the verified local archive/binary hashes, keeping exact
future extraction paths and all live actions pending separate approval.

At05:52UTC new hosted34676397291 has mock jobs passed on both Linux/macOS
(26tests and2commands0 each) and macOS E2E passed (40tests/7commands0).
Downloaded artifacts independently confirm those counts, zero failed/ignored,
parent/child egress denial and loopback access. Linux evidence additionally
confirms cgroup scope/reject counters and same-UID controller success in all
6address/phase exchanges for exit0 and17, with test-child denial.
remote-ci/34676397291/{mock-linux,mock-macos,e2e-macos} and summary JSON.
Seven jobs remain in progress; no overall hosted success claimed.

At05:54UTC hosted Linux system and macOS lint also passed: downloaded artifacts
confirm7commands/37system tests (Google22/operations6/repair2/reconciliation3/
multi-engine1/security3) and8commands/152core tests, respectively. Zero failed or
ignored. Egress evidence0 with parent/child denial; Linux cgroup counters verified.
Five hosted jobs passed; five running. New watcher60587 monitors the same run
following33838's TLS observation error. Summary JSON and artifact directories are
remote-ci/34676397291/{system-linux,lint-macos}.

00:57Central hourly chart report sent05:57UTC to james@kof22.com, Gmail ID/thread
1a094310f51ac411, SENT; independent read confirms183475-byte inline16-task PNG
and CIDnuncio-progress-0057. Exact body/chart/estimates/payload/receipt:
status-emails/2026-09-12-0057*. Report records7hosted jobs passed/3running and
2–6offline active hours plus3–6deferred live; full goal remains incomplete.
Next email06:57UTC /01:57Central during active execution. Linux E2E40tests/7commands0
and lint152tests/8commands0 additionally confirmed in downloaded artifacts;
parent/child denial, loopback and Linux cgroup reject counters verified. Remaining
hosted jobs: IMAP and both release-check jobs; watcher60587 remains active.

New hosted IMAP job failed: independent Python services, IMAPcontract2/system27/
actual E2E13 passed; resource_system3passed/1failed (cargo101). The10,000-message
case times out polling GetSyncRun at resource_system.rs:62 using the production
30-second gRPC timeout; suite duration76.56s. Egress isolation still passes and
wrapper propagates1. Retained imap-linux/results/logs and imap-linux-failed.log.
Do not increase the timeout, reduce workload or weaken independent assertions.

Current hypothesis: mail_promotion.rs::promote_one deletes FTS rows by two
UNINDEXED columns for every message, even after full-generation clearing, making
promotion quadratic and blocking the serialized store worker/status calls.
Independent SQLite FTS5 diagnostic confirms full scan and1k=.057s/5k=1.273s/
10k=4.952s (non-SQLCipher; diagnostic only). Proof: ci-runner-resource/
fts-repeated-delete-diagnostic.json. No production fix yet. Next: measure actual
SQLCipher promotion and compare a single account-scoped batch delete; preserve
atomicity/identity/search isolation and run original load/crash/E2E gates.
If production changes, rebuild/reverify a new package; preserve existing8996085.
Watcher60587 ended1; fetch final job ledger before claiming remaining release status.

Focused real-SQLCipher regression19128 fails101 at30,003ms: a10,000-message staged
promotion with10,000 indexed messages in another account starves queued status.
Full test41.94s; production30s deadline retained. Proof: ci-runner-resource/
promotion-red.log/envelope. New permanent test: engine/tests/mail_promotion_resources.rs.
Original10k system baseline1256 passes locally (sync24,545ms/list1,420ms; total
42.76s including14.09s compilation), preserving all workload/count/effect assertions.
This does not negate hosted failure. baseline.json/log/metrics retain evidence.
New delta-preservation test first failed21158 because its expected-absent subject
term Original also exists in every fixture body. Corrected only that subject to
Supersededtitle; unchanged assertions test the intended index replacement.
No production fix yet; run corrected delta baseline, then batch-clear affected
search rows once in the existing promotion transaction and rerun both regressions.
Final hosted34676397291:9jobs passed/1IMAP resource failure. Both release artifacts
confirm8commands0, isolation2/external1 and package command0; no hosted archive
hashes uploaded. source/package refresh will be needed after a production fix.

Production correction now in store/mail_promotion.rs: one affected-account search
reset per promotion transaction, removing per-message FTS full scans. Delta
clears only staged message IDs; full clears its account as before. Same transaction,
identity/cursor semantics and all original timeouts/workloads/assertions retained.
Corrected delta baseline30313/0 passed after the fixture-term correction.
Focused11591/0 passes3projection tests plus new promotion resource test:
encrypted promotion+queued status3,669ms versus red19128 timeout30,003ms.
Exact logs/envelopes: ci-runner-resource/{promotion-red,promotion-green,delta-baseline-corrected}*.
New test verifies10k staged plus10k preserved other-account index rows; added delta
test proves unchanged messages/other account, deletion/update/add, old-index removal,
identity preservation and transaction rollback. Production change is uncommitted.
Next: full --all offline gate with source hashes, verify new actual10k timings,
then signed checkpoint/push/hosted run and fresh verified package pair. Existing
8996085 artifact is preserved but predates this production fix. No CI watcher or
local test remains running at06:13UTC; next email06:57UTC/01:57Central.

Full offline gate is now running from test-results/ci-promotion-all/run.py.
Poll shell91408; do not change crates/scripts/tests/clients/licenses
while its source snapshot is active. It retains prior all/command logs and metrics
before running --all and copies the new results afterward, checks source hashes,
and records envelope/exit. Continue this gate, resolve actual failures, then update
counts/docs, create signed checkpoint/push and observe a new hosted run. New package
pair is required after this production change; preserve the old candidate.

Full gate91408: normal workspace passed307tests, zero failed/ignored, command0
at06:31UTC. Actual original10k system workload now sync13022ms/list1506ms,
with100APIpages/100remote pages and all independent effects/resource assertions.
Normal baseline before the fix: sync24,545ms/list1,420ms. Exact current metrics
and counts retained in ci-promotion-all/normal-*.json. Feature-workspace and
separate named suites remain pending/running; full gate not complete.

Resume at06:38UTC: full gate91408 remains active. Normal workspace307passed,
feature workspace currently in IMAP system/recovery checks with no reported
failure. Code/source freeze remains in effect. Poll91408 and inspect
ci-promotion-all/run.log plus all/results.json; active.json names the handle.
Next email06:57UTC/01:57Central with all16tasks; current estimates in PROGRESS.md
are3–8offline plus3–6deferred live. Last chart email0057 was sent and independently
verified (1a094310f51ac411). No additional source edits, checkpoint, hosted run or
package refresh has occurred since starting the full gate.


## Earlier checkpoints and execution records

Latest signed checkpoint2e4d30ef9434638db4a5b824b490cdb4278bf13c is pushed;
independent ls-remote and signature checks passed0. Seven staged files/27974diff
bytes passed Gitleaks/whitespace0. Logs/receipt: test-results/checkpoint-resource/.
Hosted run34672641999 completed failure: all4macOS jobs passed; all6Ubuntu
jobs failed with hosted-runner communication loss. Watcher59190 ended1.
https://github.com/KofTwentyTwo/nuncio/actions/runs/34672641999.
Terminal ledger: remote-ci/34672641999/run-final.json; annotations retained.
Earlier log-download404 responses were observation failures, not command logs.

Confirmed root cause: owner-UID iptables filtering also catches the runner's
same-user control connection. Independent local controller regression failed1
at its during-test socket, after baseline connectivity and test-child denial;
proof: ci-runner-egress/control-red.json/log/artifacts. Current uncommitted fix
uses a dedicated cgroup-v2 path for the test hierarchy, preserving UID and
runner connectivity. Privileged helper drops privileges before exec and kills
remaining test processes before detaching filters. Linux cgroup matcher support
was verified in a private disposable container; no host firewall was modified.
Local28824/0 and extended26231/0 pass controller continuity, strict denial,
exit0/17, detached-child stop and owned cleanup on return/SIGTERM. Eight existing
script regressions pass under macOS egress0; configured Ruff check/fmt24files0.
Controller check is added before each Linux CI job. Final explicit-path regression11985/0 passed; actionlint0. No local test running.
Next: final gate/docs/whitespace/Gitleaks,
signed checkpoint/push, then verify actual replacement hosted run.
Production source and selected archive remain unchanged. Do not repackage for
CI-script or dated-report edits. No product/build/test process remains running.

Corrected-head hosted artifacts independently confirm mock26/2commands0,
lint152/8commands0, actual E2E40/7commands0, release-isolation2/external-client1
with8release commands0, plus separate package-command egress exit0. All recorded
Rust results have zero failed/ignored; parent/child externalIPv4/IPv6 denial and
allowed loopback confirmed. Artifacts and summaries: remote-ci/34672641999/.
Hosted archive hashes/check JSON were not uploaded; do not invent them.
Old34671490682 terminated cancelled04:23UTC:3macOS passes/1macOS failure/6Ubuntu
cancellations. Final ledger retained; unavailable Linux partial log returned1.

Final local documentation refresh45789 completed0 at04:55UTC: two fresh packages,
22extracted checks each, byte-identical archive/binaries, zero changed inputs or
README link failures. Canonical selected archive:
dist/final-candidate/verified/nuncio-0.1.0-rc-aarch64-apple-darwin-2e4d30ef9434.tar.gz
SHA2568996085e4e698383e11625d848b287616b165c0a9b33fb4430c8ab872cedc1be.
Daemon SHA256f64aad27cb3c75eebd05822f73f8439f4ae9fc0e52e9b8cc6e0bb0642c47b7de;
CLI SHA2561495aa640c31a3b45159005af8f040292af3d03ffacb41448d2b33e22163cac5.
237notices and frozen descriptor verified; all244production source files unchanged.
Metadata honestly records dirty documentation based on2e4d30e and source epoch
1789186687. Current EVIDENCE.json points to this selected archive; prior47d65f and
intermediatee49121a archives/receipts remain preserved. Proof: task16-package-final/.

Intermediate30482 already proved equal e49121a archives, but a final guide inspection
found PACKAGING declaring the preceding artifact current. Generic guide now points
to adjacent metadata/checksum/evidence, avoiding a self-referential archive hash.
No package code or product feature changed. Corrected45789 proved the guide matches
source and contains no obsolete candidate declaration. Do not rebuild merely for
later dated report updates; operating guidance now identifies its own artifact.
Archived developer reports are explicit build-time snapshots; current source report
and external receipts retain later evidence. No installation/publication/live use.

Implementation report and requirement/operating/dependency/mock guides are current;
latest documentation edits remain uncommitted pending hosted results. Updated source
report names the selected8996085 archive. All17current source docs pass local links/fences and git diff --check0
(task16-package-final/current-docs-check.json). Run staged Gitleaks/whitespace
before the authorized documentation checkpoint/push when hosted results settle.

23:57Central hourly report sent04:57UTC, Gmail ID/thread1a093fa2c99cabb4, SENT;
independent read confirms183443-byte inline16-task PNG/CIDnuncio-progress-2357.
Exact report/chart/estimates/payload/receipt: status-emails/2026-09-11-2357*.
Estimates remain2–6offline active hours plus3–6deferred live; external waiting excluded.
Next email05:57UTC / September12 00:57Central while active. Last report includes
all4macOS passes,6Ubuntu pending and final8996085 artifact. Live/native-keystore
acceptance remains deferred/unapproved; no account request now. Full goal active.

Previous checkpoint context:
Signed checkpoint7f81b735b593c3e86caf64d59964fedfa3ffb2fc is pushed to the approved
feature branch; independent ls-remote matched exactly. Final staging56files/
365648diff bytes, Gitleaks0, whitespace0, clean worktree before commit. Initial
signature verification could not open GPG trustdb under the sandbox; narrowly
escalated verification passed0. No auto-review rejection or remote config change.
Receipt/logs: test-results/checkpoint-release/.

Affected gate91079 completed exit0: all6 outer commands passed (fmt/bothClippy,
E2E7commands, IMAPE2E2commands, release8commands). Actual suites: Google27,
recovery5, repair2, migration1, security2, resources3, IMAP13, release-isolation2
and external generated-client1; zero failed/ignored. Parent/child egress checks
passed. Commands/counts/copied suite logs: test-results/ci-resource-fix/.
All244 production source files still match the verified packaged candidate.

The verified test-only correction and updated evidence are now checkpointed in
2e4d30e; observe its hosted run as specified above. Original hosted workflow34671490682
on7f81b73: macOS mock-contract, lint and release-check passed; macOS E2E failed in
initial large-message sync (CLI4); Ubuntu jobs still running at04:15UTC.
Preserved artifacts: remote-ci/34671490682/{e2e-macos,mock-macos,release-macos}/.

Deterministic slow-response repro87753 failed at1007ms with provider_unavailable,
confirming the one-second synthetic harness deadline. Initial79865 compile mistake
is recorded separately. Production uses30s. Only the large-payload fixture now
uses that existing production deadline; all fast fault-test defaults remain1s.
Added1250ms independent response delay, retained16MiB/eight cycles/128MiB growth
bound and all remote send/copy/notification assertions. Focused71160 passed3;
affected91079 subsequently passed. No mock validation or production timeout changed.
Inspect remaining original jobs for other defects before replacing its run.
New pushes cancel the old run via existing workflow concurrency policy.

Actual run URL: https://github.com/KofTwentyTwo/nuncio/actions/runs/34671490682.
Full goal is incomplete; live/native-keystore acceptance remains deferred/unapproved.
No new account request now. Do not equate a running workflow with passing CI.

Full integrated egress gate58495 completed0 September12 03:31UTC: all34commands,
305tests in each workspace configuration,18named suites, six independent Python
mail-service scripts, six then-current script tests, dependency and external client
checks. Zero failed/ignored; all396snapshotted source files unchanged. Full logs,
commands/counts/source verification/metrics: test-results/task15-all-green/.

After that gate only scripts/package.py and scripts/test_package.py changed. The
four broken packaged README links are fixed. Controlled repeat66388 failed1 despite
22runtime checks passing in each build: only random embedded protobuf/OpenSSL paths,
OpenSSL date and current metadata differed. Correction uses an exclusive fixed
fresh build path, Git source epoch and normalized tar/gzip metadata; existing build
data is refused/preserved. Ruff/format, package4 and fullscript8 pass. Controlled
repeat31790 completed0: two fresh archives and both binaries are byte-identical,
no input/file differences,22extracted checks each, all README links resolve.
Original failure/diagnosis: task15-repeat/; corrected evidence: task15-repeat-green/.
No running verification process remains at this checkpoint-preparation stage.

Canonical local artifact: dist/final-candidate/
nuncio-0.1.0-rc-aarch64-apple-darwin-720081f661d0.tar.gz, SHA256
47d65f00764da1cedb691109bd5b65c402ed253d69a40e2f0fe43fff13c0063f.
Daemon:db0dce123719b9126b7aa5f09aa47e879d6d8c0759bf6a97dbda991c173e1bc3.
CLI:1495aa640c31a3b45159005af8f040292af3d03ffacb41448d2b33e22163cac5.
237third-party notices and frozen six-file v2 descriptor verified. EVIDENCE.json,
SHA256 sidecar, build JSON and extracted-check JSON are adjacent. Archive metadata
honestly records uncommitted source based on720081f; source hashes identify it.
Archived docs are the build-time snapshot; current verification lives in source
docs and the adjacent evidence. No installation/publication. Repeatability proof
is for identical recorded platform/toolchain/compiler/SDK/checkout/build-path inputs.

Latest signed/pushed head is7f81b735b593c3e86caf64d59964fedfa3ffb2fc; actual hosted
workflow34671490682 is running. No root Cargo changes; .github/workflows/rebuild-ci.yml is
the only root integration change. Six named jobs/ten Linux/macOS matrix executions;
YAML/binding checks, actual exit17 propagation per job and actionlint1.7.12 pass.
Hosted CI has three passing macOS jobs and one failed macOS E2E job; Ubuntu
jobs remain pending. Rerun staged scans before the next checkpoint.

Previous full gate98403/1 and sampler failures are preserved. Native numeric
libproc/procfs sampler passed macOS/Linux real allocation tests; spawn_blocking
keeps it off the mock runtime. Historical full gate used1s; the latest large-resource fixture correction is
documented above. The128MiB RSS growth bound and remote-effect assertions remain. Full provider egress/relay and dependency
remediation evidence is in VERIFICATION; do not reopen already-passing foundations.

## Hourly status emails

James authorized hourly progress emails to james@kof22.com beginning immediately.
He additionally requires a chart of all16 major tasks, estimated percent complete
and remaining active-work hours per task. Baseline definitions/estimates are in
PROGRESS.md; revise with actual evidence every hour. Renderer/example MIME/PNG are
under test-results/status-emails/. Embed the chart plus a readable HTML table and
plain-text fallback; visually inspect before sending.
First hourly email sent September11 at approximately22:57UTC/17:57Central using
connected Gmail, subject `Nuncio rebuild — hourly status — September 11, 2026,
5:57 p.m. CT`; Gmail ID/thread `1a092b0a4e84f109`, SENT confirmed.
Second hourly email sent September11 23:57UTC/18:57Central, confirmed at23:57:17UTC;
ID/thread `1a092e77786bec8b`, SENT. Includes verified security/workload checkpoints
and the new RSS failure. Exact body/receipt: test-results/status-emails/2026-09-11-1857.md.
Third update initially returned an internal connector error at00:57UTC. Sent-folder
search at approximately01:02UTC found no third message; one retry succeeded at
approximately01:03UTC, Gmail ID/thread `1a0932257375285e`, SENT. Exact body and
attempt history: test-results/status-emails/2026-09-11-1957.md. Recipient delivery
or reading is not independently confirmed.
Next due: September12 05:57UTC / September12 00:57Central. Check the clock during active goal execution;
At James's explicit request, a revised third report was sent at01:08UTC with all16
task bars/percentages/hours. Gmail ID/thread1a09328893b32fa7, SENT; sent-folder read
confirms the186505-byte inline PNG and matching content ID. Exact report and chart:
2026-09-11-1957-revised.{md,html} and -chart.png under status-emails/. This revision
does not shift the regular01:57UTC schedule. Estimates15–31offline hours plus3–6
live hours after authorization; live waiting time excluded, no delivery guarantee.
send one concise update each hour with actual progress, test results, blockers,
remaining deliverables and next action, then update this timestamp and message ID.
Do not send catch-up bursts or infer progress while paused. This authorization is
for project status mail, not rebuilt-provider acceptance. No persistent schedule
is configured: no thread scheduling tool is exposed, and Computer Use refuses
Codex access. The user was told that delivery during pauses/stops is unconfigured.

## Current local artifact evidence

Resource-status gate shell57763 completed all15 commands exit0. Daemon:
`target/production/release/nunciod`,16402304 bytes,SHA256
`24b7c7aa3521ffe871b7a6488829dc825b28566e0abc386d88bb1ba705d0b1fc`.
CLI:`target/production/release/nuncio-cli`,3506544 bytes,SHA256
`9715785d0c29467b9e477499308e3b7c35fbea6aeff3099b47aeb076e661808d`.
Both exclude production test hooks. These are local binaries, not packaged or
installed releases. No remote-CI/live-compatibility claim. Older startup evidence
below remains valid for that earlier checkpoint.

## Historical startup-cleanup checkpoint

Startup cancellation: `engine.rs::open_inner` creates Arc<File> profile ownership immediately after profile preparation. `engine/recovery/journal.rs::recover` clones it into each blocking cleanup closure. Cancelling Engine::open therefore cannot allow another engine to overlap still-running cleanup. Existing deterministic SecretStore.delete barrier test proves Locked during cleanup, normal restart afterward, original source keys/manifest, and removal of only owned stage/upload. Direct journal fixtures acquire a real profile file lock.

Relative paths: Journal creation and startup canonicalize the owner using directory-identity validation. `Journal::record` also normalizes the upload's owner before comparison, permitting macOS `/var` aliases while rejecting another owner's input. No final-component owner symlink is accepted. Tests cover failed restore reaching encrypted inspection, an actual persisted cleanup row, startup retirement, successful one-component relative restore with independent identity/API keys, owner symlink preservation, ancestor alias acceptance, and foreign-owner upload rejection. Source locations: `engine/recovery.rs`, `engine/recovery/journal{,_tests}.rs`, `tests/profile_lifecycle.rs`. Schema22, proto and dependencies are unchanged.

## Historical startup-cleanup evidence

All logs: `test-results/task13-startup-cleanup/`.

- Original startup red: shell98698/101, red-corrected-fixture.log (1 failed). Earlier red.log includes a fixture mutex-lifetime bug, corrected before the product failure was confirmed.
- Lock fix: shell82528/0, green-lock.log, journal4 passed.
- Strengthened relative red: shell82643/101, relative-strong-red.log, InvalidPath before encrypted inspection. The earlier relative-red.log passing test was too weak and is not accepted as recovery proof.
- Relative fix: shell75532/0, relative-green.log, profile5 passed.
- First two broad attempts stopped on new-test Clippy/type errors (shell24447/101, shell19129/101); preserved as gate-first-* and gate-second-*.
- Next broad attempt (shell71857/101) passed fmt/bothClippy but exposed `/var` versus canonical upload-owner mismatch in maintenance2tests; gate-alias-failure-* preserves output. Product comparison fixed, no assertions weakened.
- Focused alias/path/lifecycle run: shell56304/0, alias-focused.log, 13 passed/0 failed/0 ignored. Includes successful relative restore, existing cancellation ownership, path replacement and cleanup-failure cases.
- Full gate shell87318 is terminal: all7 commands0, engine126/recovery4/IMAP13/release2. New library alias/foreign-owner and owner-symlink checks passed; source/artifact hashes retained.

## Previously verified foundation

Last fully verified gate before these changes: `task13-maintenance-ownership`, all7 commands0, engine122/recoveryE2E4/IMAPE2E13/release-isolation2 plus format/bothClippy. Preceding `task13-restore-crash`, all12 commands0: engine119, protoCLI/daemon20, Google system21/E2E26, IMAP system26/E2E13, migrationE2E1, recoveryE2E4, release2 and format/bothClippy.

The schema22 migration E2E includes44 actual SIGKILL cases, before/after every migration commit from schemas0–21, with two verified restarts each; frozen histories1–21 remain unchanged. `migration-case-index.json` identifies exact evidence. Recovery E2E includes five restore kill boundaries: owned empty stage before export, completed stage, first key, second key, activation. Original profile/keys/draft/backup and independent provider state survive; activated targets retain keys. Separate recovery tests verify real PDF/raw bytes and held request identity through engine/API/CLI.

Earlier Sent/reconciliation/path/worker checkpoints and exact implementation locations are retained in VERIFICATION and `docs/history/2026-09-11-startup-cleanup-working-history.md`. Do not reimplement completed work.

## Invariants to preserve

Schema22 source-owned restore cleanup rows record owner/parent/stage/upload identities and activation intent. Cleanup preflights bounded allowlisted entries through directory handles before new-key deletion. Replaced/uncertain paths retain evidence and keys. Backups/restores exclude cleanup authority. Successful activation retains new keys even if later upload cleanup/fsync/reply fails. A closed Store leaves the durable row for startup.

MaintenanceLease retains the transfer semaphore and source profile lock through returned backup artifacts, upload/input objects and blocking work. Engine::restore_backup is the production RPC path. Engine::restore_profile is a caller-owned lower-level integration helper; do not silently claim it has a source cleanup journal. Avoid store-worker self-deadlocks and keep ownership after cancellation/shutdown.

Schema21 explicit reconciliation is durable/scoped/versioned through engine/API/CLI. Observe never mutates providers. Positive exact Sent evidence can resolve an accepted SMTP copy; missing/ambiguous reads never authorize resend/APPEND. SMTP acceptance, copies, and calendar notification uncertainty remain independently represented. Restored pending work remains held until an explicit safe reconciliation/duplicate-risk decision. Original requests, frozen MIME/private Bcc, receipts and attempt ordinals survive.

## Remaining full scope

Task13: restore-retry verification passed and checkpoint57dd648 is pushed. Existing encrypted WAL backup, rekey/hold, historical schema, projection repair and subprocess recovery evidence covers the functional task. Reconcile any remaining explicit Task13 acceptance gaps against that evidence during the full gate. Earlier artifact-lifetime and caller-owned-helper audit ideas are not an open-ended prerequisite for Task14. Rich send histories10–21 and schema22 migration/crash cases are verified. No destructive account purge.

Task14: multi_engine_system and security_system/security_e2e with relevant gates pass. Create/run resource_system and actual subprocess resource checks next. Require three independent engines,10,000 metadata messages,16MiB attachments,64MiB payload limits, concurrency2, bounded queues, measured RSS/latency, canary scans and independent encryption checks. Audit actual outstanding defects rather than repeat completed fixes: post-submission storage failures, worker failures, parser bounds, aggregate attachments, legacy intents, calendar inheritance/Trash origins; writerWithoutPrivateAccess remains unverified. Run the complete offline verifier after implementing the missing suites.

Tasks03/15: explicit test egress denial (Docker bridge alone is not a firewall); external descriptor-generated client and contract freeze/docs; dependency advisory/security/license checks; package.py and extracted artifacts; CI definitions and required-job failure behavior. Task16: final R01–R16 evidence matrix plus operating/recovery/compatibility/risk report. MANUAL-ACCEPTANCE.md exists; all live acceptance is unapproved/unverified. Passing local mocks does not prove live Google/MailPlus compatibility; local checks do not prove remote CI ran.

Local binary locations: `target/production/release/{nunciod,nuncio-cli}` and feature-only `target/test-harness/debug/{nunciod,nuncio-cli}`. Production hashes were refreshed by the restore-retry gate above. These are not installed or packaged releases. No full Task13 or R01–R16 completion claim.

Hourly21:57 report sent September12 approximately02:57:13UTC, ID/thread
1a0938c424bff603, SENT. Embedded184157-byte chart covers all16 tasks; estimates
7–14offline hours plus3–6deferred live hours. Full58495gate pending; focused
resource fix passed. Body/chart/estimates/receipt: status-emails/2026-09-11-2157*.

Hourly22:57 report sent at approximately03:57:08UTC, ID/thread1a093c31c36b8bba,
SENT with independently confirmed183517-byte inline chart. All16 tasks included;
estimates2–6offline active hours plus3–6deferred live. Evidence: status-emails/
2026-09-11-2257*. Next04:57UTC/23:57Central during active execution.
Hosted macOS mock-contract job passed; downloaded artifacts independently show
26tests/0failed/0ignored,2commands0, parent/child egress denial and underlying exit0.
Other jobs remain pending; exact artifacts: test-results/remote-ci/34671490682/mock-macos/.
