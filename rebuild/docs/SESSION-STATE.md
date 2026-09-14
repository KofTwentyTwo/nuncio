# Rebuild session state

## Active IMAP sync failure and startup delay — September 14

The user’s run processed 66,524 entries, then failed with `unavailable` 8 ms after its last batch. No completed snapshot was published. Startup took 251.686 seconds: 11.638 seconds at profile-key access and 240.040 seconds in the combined account-credential cleanup / interrupted-sync recovery phase. Three native prompts were reported. The existing logs do not establish which recovery operation caused the four-minute pause; static ad-hoc signing inspection does not prove that cause.

The independent Dovecot system regression now reproduces a concrete defect: append one message while initial sync downloads, and the complete run fails `unavailable` (exit 101, expected assertion; `test-results/startup-delay/arrival-red.json` and log). A bounded three-pass mailbox reconciliation is implemented. It retains same-run immutable bodies within one UIDVALIDITY, refreshes flags, reconciles expunged staged rows, and keeps atomic publication and strict protocol checks. Mailbox identity changes or continuous churn remain failures with a specific code. This is a proven offline defect, not yet the proven live Synology trigger.

Targeted checks pass: CLI output 9, startup/privacy subprocess 6, human walkthrough 1, IMAP catch-up system 5 and actual subprocess crash E2E 1 (two crash boundaries); all six commands exit 0 (`targeted-checks.json`). A concurrent flag update first reproduced `invalid_provider_response`; valid unsolicited flag notifications are now handled separately, with all four initial cases passing including disabled optional capabilities (`catchup-notifications.json`). The added UID epoch and strict parser cases also pass in the full gate. A 66,524-entry encrypted staging cleanup passes in 2.548 seconds, preserves published mail and leaves no staged children/FK violations (`large-cleanup.json`). This does not prove native Keychain latency.

Both complete workspace configurations passed 359 tests each, zero failures/ignored (exit 0); these are repeated configurations, not 718 unique tests. The separate required suites also passed: Google mock 26/system 22/E2E 42; independent IMAP contract 2/system 34/actual subprocess 17; all recovery, migration, reconciliation, security, resource and release-isolation suites, plus the independent API client. Five production preflight/typo checks also pass without profile or Keychain access (`local-production/checks.json`).

All 35 offline commands passed at 20:28:40 UTC, including 40 script regressions, both Clippy configurations, formatting, dependency policy and client boundaries. All 372 recorded source hashes matched. Exact commands, exit statuses and test counts: `test-results/startup-delay/full-gate-final.json`; complete archived logs: `full-gate/`. The first gate's older transfer-test initializer compile failure is preserved in `initial-full-gate-test-fixture-error/` and corrected; no assertions were weakened.

Changes are ready for a signed checkpoint/push. Next: qualify the exact committed source through actual hosted jobs, clean local production archives and public fresh/update installs, then send the laptop retest instructions. Qualified software remains 7792643 until that delivery passes. No schema, credential format, native Keychain or live provider change. See [the bounded follow-up plan](PLAN-IMAP-CATCHUP.md).

Hourly status email `1a0a17628e4c5104` sent September 14 at 14:47:50 Central to james@kof22.com. SENT, recipient, exact plain/HTML bodies and 251798-byte chart metadata verified. It reports passing targeted fixes, full verification still running, qualified public source still 7792643, and native/live gaps. The 25-task chart reopens IMAP/reliability/delivery/logging/help follow-ups; 2–4 active hours estimated, overlapping task estimates and CI waits excluded. Next hourly report due 15:47:50 Central while active; no inactive-session scheduler.

## CLI, startup logging and Rustls patch — delivered at 7792643

September 14, 2026. The CLI defaults to readable results and useful errors;
`--json` retains machine envelopes. All 71 help pages, 531 visible options and
192 argument cases were audited. Startup now reports 20 ordered phases at INFO,
with safe schema/account counts and elapsed time at readiness. DEBUG includes
committed migrations. Running sync/account/operation/shutdown events remain.
Original stderr is tested for privacy; readiness stdout/file contracts are unchanged.

The current source passed the full 35-command offline gate at 18:23:24 UTC:
348 tests per workspace configuration, zero failures/ignored in either run.
These are repeated configurations, not 696 unique tests. Separate Google mock 26,
system 22 and E2E 41 pass; independent IMAP contract 2, system 28 and subprocess 16
pass. Recovery, migration, reconciliation, security, resource bounds, release
isolation and the independent API client pass. All 40 script tests, both Clippy
configurations, formatting, dependency policy and client boundaries pass.
Exact commands, exits, counts and archived logs:
`test-results/startup-logging/patched-gate-final.json` and `patched-gate/`.
All 369 recorded source hashes matched at completion (`patched-source.json`).

Actual CLI checkpoint 92e9394 passed hosted macOS/Ubuntu E2E but failed the newly
published RUSTSEC-2026-0285 advisory. All three locks now use Rustls 0.23.45.
The original also advances AWS-LC/webpki to Rustls's new required minimums.
Six fresh cargo-audit/cargo-deny checks pass; all three old locks first failed
with the actual advisory. Database: e2e640471715167f73e22eaf761f2e547adafeec.
Original formatting, Clippy and 790 tests pass with external egress denied.
Evidence: `test-results/startup-logging/rustls/`. No scanner policy or assertion
was weakened. The first wrapper selected a database lock file, then lacked
cargo-audit on PATH; both were corrected using the existing project-local tool.
The earlier startup verifier 9436 was stopped during a workspace run (exit 143);
that interrupted run is not a pass and is superseded by the complete gate above.

Four production preflight/parser checks passed without profile creation or
Keychain access (`local-production/checks.json`). They verify INFO/debug/off
startup behavior and the user's exact missing-account CLI example. The startup
regressions failed before instrumentation; fresh/reopened startup order and
failed startup without false readiness now pass against independent provider
state. Production code changes are static lifecycle log events plus the TLS
patch: no API, schema or provider-operation behavior changed.

Signed/pushed `779264348db7240bfc368a5a5e5650f57e9d0b80`; signature and exact
remote branch head verified. Actual rebuild 34880705357 passed all 10 jobs;
security 34880705426 passed all 7. Its 45 open CodeQL findings have the same IDs,
rules, paths and severities as the prior reviewed baseline and were all seen
at this source. No findings were dismissed or suppressed. The comparison is
recorded in `test-results/startup-logging/codeql-alert-comparison.json`.

Two clean local production archives are identical 29b9e6b2, with 478 manifest entries,
241 notices and 22 extracted checks each. `dist/final-candidate/EVIDENCE.json` selects
this 7792643 pair and preserves the earlier 82a92a0 selection. The actual public
fresh/update pipelines both selected 7792643/run 34880705357/attempt 1/artifact 10363403258,
hosted archive b7af0ad4. All 478 files, ARM64 headers/versions and 71 help pages,
531 options and 192 argument cases passed in each installed package. Fresh production
INFO/debug/off preflight passed before profile/Keychain access. The old f14b02a
prefix updated through the same bin paths and retained every prior-build byte.
Normal environment unchanged; no serving production daemon or live account action.
Source-specific records: `test-results/startup-logging/{installer,update}/`.

James's no-logging report came from f14b02a; the new public 7792643 is now verified.
Email 1a0a13ee067320a5 contains the exact source, stop/update/debug-start/status
commands and 25-task chart. SENT/recipient/subject/exact plain/HTML and 247874-byte
chart metadata were independently verified at 13:47:27 Central. Next hourly report
is due 14:47:27 Central during active work; no inactive-session scheduler exists.

The older a5b Ubuntu attachment timeout did not recur in current local or hosted
checks. Its cause remains unproven; strict assertions and diagnostic evidence
are retained as a risk to investigate if it recurs, not a claimed root-cause fix.
Current resource/delivery checks passed without weakened assertions.

This report-only checkpoint records qualification of software 7792643; it does
not introduce another tested build. Documentation/source checks and the signed
commit/push receipt are in `test-results/startup-logging/final-report/`.

Next: resume for laptop feedback on 7792643 or explicit Google registration/named live
Google/Synology/native-keystore acceptance authorization. Those external checks
remain deferred and the full goal is incomplete. Do not repeat unchanged builds
or infer provider compatibility from mocks. All software/test/CI processes ended.

## Earlier checkpoint history

The entries below preserve intermediate results and pending actions at those
checkpoints. The current delivery and next action are recorded above.

## Daemon running logs — in progress, September 14

User requested visible running output and a configurable info/debug default.
Implement timestamped stderr application logs, default info, with --log-level
for troubleshooting. Preserve readiness JSON on stdout and existing wire-log
protections; allow only application tracing targets and safe IDs/counts/codes.
Plan: reproduce missing output/options, add startup/sync/account/write/shutdown
events, test actual daemon/CLI against independent offline providers, run the
relevant quality gates, then signed commit/push and verify testing delivery.
The user also requested hands-on Google setup instructions; GOOGLE-SETUP.md
now documents the current Console flow and already-shipped --client-config
route, allowing setup on the remote laptop without a build or repository secret.
No live account or Google Cloud setting was accessed.
Full offline gate passed:35/35 commands exit0, including both workspace
configurations, named provider/system/subprocess suites, dependency review,
security/resource/release isolation and external-client boundaries. Exact
counts and archived logs: test-results/daemon-logging/gate-final.json.
The first gate stopped because Ruff was absent from PATH; the rerun used the
already installed project-local Ruff. No source change or skipped check.
Signed and pushed a5b71ba8a2e14cc2efe9831913127b474c34b81d; exact remote ref
and signature were verified. Hosted rebuild34863965649 has an Ubuntu resource
E2E failure: attachment download iteration4 exceeded the CLI40-second deadline
after four successful16MiB downloads. Google E2E38, including all new logging
cases, passed there; the macOS E2E job passed. Do not qualify this candidate or
relax resource assertions. The unchanged Linux reproduction passed (1/1) in an
offline ARM64 container; evidence: test-results/daemon-logging/linux-resource/.
Actual CI finished9/10rebuild and7/7security. A test-only diagnostic now reports
partial bytes and bounded daemon health on failure. macOS resource3/3 and Linux
diagnostic case1/1 pass, as do format/Clippy. A temporary injected stream stall
verified262144partial bytes, status_exit0, and preservation of the original
timeout failure. This proves the diagnostic, not the cause of the CI stall.
Next: diagnose the timeout, apply a verified fix if required, then qualify actual
hosted and public installer delivery. The installer still selects f14b02a.
Install/update instructions were emailed to james@kof22.com at10:58:55Central,
message1a0a0a495ab2a856; SENT and exact plain/HTML content verified. They explain
the permanent command and executable paths, stop/update/restart, Google setup,
and that unattended self-updates are not implemented.
Evidence goes in test-results/daemon-logging/. The full goal still needs the
previously deferred live/native acceptance; this is independent authorized work.

## Delivery report alignment — September 13

The continuation audit found stale current-build claims in the main report,
requirement matrix and READMEs. These now identify the qualified testing source
f14b02a and its actual10-job rebuild/7-job security and public update evidence.
The separately retained reproducible local candidate remains82a92a0; its archive
hashes and evidence were preserved, not relabeled. The manual worksheet now
selects a testing download from its own `current/TESTING-INSTALL.json` and uses
permanent command paths. All live/native rows remain unapproved/unverified.

This is a reporting correction with no implementation, artifact or authorization
change. Verification inputs and subsequent checkpoint receipts are retained in
`test-results/delivery-report-refresh/`. The prior installer turn made concrete
progress; this continuation corrects the R16 handoff. No CI/test process is live.
Validation passed: 160 local links resolve, software matches qualified f14b02a,
both retained local archives match their identical recorded5dbed9e9 hash, and all
470 current installed files plus binary hashes match their own f14b02a receipt.
All16 requirement rows are retained, with eight still externally pending.
Whitespace and scoped secret scanning passed. This report-only checkpoint uses
`[skip ci]`; it does not claim a separate hosted run or rebuild. Exact validation:
`test-results/delivery-report-refresh/validation.json`.
Next action after the documentation push: await the previously prepared Google
registration approval and separately deferred named live/native acceptance.
The full goal remains incomplete; do not restart unchanged builds.

## Stable installer paths — delivered at f14b02a

The installer keeps startup commands at
`~/.local/opt/nuncio-testing/bin/{nunciod,nuncio-cli}`. Verified versioned builds
are retained; `bin -> current/bin` and one atomic `current` replacement activate
updates. Repeat installation verifies and reuses the existing build. An OS lock
prevents concurrent activation. Old-layout migration, failed downloads/copies,
interrupted activation, tampering and unrelated-path preservation are covered.
Stop and restart the daemon through the permanent command path to use an update.
No profile, account or shell configuration is changed by installation.

The failing stable-path regression was reproduced first (exit 1). All 20 installer
tests pass. The complete script suite passes 40 tests; Ruff check/format,
Bash syntax and ShellCheck all exit 0. Source/tests were signed at
`0c7083bf8edd4bd0334fef3b9d28ec4a62146d31`, then the bootstrap and guides were
integrated in signed/pushed `f14b02a223ba95d95a0ce3f3e8f680910189869e`.
The bootstrap's immutable source SHA-256 is
`b8856606bcdcc211e2b34c147902109895818cc4825afd8e12b082d4f6d396a0`;
historical `git show` bytes were independently matched. All 67 checked local
documentation links resolve; whitespace and scoped Gitleaks passed.

Actual rebuild 34793311166 passed all ten jobs; security 34793311124 passed all
seven at f14b02a. Watcher 93018 exited 0. Exact run/job/API observations are in
`test-results/stable-installer/hosted-delivery-final.json`. Both hosted lint
evidence archives were digest-verified: all 40 script tests
and all nine commands passed on macOS and Linux. Exact logs and artifact digests
are in `test-results/stable-installer/hosted-scripts/verification.json`.
Local tests are not used as a substitute for these hosted results.

Actual public curl-to-Bash checks passed: fresh installation, repeat installation,
and migration from the old layout using the qualified 7508e25 package. Both
binary inodes and the receipt were preserved on reuse. After CI completed, the
same public command updated that same prefix to f14b02a and retained every byte
of the previous build. Both permanent command paths worked. The new package's
470 manifest files, production metadata, ARM64 binaries,
versions and eight CLI cases passed. Archive SHA-256:
`1c6283dfdf16b37c8ebcf359c65387cffd1de985211fd0b645c4d645ff834f5d`.
Selected CI run 34793311166 attempt 1,
artifact 10328798490. Retained temporary prefix:
`/private/tmp/nuncio-curl-acceptance-01p6bwb2/testing prefix`.
Normal-environment snapshots were unchanged. No daemon, provider action, normal
installation or local compilation ran during these public installer checks.

Exact commands, exit statuses and retained installations:
`test-results/stable-installer/{red,installer-checks,checks,pin-source}.json`,
`installer/public-verification.json`, `installer/repeat-and-migration.json`,
`update/public-verification.json` and `checkpoint-receipt.json`.
This final report-only checkpoint uses `[skip ci]`; it claims no separate CI run
or fresh local reproducible build pair. Prior candidate evidence is preserved.
No watcher or test process remains active. Next action: laptop feedback or the
pending Google registration approval; named live/native acceptance remains
deferred and the original full goal is incomplete. Latest continuation:
`test-results/checkpoint-final/current-execution.json`.

## Individual-account CLI help — delivered at7508e25

The user reported unclear account targeting. The account help now explains
`account list` → copy the entry's `id` → `ACTION --account ACCOUNT_ID`, distinguishes
the local profile from its accounts, gives examples, and describes every action.
`--account`, display-name edits and configuration versions have explicit help.
Bare `account` displays the guide and exits 2; explicit `--help` exits 0.
Scripted `--json` missing-action errors keep their structured contract.
The subprocess regression reproduced the missing-help failure (101) before the
fix; all four output tests then passed. Twenty-one action help pages and seven
examples were checked locally without provider access. All five relevant gates
passed: CLI tests14/0/0, test-harness CLI tests15/0/0 (including actual daemon
restart/authentication), formatting and Clippy in both configurations. The first
harness run failed because the sandbox refused loopback binding (EPERM); the
unchanged test passed with authorized local binding. Both results are retained.
Signed/pushed7508e25194843e069bcabcdbd1e632d181ef4ac2; signature and exact
remote head verified. Actual rebuild34776720576 passed all ten jobs; security
34776720552 passed all seven. Watcher93518 exited0; exact terminal/API results
are in `account-help/hosted-delivery-final.json`. Public installer15198 exited0,
selected that source/run/attempt 1 and artifact10323998130. The hosted archive
SHA-256 is `7df994d6abd1af0031c032d57a02b4fcbcaf61b6df34596b70a5209548024caf`.
All470manifest files, both ARM64 binaries/versions and eight packaged CLI cases
were checked; no profile was created and normal-environment snapshots stayed
unchanged. Retained installation and exact receipts are recorded in
`test-results/account-help/installer/public-verification.json`. No daemon or live
provider action ran during installation verification.
Exact commands, logs and continuation: `test-results/account-help/` and
`test-results/checkpoint-final/current-execution.json`. Google registration approval
and named live/native acceptance remain pending. The latest testing download is7508e25; prior82a92a0 local reproducible pairs
and manual-candidate evidence remain preserved as historical qualification.
This final report-only checkpoint uses `[skip ci]`; it does not claim its own
hosted run or a new local reproducible pair. No active test/CI process remains.
Next action: laptop feedback or explicit approval of the previously prepared
Google registration worksheet; live/native acceptance remains deferred.

## Google registration owner supplied — September 13

The user supplied the owner account. The concrete proposed Cloud/consent/Desktop
client and GitHub-secret settings are in [GOOGLE-APP-REGISTRATION.md](GOOGLE-APP-REGISTRATION.md);
the owner address and exact secure file destination are retained only in the
ignored local `test-results/google-app-registration/APPROVAL.md`. Approval for
those remote settings is pending. No Google Cloud or live account action ran.
Current official Google documentation and the implemented scopes/trusted-push
workflow were checked. External Testing requires reconnection after seven days
for these scopes. Software82a92a0 qualification remains unchanged; no rebuild
is needed for this documentation-only checkpoint. Exact validation and checkpoint
receipts belong in `test-results/google-app-registration/`.

Documentation checks passed: 366 source/test/script files match qualified
82a92a0, ten local links resolve, whitespace and scoped Gitleaks exit 0. The
initial source comparison used a pre-correction receipt and detected the known
scheduler test change; comparison against qualified82a92a0 resolved that baseline
selection error. Both results are retained. The updated hourly email was verified
SENT at13:55:25Central, message1a09c1fcdf16d47c, exact plain/HTML and234378-byte
23-task chart metadata. Next due14:55:25Central during active execution.

## Selected-candidate handoff correction

The manual worksheet's `dist/final-candidate/EVIDENCE.json` now selects verified
software82a92a0 and local archive5dbed9e9. Its embedded metadata and both binary
hashes were checked against the actual archive; all352production source hashes
remain unchanged. The former164b021 evidence is preserved byte-for-byte in
`EVIDENCE-before-guided-setup-82a92a0.json`. No archive or executable was changed.
Receipt: `test-results/account-setup-ux/candidate-selection/receipt.json`.
This documentation-only checkpoint uses `[skip ci]`; existing82a92a0 CI remains
the software qualification. External registration/live/native acceptance remains
pending; no independent implementation or live process remains.

## Latest continuation — guided setup delivered

Signed/pushed software `82a92a06eec5c6a674dde25981293404fd074b59` is qualified:
full guided-setup offline gate35/0 (328 tests per workspace), followed by both
scheduler corrections passing Linux22/macOS22 and fmt/bothClippy. Fresh clean
package pair34590/0 produced identical5dbed9e9 archives,22 extracted checks and
469 manifest entries each. All ten hosted rebuild jobs in34771537477 and all
seven security jobs in34771537540 passed; watcher86694 exited0 and is finished.
All ten job-evidence ZIPs were digest-verified and their command results checked.

Actual public installer58846/0 selected this source, run34771537477 attempt 1,
artifact10322850026. Hosted archive9dc2d94e,469-file manifest,both ARM64 binaries,
versions,guided command help and nonterminal refusal were checked independently.
Normal-environment snapshots were unchanged; no daemon or live/native account
access occurred. Exact receipts and retained paths:
`test-results/account-setup-ux/{hosted-delivery-final,final-package-final}.json`,
`hosted-evidence/all-job-evidence.json` and `installer/public-verification.json`.

The security scan leaves45 open classified alerts; new64 concerns synthetic PTY
transcript reporting. The driver rejects supplied fixture passwords before output
and is excluded from production packages. No production credential exposure was
demonstrated; no alert was dismissed or suppressed. Exact review retained.

The concise setup email was sent and independently verified (exact plain/HTML,
SENT and recipient) at12:45:53Central: message1a09be026ffbf31b. It gives update,
startup and one guided account-add command, and identifies Google registration
as pending. Receipt: `test-results/account-setup-ux/setup-email-receipt.json`.
This final documentation-only checkpoint uses `[skip ci]`; its own remote CI is
not claimed. Signature/push evidence belongs in `account-setup-ux/final-docs/`.
Software, package and hosted qualification retain source82a92a0. No watcher or
test process remains active. Resume for concrete laptop feedback or the missing
Google registration setup authorization; do not repeat unchanged builds.
Google app registration is absent; its owner was supplied in the latest continuation.
Registration/settings changes and named live/native acceptance are unapproved;
the original full goal remains incomplete. Latest exact state:
`test-results/checkpoint-final/current-execution.json`.

## History — guided setup implementation and earlier CI

Signed/pushed software `78d4d9e2cf76c78fb6c02832589b63439fd7dae9` implements
`account add`, hidden terminal password entry, advanced verified-TLS MailPlus
settings, bundled Google registration support and safe browser cancellation.
The full 35-command offline gate passed: both workspace configurations 328/0/0,
all standalone suites and quality checks exit 0. The later CI wrapper also passed
all 34 current script regressions and trusted event/ref checks. No Google app
registration exists; no real account/remote settings changes have been made.

Actual security run34769543129 passed all seven jobs. Rebuild34769543130 passed
nine jobs but Linux system tests failed at scheduler recovery after cancelled
provider backoff. Do not qualify that run's artifact. Original macOS focused
checks passed five times and the unchanged Linux suite passed22/22. A deterministic
Linux experiment reproduced the timeout with seven valid800ms Google responses:
the original8sec recovery budget included the remaining6sec Retry-After wait.
The corrected experiment passed by separately bounding deadline eligibility and
subsequent recovery, retaining8sec convergence and all original assertions.
Tracked regression adds the valid latency, continuous pre-deadline request checks,
and success-after-deadline assertion; engine/adapter behavior is unchanged.

Relevant regression passed: macOS scheduler22/22, fmt and bothClippy
(shell67502/0); independent Linux22/22 (shell20835/0). Correction signed/pushed
`b2b3ed5732f5884d1a1dc6b33daf18aa3cc3bc85`, with exact remote/signature receipt.
Two fresh production builds passed (81893/0): identical8dc16d97 archives,
22 extracted checks,469 manifest entries and237 notices each. Retained extraction:
`/private/tmp/nuncio-account-setup-package-ijqr8s4r/nuncio-0.1.0-rc-aarch64-apple-darwin-b2b3ed5732f5`.
Exact hashes/metadata: `test-results/account-setup-ux/package-final.json`.

Hosted rebuild34770885579 failed the earlier fixed1.4sec beta-progress
assertion (job103760149174); other jobs/security remain under watcher40728.
The next correction waits up to8sec for independently observed beta Gmail
progress during a20sec provider backoff, checking alpha makes no early requests.
The explicit cancellation and8sec post-eligibility recovery assertions remain.
Relevant checks passed: macOS22/22 plus fmt/bothClippy (92540/0),
Linux22/22 (51695/0). No engine behavior changed.
Next observe their exact terminal results, then run the prepared
`test-results/account-setup-ux/installer/verify-public.py` against the successful
build. Final docs/checkpoint and concise setup email follow. No normal installation,
native credential access or live account action. Latest exact handles:
`test-results/checkpoint-final/current-execution.json`.

The existing guide is [ACCOUNT-SETUP.md](ACCOUNT-SETUP.md); original scope and
remaining external acceptance are preserved. James reports the prior laptop quick
start worked; named live/native checks remain deferred, and the full goal remains
incomplete. App registration is a one-time maintainer task, not per-account setup.

Latest hourly email: September13 at12:13:53Central, verified SENT message
`1a09bc2db1046d78`, exact plain/HTML and233793-byte23-task chart. Guided setup
estimated90% with0.5–1.25 active hours remaining at send time. Next due
13:13:53Central/18:13:53UTC during active execution. Receipt/body/chart:
`test-results/status-emails/2026-09-13-1214*`.

## Curl-to-Bash testing installation delivered

The requested Apple Silicon installation/testing guide was emailed at 13:23
Central to james@kof22.com, message/thread `1a096dc5b5c3290a`. Independent readback
verified SENT, recipient, subject and complete body. It includes the public
installer, prerequisites, fresh-profile startup/status/restart/shutdown checks,
account/provider boundaries, updates and cache repair. Exact body and receipt:
`test-results/curl-bootstrap/installation-email*`. This separate how-to email
does not replace the hourly chart schedule above.

Cache question: engine/API/CLI expose `Maintenance.RepairProjection` through
`repair --account ID --scope mail|calendar`, with local `--dry-run` and awaited
provider rebuilds via `--wait`. Existing drafts and durable operations survive;
failed staging preserves the visible projection. There is no immediate
discard/refetch-later or per-object eviction command. Actual packaged help was
checked (`test-results/curl-bootstrap/repair-help.json`); no repair was executed.

Signed/pushed `57c619e` publishes the root `install-testing.sh`; both READMEs and
TESTING-INSTALL.md give the one-line command. It pins the reviewed Python
installer by immutable commit and SHA-256, while selecting the newest successful
retained application build. Nine new offline subprocess tests, all 31 script
regressions, Ruff lint/format, Bash syntax and ShellCheck passed. Source, exact
staged scope, signature, push, links and scoped Gitleaks were checked; prior
411 captured source hashes stayed unchanged.

Actual public pipeline shell `46985` exited 0. Bootstrap bytes matched the
checked source; the installer selected `c662e48`, run `34706301412` attempt 1,
artifact `10302106519`. The 467-entry installed manifest, clean metadata and
both ARM64 headers were independently verified; both version commands passed.
The retained installation is under `/private/tmp/nuncio-curl-acceptance-wtxpbkos/`.
Normal-environment snapshots were unchanged, with no build, daemon or provider
action. Exact full hashes, commands and installation path:
`test-results/curl-bootstrap/public-verification.json`.

Hosted rebuild `34710211208` passed all ten jobs and security `34710211290`
passed all seven jobs at `57c619e`. Watcher `63008` exited 0 at 13:25 Central;
actual terminal API outcomes, exact heads and job results are retained in
`test-results/curl-bootstrap/hosted-final.json`. No watcher remains active.
The public pipeline's earlier successful `c662e48` artifact remains separately
identified; no new download or native/provider compatibility is inferred.
This final report-only checkpoint uses `[skip ci]`; its receipt belongs in
`test-results/curl-bootstrap/final-docs/`. Native/live acceptance remains
explicitly deferred, and the full goal tool retains its prior blocked status.
No native apps, normal installation, release, merge or remote setting change
is included. Latest exact continuation: `test-results/checkpoint-final/current-execution.json`.

## Final continuation — documentation CI passed; live acceptance deferred

Documentation checkpoint `c662e48` passed all ten rebuild jobs in run
`34706301412` and all seven security jobs in run `34706301536`. Watcher `38391`
exited 0; exact heads, job outcomes and logs are retained in
`test-results/account-management-final-docs/hosted-final.json`. The 411 tested
source hashes remain unchanged. Software/artifact/installer qualification at
`164b021` remains separately recorded below.

The final worksheet review added explicit native Keychain creation, restart,
credential persistence and approved deletion observations. No native or live
check ran. This last change affects four Markdown files only. Its documentation
checks and signed/pushed receipt are in `test-results/acceptance-handoff/`.
The commit uses `[skip ci]` to avoid repeating unchanged software builds after
recording their results; it is not claimed to have its own hosted run.

No implementation, packaging, installer or offline verification remains active.
The next action requires James to resume the named live/native checks in
MANUAL-ACCEPTANCE.md. He requested full mocks for now; do not ask for accounts
again or begin native-app work. Keep the goal incomplete and use
`test-results/checkpoint-final/current-execution.json` for the exact blocked
turn count, last checkpoint and hourly email deadline.

Latest hourly email: September12,11:40Central, message/thread1a0967dcff0032a4 independently verified SENT to james@kof22.com with211603-byte inline20-task PNG (original16 plus dependency/security/API-design/future-roadmap deliverables). Receipt and report: test-results/status-emails/2026-09-12-1140*. Nextdue12:40Central/17:40UTC duringactive execution. The report records all current software/artifact/CI/installer checks passed, with0.5–1hour finalcheckpoint/handoff and3–6hours deferredlive/native acceptance estimated.

## Current handoff — account and testing delivery verified at 164b021

All authorized account-management implementation and offline delivery checks passed. Signed/pushed software164b021 has49RPCs/schema23; full34-command gate4169 passed322tests per configuration and411unchanged source hashes. Fresh local pair36678 passed with identical4a9e33a9 archives/binaries,22extracted checks/467manifest entries/237notices each. Exact full hashes and operating/recovery instructions are in IMPLEMENTATION-REPORT.md and REQUIREMENTS.md. The earlier failed fixture/whitespace and security-yank results remain preserved in VERIFICATION.

Actual rebuild34704460234 at164b021/attempt 1 passed all 10 jobs; all11artifacts retained and independently digest/size/CRC checked (capture97079/0). Actual unmodified installer55011/0 selected artifact10301603739, validated its22package checks and467-entry manifest, and installed under `/private/tmp/nuncio-testing-164b021-blrvan5v/testing-prefix/nuncio-0.1.0-rc-aarch64-apple-darwin-164b021750a7-run-34704460234-attempt-1`. All27help/version/account parser checks passed. No compiler, daemon or provider action ran; normal-environment snapshots and installer source stayed unchanged. Hosted archivea9e9a1f7 is verified against its own environment/receipt, not assumed equal to local4a9e33a9. Evidence: test-results/testing-installer-current/.

Actual security34704460237 passed all 7jobs. All3complete-lock scans passed;9action findings are fixed, no newfindings,44previously classified findings remainopen. Rust scanned435files, with23semantic warnings only in archivedreferencefiles. Source/sink classifications, exact analysis IDs and logs are in test-results/security-ci/hosted-34704460237/ and SECURITY-CI.md. No alerts dismissed, PRsmerged/closed or remote settings changed. All11openPRs reviewed and addressed by checked feature-branch replacements; later disposition remains separately authorized.

The requested future PO/PM roadmap and API-publication design are delivered and linked from READMEs. They do not authorize or expand native implementation. All delegated tasks are complete; no agent watcher remains necessary. Root is closing a documentation-only checkpoint, preserving411tested sources and the verified164b021 artifacts. Read test-results/checkpoint-final/current-execution.json and test-results/account-management-final-docs/ for its exact commit/push/CI receipt. A report-only push may trigger new hosted jobs; observe that exact run without claiming it already passed or rebuilding merely to relabel the software artifact.

After this report checkpoint/any active observation finishes, the sole full-goal completion condition is deferred named live GoogleG01–G10, SynologyS01–S05 and native-keystore/finalsignoffX01 acceptance. James asked for full mocks for now; do not ask again for accounts or access them without new authorization. Keep the goal incomplete, and follow its three-consecutive-turn blocked-state rule when no independent work remains. No nativeapp, release/tag, normal installation or remote-setting change is authorized. Hourly chart email remains authorized to james@kof22.com; next12:40Central/17:40UTC duringactive execution.


## September 12, 16:22 UTC — current package verified, hosted checks active

Signed/pushed `164b021750a70b49a1e060602f653f22cd14a46c` contains the 73-file account/installer/documentation checkpoint and follows the checked `1f47fe4` dependency/pin correction. Exact remote head and signatures verified. The initial staged whitespace check found two Markdown hard-break spaces in API-PUBLICATION-PLAN.md; converted to blank lines, rechecked and committed. All 411 tested source hashes stayed unchanged. Receipts: `test-results/checkpoint-account-management/` and `checkpoint-security-followup/`.

Fresh package pair shell36678 exited0. Both clean builds produce identical `4a9e33a91c11d3dbdf376ebd01827f9bd645f4fbbce03517fa8fad04f13af7cc` archives and binaries; 22 extracted checks and 467 manifest entries verified per archive, 237 notices, no changed inputs/differing files/missing README links. Selected `dist/final-candidate/verified/nuncio-0.1.0-rc-aarch64-apple-darwin-164b021750a7.tar.gz`; adjacent receipts and EVIDENCE.json identify current source. Prior b26d2c93 archive and receipt are preserved. Metadata dirty:false, no test features, Rust1.97.1/macOS ARM64/Appleclang21/epoch1789229447. Package input freeze is released for root's final documentation only; production source stays frozen.

Actual new-source rebuild34704460234 is watched by testing_installer/shell62703; six of ten jobs passed at last report. That agent owns final ten-job evidence and real hosted artifact installation under a fresh temporary prefix, with no normal profiles/daemon/real accounts touched. Actual security34704460237 is watched by calendar_api_audit/shell60584; six of seven jobs passed (all3advisories and Actions/JS/Python), Rust pending. Nine prior action findings are now fixed automatically by new analysis; no manual dismissals. Other findings remain subject to current-run classification. All agent outputs stay in ignored receipts, root owns final tracked docs.

Next: retain both existing terminal CI results, resolve any demonstrated failure, verify actual installer receipt, then finish current report/matrix/README/TODO and one signed documentation checkpoint/push. No new production build is required solely for dated report edits. Full goal still requires deferred, named live/native acceptance; do not mark complete. Next hourly chart email11:40Central/16:40UTC. `test-results/checkpoint-final/current-execution.json` is the latest machine-readable handle/agent state.

## September 12, 16:07 UTC — full account-management offline gate passed

Shell 4169 exited 0: all 34 commands passed, both workspace configurations passed 322 tests each with zero failed/ignored, and all 411 captured source hashes remained unchanged. Independent named suites passed: Google mock 26/system 22/actual CLI 30, operations 8; IMAP contract 2/system 28/actual CLI 14; recovery 5, repair 2/2, migration 1, reconciliation 3, multi-engine 1, security 3/2, resources 4/3, release isolation 2, external client 1. Six independent Python service scripts and 22 script regressions passed. Independently inspected 196 RPC-auth records and all 46 distinct migration SIGKILL receipts. Evidence: `test-results/account-management-final-all/{final-summary,exit,source-verification,egress}.json` and `current-all/`. Earlier failed gate evidence is preserved.

Signed local checkpoint `1f47fe4e64cc7bbf4c10e2cb002d44aa1ed03167` contains the tested yanked-lock correction, existing Dependabot label fix, nine action pins and PR/security reports. It is not pushed yet; root will push it with the checked account/installer checkpoint. Exact 9-file scope, unchanged original source, lock hash, Gitleaks/whitespace/signature and command receipts: `test-results/checkpoint-security-followup/`.

Next: finish current-source documentation, validate the explicit 73-file account/installer/docs scope, sign and push the account checkpoint, then run the prepared fresh package pair at `test-results/account-management-package/run.py`. Track actual new-source rebuild/security CI and verify the new hosted testing artifact through an isolated temporary-prefix installation. No normal environment or live provider access. Root owns these steps; `testing_installer` is finishing five documentation files only. Source remains unchanged through packaging.

The requested senior PO/PM roadmap is delivered in `POST-ENGINE-ROADMAP.md`: proposed native Mac alpha, daily mail, daily calendar, dependable personal release, then one evidence-backed expansion. It begins after current engine/CLI acceptance. The estimated 17–27 engineering weeks assumes one senior engineer at 25 focused hours/week; it is planning, not current implementation scope or a commitment.

Actual older-source rebuild run 34702939717 at 549a598 completed successfully, all ten jobs passed and all ten evidence artifacts were downloaded/inspected (44847/0). This establishes the pushed baseline only. Its sibling security run retains six successful jobs and the old yanked-lock failure, with 53 classified open code alerts and archive-only extraction limits. Corrected current-source hosted runs remain pending. Goal tool is active; full live/native acceptance remains deferred and required for completion. Next chart email is 11:40 Central / 16:40 UTC.

## Account-management implementation background (September 12)

James requires account management to be fully implemented before laptop alpha testing. Apple Silicon is confirmed; removal archives by default, with separate confirmed permanent deletion. Schema 23 storage, engine coordination, seven additive authenticated account RPCs (49 total), and CLI show/edit/add/reauth/auth-wait/cancel/pause/resume/archive/restore/purge are implemented but the full current-source gate and refreshed artifacts are still pending. Follow [ACCOUNT-MANAGEMENT-PLAN.md](ACCOUNT-MANAGEMENT-PLAN.md). The unbudgeted goal tool is active as of September 12, 15:55 UTC; this newly authorized implementation continues, with live acceptance deferred. Do not reuse the old artifact to qualify new source.

Focused evidence in `test-results/account-management/`: five lifecycle storage/recovery tests pass; original credential storage and draft suites pass; table-coverage test and previous-wire-contract comparison pass. Google lifecycle E2E, auth wait/cancel E2E, four actual archive/purge SIGKILL cases in one E2E, two separate account/operation system tests, one independent IMAP account E2E and one independent IMAP account system test pass. The latter verifies failed probes preserve configuration and failed credential deletion remains retryable after purge/restart. Actual REDs found missing commands, an upload publishing after archive, and admitted paused work starting a local attempt; fixes and evidence are retained. No full-current-source or new artifact claim yet. Schema history now independently retains versions 1–23; migration SIGKILL must cover 46 boundaries. Normal Clippy shell3324 and feature Clippy shell77366 both exited0. Archived resend and reconciliation admission were separately reproduced (19279/23627 exit101) and fixed transactionally; focused82295 exited0. Initial gate31288 exited1 on a synthetic schema18 restore fixture that retained schema23 columns. Corrected fixture; focused7104 passed4historical+3restore tests. Fresh full offline frozen-source gate is running in shell4169 via test-results/account-management-final-all/run.py; initial failed evidence remains at account-management-all/. Next: poll4169, resolve any demonstrated failure, then checkpoint/push and fresh package/actual CI. ACCOUNT-MANAGEMENT.md documents the implementation.

James explicitly requested another agent for a repo-run testing installer. Worker `/root/testing_installer` owns its script, focused tests, TESTING-INSTALL.md and minimal existing rebuild workflow artifact-upload changes. It will use authenticated gh access to successful feature-branch CI artifacts and an isolated testing prefix on Apple Silicon, without local compilation, profile/PATH mutation or normal-environment installation here. Root continues account work and owns this state/TODO/VERIFICATION and full gates. Do not revert the worker's concurrent edits. No releases/tags/live accounts are authorized.

James also explicitly delegated API publication/SemVer research to calendar_api_audit (API-PUBLICATION-PLAN.md), all open Dependabot/CVE remediation to sync_storage_audit (dependency manifests/lockfiles and DEPENDENCY-SECURITY-REPORT.md), and current README/documentation maintenance to testing_installer after its installer handoff. Installer and interim README/docs updates are complete (14focused/22script tests,152links checked). Development security workflows/docs are implemented with local YAML/structural checks; actual hosted Rust/Python scans remain pending. Root owns ACCOUNT-MANAGEMENT.md and shared state/TODO/VERIFICATION. Coordinate overlapping changes before the full gate source freeze. The API plan is complete; James subsequently assigned that agent security CI coverage for development, owning CodeQL and coordinated rebuild workflow triggers plus SECURITY-CI.md. Actual branches are main/dev/rc; no develop branch exists. GitHub security alerts0; original RustSec scan found vulnerable h2 and3 maintenance advisories, while rebuild/client scans pass. No remote publication, alert dismissal, merge or normal installation is authorized. James reaffirmed regular checkpoint commits/pushes; make coherent checked checkpoints, not an indefinite uncommitted accumulation.

The full Tasks 01–16 / R01–R16 goal is unbudgeted and incomplete. Software implementation, offline verification, local production artifacts, and actual CI passed at the checkpoint below. Live Google/Synology and native-keystore acceptance remain deferred, unapproved, and unverified. Consult the goal tool for its current active/blocked status; do not recreate the goal or redefine completion around mocks. Native apps are excluded.

Worktree: `/Users/james.maes/Git.Local/KofTwentyTwo/nuncio/.worktrees/nuncio-google-first-rebuild`; branch `feature/nuncio-google-first-rebuild`. Cargo cwd is its `rebuild/` directory. The original checkout remains on `dev`; original source, data, and unrelated work were preserved. James authorized coherent signed checkpoint commits and pushes to this feature branch after relevant checks. Merges, releases, normal-environment installation, remote settings changes, and live acceptance remain unapproved. Continue inline; no delegation or new issue is required.

After a context reset, read AGENTS → CLAUDE, shared personal rules/style, rebuild/AGENTS, this file, TODO, and the newest VERIFICATION entries. The approved September 10 spec/plan and engineering audit live in the worktree's parent `docs/`. They remain authoritative; do not restart planning. [IMPLEMENTATION-REPORT.md](IMPLEMENTATION-REPORT.md) is the delivery report, [REQUIREMENTS.md](REQUIREMENTS.md) is the current matrix, and [MANUAL-ACCEPTANCE.md](MANUAL-ACCEPTANCE.md) contains the remaining authorized-action gate.

## September 12, 16:00 UTC — current execution and future roadmap

Both full-gate workspace configurations passed 322 tests each, zero failed/ignored. Current named Google mock/system/E2E suites passed 26/22/30 tests, and operation system passed eight. Shell 4169 continues the remaining named checks; 411 source files remain frozen until its terminal receipt. These partial results do not establish the full gate yet.

James requested a background senior PO/PM to plan major development milestones after the engine and CLI are fully done. Existing agent `testing_installer` now owns only `POST-ENGINE-ROADMAP.md`, with outcomes, dependencies, gates, effort assumptions, and decision points. This is future planning; native apps remain outside the active implementation goal.

All 11 open PRs have been reviewed and their required changes incorporated or superseded. `OPEN-PR-REPORT.md` records exact heads, failed checks, fixes and proposed later dispositions. No remote PR actions occurred. The follow-up original lock now selects unyanked chacha20 0.10.2; formatting, strict Clippy, 790 tests, cargo-deny, and online cargo-audit with fresh RustSec and registry indexes passed. Dependency report/lock and the one-line Dependabot label fix are frozen, awaiting a signed checkpoint.

Hosted security run 34702939715 finished with six successful jobs and the preserved original advisory failure on the previous yanked lock. All 53 generated code alerts are being documented by `calendar_api_audit`; no confirmed production defect was demonstrated in its trace review. Archived reference crates had 23 semantic-extraction warnings; active workspaces had none. Nine action-pin corrections are ready; synthetic/guarded Rust/Python findings remain open without suppression. Hosted rebuild run 34702939717 has eight successful jobs and two release jobs still running. Both runs use pushed 549a598, which predates the account implementation.

## September12 checked dependency/security checkpoints

Signed81a1bc6ce6ea84d82d38f88e21679d51c42cf85b contains13dependency/compatibility/report files; signed549a5983db507670274422d6f702122b56210702 contains5security workflow/doc changes. Both pushed; exact remote549a598 verified (82025/0). Original final790tests pass,123source hashes unchanged, fmt/Clippy/cargo-deny and all3complete-lock cargo-audit scans pass with zero vulnerabilities/warnings. Scoped checkpoint Gitleaks scans exit0; whole-repo archivedpublickey finding remains separately recorded. Receipts: checkpoint-dependencies/ and checkpoint-security/. All11bot updates incorporated/superseded; bot PRs and defaultbranch remain unchanged pending authorized integration.

Only development-trigger hunk from rebuild-ci.yml entered security checkpoint; installer-upload block stays unstaged with new account implementation. API publication plan is still proposed/uncommitted. Security agent calendar_api_audit now owns actual hosted7job CodeQL/advisory verification at549a598 and branch findings; root owns rebuild/local gate. Hosted549a598 tests use OLD rebuildsource; they cannot qualify currentuncommitted accountcode. Security run34702939715 and rebuildrun34702939717 are active at549a598. Security agent is fixing9actual unpinned-action findings in original workflows;2Python cleartext findings are synthetic local-service credentials underprivate paths, retained/classified without dismissal or frozen-source edits. James subsequently requested addressing ALL open PRs; freshfullqueue is exactly11Dependabot PRs, no additional PRs. sync_storage_audit owns OPEN-PR-REPORT.md and currentfix for hosted complete-lock yank:original chacha20.10.1 (optionalrand chain) is yanked although priorlocal cachedindex audit passed; updatingto.10.2 with freshindex/fullgates. Security originaladvisoryjob103577787655 failed; rebuild/client pass. Preserve this failure, no suppression. No merge/closure/comment approval inferred; root account delivery continues. Fullgate4169 now passed normalworkspace322tests/0failed/0ignored; feature/named suites remain running, sourcefrozen411files.

## Verified software and artifact

- Signed/pushed software checkpoint: `6ff9bb9e7be23249c9f448276e08c3062efc44a7`. Signature and exact GitHub branch head were verified. Nine files / 47,868 diff bytes; source hashes, documentation, whitespace, and Gitleaks checks passed. Receipt: `test-results/checkpoint-interest/`.
- Full offline gate: shell `88082` completed with exit 0. All 34 commands passed; both workspace configurations passed 308 tests, zero failed/ignored. All 399 captured source hashes stayed unchanged. Every separate suite, six independent Python mail-service scripts, eight script regressions, dependency and client checks passed. Exact commands/results: `test-results/ci-imap-interest-all/{final-summary.json,exit.json,source-verification.json,current-all/}`.
- Actual hosted run [34684158773](https://github.com/KofTwentyTwo/nuncio/actions/runs/34684158773) completed successfully: all ten jobs passed, watcher `3872` exited 0. Every job artifact was downloaded and checked. Both core suites: 155 tests; mocks: 26 each; Google system: 37; E2E: 40 each; IMAP/resource: 46; release/client: 3 each. Both package commands exited 0. Proof: `test-results/remote-ci/34684158773/{run-final.json,downloaded-summary.json,watch-exit.json}`. Hosted archive hashes were not uploaded or inferred.
- Fresh local package pair: shell `51959` completed with exit 0. Two clean builds produced identical archives/binaries, with 22 extracted checks and 458 manifest entries verified per archive, 237 notices, no input differences, and valid README links. Source freeze is released. Proof: `test-results/task16-interest-package/`.
- Selected local artifact: `dist/final-candidate/verified/nuncio-0.1.0-rc-aarch64-apple-darwin-6ff9bb9e7be2.tar.gz`; SHA-256 `b26d2c93b57fbe31123ddaaa31a25fadb520e5d7fd78429ec603ee07078ed105`. The selected receipt is `dist/final-candidate/EVIDENCE.json`; older candidates and receipts remain preserved. Metadata records clean 6ff9bb9, macOS ARM64 / Rust 1.97.1 / Apple clang 21 / epoch 1789202823. Reproducibility is limited to identical recorded source/platform/compiler/SDK/paths.

The last production correction excludes optional IMAP Marked/Unmarked hints from coverage hashing while preserving full provider state. Its regression failed before the fix and passed afterward; all seven projection tests and the original independent-server cases passed. Real mailbox changes still alter coverage. Hash domain v2 gives existing profiles a one-time local coverage-token change on next promotion, with no schema/API/remote-cursor change. Original assertions and deadlines are retained. Earlier failures remain in VERIFICATION, including a Linux transfer timeout whose cause was not established; subsequent runs passed unchanged bounds.

## Historical next action at the previous delivery

1. Finish the documentation delivery checkpoint if the working tree still contains the final report changes. The prepared delivery state and completed commit/push receipt belong in `test-results/checkpoint-final/`. Verify that changes are documentation only and that the 399 tested source hashes still match before reusing the software evidence. Do not repeat the full gate or rebuild solely for dated report edits.
2. A documentation push can trigger another workflow. Record its exact run/handle in `test-results/checkpoint-final/current-execution.json` and its own `test-results/remote-ci/<run>/active.json`. If it is running, poll that existing handle/API and retain the terminal result; an observation timeout is not job termination. Never infer success from a state file alone. Do not keep creating documentation-only pushes while a workflow is active. Software CI at 6ff9bb9 remains independently established.
3. Once that delivery is closed and no new failure exists, only the deferred manual acceptance remains. James requested full mocks for now; do not ask for accounts again unless he resumes live scope. Rows G01–G10, S01–S05, and X01 need named resources/actions and explicit approval before access or effects. Use secure credential entry, never chat secrets. Do not mark the goal complete without those checks. If no independent work remains, apply the goal tool's three-consecutive-turn blocked audit honestly rather than inventing new audits or work.

The report and matrix describe the verified software checkpoint; documentation-only revisions do not change that production code or relabel its archived build-time reports. The checkpoint receipts and Git history identify later documentation revisions without a circular rebuild/report/commit loop. If production code changes, run the appropriate regression and create a fresh verified artifact.

## Hourly status email

Recipient: `james@kof22.com`. Every hourly update includes all 16 major tasks, estimated completion percentages, and remaining active hours in an inline PNG chart, HTML table, and plain-text fallback. Refresh from actual evidence and explain estimate changes. Percentages are engineering estimates; 100% means implemented and applicable offline checks passed, with live acceptance tracked separately.

Latest report: September12,10:40Central, message/thread1a096482a4909b90, independently verified SENT to james@kof22.com with208413-byte inline19-task chart (original16plus requested dependency/security/API-design follow-ups). Exact renderer, MIME/body, estimates and read receipt: test-results/status-emails/render-progress-1040.py and2026-09-12-1040*. It records pushed dependency/security checkpoints and normalworkspace322passed; current full gate/hosted/newartifact stillpending. Remainingestimate6–12offline hours plus3–6deferred live. Next due: September12,16:40UTC/11:40Central during active execution. Earlier receipts remain. The Gmail connector is separately authorized communication, not rebuild-provider acceptance evidence. No persistent inactive-session scheduler was established.
