# Verification evidence

## Verified repeatable local candidate

Corrected two-build comparison31790 completed0 September12 03:46UTC. Both builds
were fresh and each passed22 extracted-binary checks. Source/docs were unchanged;
no differing packaged files, no broken README links, and byte-identical archives:
SHA25647d65f00764da1cedb691109bd5b65c402ed253d69a40e2f0fe43fff13c0063f.
Canonical copy: dist/final-candidate/nuncio-0.1.0-rc-aarch64-apple-darwin-720081f661d0.tar.gz.
Daemon SHA256db0dce123719b9126b7aa5f09aa47e879d6d8c0759bf6a97dbda991c173e1bc3;
CLI SHA2561495aa640c31a3b45159005af8f040292af3d03ffacb41448d2b33e22163cac5.
237third-party notices, descriptor and extracted manifest verified. Commands/logs/
comparison are in task15-repeat-green/; canonical EVIDENCE.json/build/check/hash
sidecars are adjacent to the archive. No installation or publication.

Archive metadata records its uncommitted720081f source state and hashes. Its docs
are the build-time snapshot; this current report and adjacent evidence record the
completed verification. Same-input repeatability was established on this recorded
macOS ARM/compiler/SDK/checkout/build-path environment, not across differing build
environments or platforms. Only package.py/test_package.py changed after full
34-command gate58495; subsequent Ruff/format, package4/fullscript8 and both actual
clean packages verify those changes. Original comparison66388/1 is retained below.
Hosted CI and live/native-keystore acceptance remain pending.

## Controlled package repeatability failure and correction

Two unchanged-input clean packages66388 each passed22 extracted checks and all
README links resolved. Comparison exited1: archives/binaries differed. Exact
comparison/differing-string evidence is in task15-repeat/. The differing binary
strings were random generated-protobuf/OpenSSL build paths and OpenSSL build time;
metadata differed only by built_at. Sizes were equal;155daemon/88CLI bytes differed.
No source or documentation changed during the comparison. This is retained as a
real R16 verification failure, not counted as passing repeatability.

Packaging now recreates a stable exclusive build directory, refuses existing data,
uses the Git source epoch for OpenSSL and normalized tar/gzip metadata, and records
actual verification time externally. New regressions failed before helpers existed;
package4 tests now pass, fullscript8 tests pass under egress denial, Ruff/format0.
The corrected full two-build comparison is running from task15-repeat-green/run.py;
repeatability remains unproven until its exact hash comparison succeeds. No normal
installation or live-provider/native-keystore access occurred.

## Full integrated offline egress gate — passed

September12 03:31UTC: shell58495 completed exit0. All34 commands passed, including
both builds, Rust/Python formatting/lint, both Clippy configurations, both workspace
runs (305passed/0failed/0ignored each), all18 named suites, the six-script independent
mail-provider suite, six script regressions, cargo-deny and external generated
client/boundary checks. Source verification matched all396 snapshotted files.

Named Rust counts: Google mock26/system22/E2E27, operations6; IMAP contract2/system27/
E2E13; recovery5, repair2/2, migration1, reconciliation3, multi-engine1; security3/2,
resources4/3 and production isolation2. External generated-client E2E1. Repeated
workspace/named executions are not distinct unique tests. Every recorded required
suite passed without ignored tests. Logs/commands/results/counts/source hashes and
benchmark JSON are preserved in test-results/task15-all-green/. This proves the
local integrated run, not hosted CI or live provider/native-keystore acceptance.

The subsequent package-only README correction addresses four links to unshipped
session/plan files; Ruff/format and package unit2 pass. A fresh archive and controlled
repeat-build comparison are next. No new production engine change after the gate.

## Workflow static validation

Official actionlint1.7.12, downloaded only into ignored test-results/actionlint-tool,
passed the new rebuild workflow exit0 with no diagnostics. Archive SHA256 matched
the publisher's checksum: aba9ced2dee8d27fecca3dc7feb1a7f9a52caefa1eb46f3271ea66b6e0e6953f.
Receipt, original checksum file and log are retained there. This supplements the
existing YAML/job-binding and six real-subprocess failure-propagation checks; it
is not evidence of hosted CI execution. No normal-environment installation.

## Integrated egress gate failure and resource sampler correction

Gate98403/1 stopped at the memory test before workload: setuid /bin/ps spawn was
EPERM under unchanged Seatbelt policy. The isolated reproduction failed1; an
unprivileged identical copy was SIGKILLed and discarded. Native libproc numeric
sampling succeeded under the same restriction. New process_stats regressions:
red1 missing module; green70517/0 (2tests macOS) and97213/0 (2tests Linux,
network-disabled container), actual32MiB touched allocation and exited-PID refusal.
First actual workload43390/1 then hit its1s sync deadline because subprocess
sampling blocked the current-thread mock runtime. spawn_blocking correction:
41482/0, all3 actualresourceE2E pass;8cycles16MiB in34.930s, peak255392KiB,
idle148816–225456KiB. Original1s timeout,128MiB growth bound and independent
remote bytes/counts assertions retained. No production code change.
Evidence: task15-all/{exit.json,failed-all/}, task15-egress/{ps-red.*,
ps-unprivileged.*,sampler-*,resource-sampler-first/,resource-green2.*,
resource-metrics.json}. Fresh full gate58495 is running; no integrated pass claimed.

## First verified local production archive

Clean packaging80508/0 completed: `dist/task15-notices/
nuncio-0.1.0-rc-aarch64-apple-darwin-720081f661d0.tar.gz`, SHA256
`955df7cff297f077c0c3bae0f58a638f5d661088dbec7be3fa941dd4dce244cb`.
Fresh isolated target build completed in1m41s; no test feature/mock artifact was
accepted in the build graph. Frozen descriptor matched.237 compiled third-party
notices verified, including upstream files omitted from crate distributions.
Extracted manifest hashes matched and22 production-binary help/version/test-control
checks passed with expected exits0/1/2 and no profile creation. Adjacent build JSON,
verification JSON and SHA256 sidecar retained. The archive identifies its dirty
source state and source hashes; it is a local candidate, not installed/published or
proof of live keychain/provider compatibility. First attempt62175/1 stopped on a
missing notice; original log retained. Full CI/egress integration remains pending.

Archive executable hashes (also recorded in task15-package/binaries.json):
- nunciod16,423,360bytes:73ee24c2d8df3af6934fcecd47fbfabee84f1cce5b8f638b89791809bcc8ab2b
- nuncio-cli3,544,544bytes:ef952632ee7487284b99a492a0881d3d0f049ce022eab3d27dd66968ccfc521a

These binaries include keyring4.2.0/Rustls PEM parsing. They supersede the older
Task14 binary hashes for this candidate. Final docs will be repackaged after the
integrated gate. Local documentation-link checks and Markdown fence checks pass0.

## Task15 dependency and contract affected gate — passed

Targeted keyring4.2.0 lock update14944/0 and fetch0 removed derivative, instant and
rustls-pemfile. Native Rustls PEM parsing retains certificate validation. MacOS
workspace/all-target check69065/0. Cargo-deny advisories/licenses/sources0: zero
errors/warnings,352 license helps. Eight-command gate24095 completed all0:
fmt/bothClippy, core/client tests, actual test binaries, IMAP system27/E2E13 and
Google system22. Per-run counts/logs: task15-dependencies/. OS keychain access and
live provider compatibility remain unverified; tests use synthetic stores.

Independent generated client/production CLI/mock normal+build dependency scans0:
100/142/154 package names; forbidden engine/proto/client dependencies absent as
appropriate. Descriptor test1/0. Package script unit2/0. CI runner unit2/0 includes
actual exit17 propagation for all six jobs and complete18-suite group coverage.
Evidence: task15-contract/. Full newly integrated runner and archive are pending.

Linux egress first attempt16516/2 timed out with default ICMP rejection. Independent
TCP-reset diagnostic returned ECONNREFUSED with one counted rejection; the corrected
rule then passed parent/child IPv4/IPv6 refusal with2 independently counted rejects
per family. Success0/failure17 propagation and separate post-run chain/route cleanup
checks passed in a disposable network-disabled Debian container. MacOS parent/child
and actual external-client sandbox tests passed earlier. No host firewall was changed.
Container evidence: task15-egress/{linux-success.json,linux-exit-0.json,
linux-exit-17.json,linux-cleanup.log}; original failure retained. This is local
Linux-container evidence, not a claim about hosted CI execution or Docker service
network isolation, which still needs implementation/checks.

## Calendar checkpoint and contract freeze

Signed checkpoint720081f661d0038de661953b5b82da8c4a2c4b44 was committed/pushed;
independent ls-remote matched. Staged Gitleaks35,972bytes/whitespace passed0, GPG
signature good with unknown local ownertrust. Receipt: checkpoint-calendar/.

Descriptor regression82556/101 rejected an empty freeze (six files expected).
An attempted freeze lookup correctly refused multiple stale build-directory
versions; selecting the actual cargo build-script output32938/0 avoids that
ambiguity. Frozen six-file descriptor27,834bytes SHA256
21ab39c24af1fd2029001195f3016e03f3da20b1e7ead0462751320012576bf5;
contract green36458/0 passed1. Package script missing-file red1; manifest-tamper
and compiler/test-control environment regressions pass2/0. No archive yet.
These are later Task15 checks, separate from the full gate below.

## Full Calendar-role regression gate — passed

September12 01:49UTC: `python3 scripts/verify.py --all` completed exit0
(shell92959). All25 commands passed: both builds, fmt, both Clippy configurations,
normal workspace304 and feature workspace304 test executions, plus18 named suites.
Named results: mock26, Google system22/E2E27, operations6; IMAP contract2/system27/
E2E13; recovery5, repair2/2, migration1, reconciliation3, multi-engine1; security3/2,
resources4/3 and production isolation2. Zero failed/ignored. These are per-run
executions; repeated workspace/named runs are not additional unique tests.

All captured source hashes were unchanged during the gate. Logs/results/counts,
independent provider/resource metrics and fresh binary hashes are preserved in
`test-results/task14-all/`. Summary script initially failed on duplicate test-target
names across packages; corrected to report unambiguous per-run totals and then
passed0. This reporting correction did not change or rerun product tests.
Production daemon16,402,304bytes SHA256
`33daba77108e7ccdc875121e63f17c927ed0ded9692265c8deebbb252c062fbd`;
CLI3,506,544bytes SHA256
`9715785d0c29467b9e477499308e3b7c35fbea6aeff3099b47aeb076e661808d`.
Standalone client/egress and new release scripts are separate later work, outside
this gate. Dependency advisories, final packages/CI and live acceptance remain open.

## Release preparation — independent client and egress foundations

Standalone clients/smoke generates a System client from the v2 source protobuf in
its own Cargo workspace. Compilation47447/101 found fixture build-argument type
mismatch, corrected before runtime red49910/101 demonstrated unimplemented client.
Green7033/0 passed1 actual daemon/mock subprocess test, then external all-target
Clippy passed0. Independent fmt check0 and cargo tree normal/build scan0 confirm
no engine/storage/CLI/daemon/mock/Nuncio-client-library dependencies. Credentials
are bounded stdin input, no argv/output secret. Test verifies status and replayed
change identities, bad-token rejection and independent zero sends/notifications.
Evidence: task15-contract/. Main verifier/package integration is still pending.

MacOS process-local sandbox probe passed0 and denies external TEST-NET IPv4/IPv6
with EPERM, allows loopback, and passes the same assertions in a child. New
scripts/egress.py preserves underlying command exits0 and17 in real wrapper checks;
both JSON evidence files retain parent/child observations. No host firewall changed.
Linux hosted-runner branch is written but unverified; IPv6 route/counter behavior,
container internal networks, full-suite wrapper and CI integration remain pending.
Evidence: task15-egress/. This does not prove CI egress or remote CI execution.

Cargo-deny0.19.8 initial review99059/5 reported three unmaintained dependencies and
three rejected license allowances; exact findings in DEPENDENCIES.md and original
JSON in task15-dependencies/. Reviewed0BSD/CDLA permissive notices are now allowed;
licenses/sources check0 with374 license helps, zero errors/warnings. The three
advisories remain errors and require remediation; no clean dependency claim.

## Calendar limited-writer permission gap — focused verification

Current official [CalendarList](https://developers.google.com/workspace/calendar/api/v3/reference/calendarList)
and [sharing](https://developers.google.com/workspace/calendar/api/concepts/sharing)
references document writerWithoutPrivateAccess as an editor of non-private events
with private details hidden. Existing implementation rejected that role entirely.
Independent mock contract red25702/101 reproduced unsupportedRole; mock now masks
private fields in get/list/instances, preserves timing/identity, rejects private
PATCH/DELETE, permits ordinary create/edit/delete and counts notification effects.
Full independent contract green69060/0:26passed. Owner/writer full details remain
verified; reader masking is also checked. Mock and engine use independent code.

Production unit red97361/101 and authenticated API red78114/101 reproduced valid
edit refusal; actual CLI red16362/101 reproduced create refusal. Minimal change
in domain/calendar_change.rs accepts the role while denying existing private-event
update/delete/respond; other scope/ETag/organizer guards remain. Focused75329:
engine4/system1/build passed; CLI failed at new fixture restart without prior stop.
No product failure at that point. Corrected fixture force-kills first; focused53623
passed1/0. Actual CLI independently checks create/edit/respond/delete, exactly3
notification effects, private data absent after restart, unchanged remote private
event and no private mutation requests. System test independently verifies one
visible edit/notification, request replay and no private delete effect. Original
red/fixture failure and green logs retained under task14-calendar-role/.
Full workspace/offline gate and production refresh pending.

## Resource telemetry and shared provider budget — full affected gate passed

Signed checkpoint0805a7dc4b7e544195f3eb14e8c85db54c41a004 pushed to the approved
feature branch. Staged Gitleaks (45,360bytes) and whitespace checks passed;
git verify-commit reports a good signature (local ownertrust unknown), HTTPS push
and ls-remote completed0 (shell13278), exact remote hash matches.

Fifteen-command gate shell57763 completed exit0 atSeptember12 00:57UTC.
`python3 test-results/task14-resource-status/run-gate.py`: fmt/bothClippy;
engine130; API/CLI/daemon20; resource system4/E2E3; security3/2; Google21/26;
operations6; IMAP27/13; production-isolation2. Zero failed/ignored. Summarize.py
completed0; exact commands/statuses, canonical counts, source hashes, independent
resource/security observations and artifact hashes are under that evidence root.
Fresh local daemon16402304bytes,SHA256
24b7c7aa3521ffe871b7a6488829dc825b28566e0abc386d88bb1ba705d0b1fc;
CLI3506544bytes,SHA256
9715785d0c29467b9e477499308e3b7c35fbea6aeff3099b47aeb076e661808d.
Original RSS bound and independent remote-effect assertions remain unchanged.
These are local binaries; packaging, remote CI and live compatibility remain
unverified. API.md describes the implemented contract, with descriptor freeze
and external generated-client smoke explicitly pending.

New actual daemon/CLI regression failed101 (shell86524) because status omitted
resources; `task14-resource-status/red.log` retains the original failure. Engine
now exposes numeric process-local counters through new additive GetStatusResponse
field10/ResourceStatus and CLI JSON. Shared job admission64 covers mail/calendar/
operation tasks including backoff. Shared network admission allows2 exchanges and
at most64 active+waiting callers; RAII releases permits on cancellation. IMAP/SMTP
hold permits for commands/handshakes, not idle connection lifetimes, preserving
server-Sent pre-DATA checks. HTTP bodies/decrypted mail-wire bytes and successfully
staged pages/batches are counted without content labels. Store channel depth and
account-sequence admission are reported separately. No schema/dependency change.

New budget unit test91489 passed1/0 (64jobs,64request admissions,twoactive,
cancelled waiter release, counters). All-target feature checks74325/33887 passed;
featureClippy13625 passed. Fresh test binaries59074 built0. Actual focused status
regression85084 passed1/0: two independently held Google requests, third queuedjob,
cancel before Calendar request, unchanged remote effects, numeric byte/page growth,
zero active work afterward, restart counter reset with mail preserved. Evidence:
`task14-resource-status/{build,green}.log` and runs/google-e2e-*/resource-status.json.
Initial mixed Google/Dovecot test85437 failed101: it expected a network waiter
from CheckAccount, but that RPC correctly waits in the outer account coordinator.
Both provider holds were established; the new test timed out waiting at the wrong
queue, then performed bounded fixture cleanup. Original log retained as
mixed-fixture-account-lane.log. Corrected fixture uses an actual OAuth callback
for the third request, which directly contends at shared network admission;
assertions still require2active/1networkwaiter and no third provider request.
Corrected mixed case79577 passed1/exit0 in2.20s; actual OAuth callback waited
behind held Google and Dovecot exchanges. Independent mailbox/effects remain
unchanged. Broader15-command gate passed as recorded above. New assertions also cover load page-count parity,
backoff job retention and IMAP/SMTP subprocess resource snapshots.

## Resource admission and queued-worker recovery verified

Fourteen-command gate shell98949 completed exit0: format/bothClippy; engine129;
resource system3/E2E2; security3/2; Google21/26; operations6; IMAP26/13; release
isolation2. All suites zero failed/ignored. Commands/statuses, canonical counts,
source/artifact hashes and independent resource/security observations are under
`test-results/task14-resource-admission/`. Fresh normal daemon16352624bytes,
SHA25619caa6caae0a5496855abfa14ae901c69bcbf3ff99788c1dbeed5ac83d5cd8f4;
CLI3506544bytes, SHA25623f75c75837bc64b25a40cdac3d5a3e0abcff436bbf8d75d161664ac3659d4b1.
Local artifacts only; no package/installation/remote-CI/live claim.

After that gate, a new system regression reproduced worker termination when
64 account-request slots are occupied: queued-send preparation returned Busy,
propagated to the worker fatal path. Shell60230/101, no dispatched attempts/sends
at saturation, followed by operation_worker_unavailable. Original red log and
profile path are retained in task14-operation-admission/red.log. Minimal fix
defers Busy before attempt creation; all other errors retain prior handling.
Focused green shell49015 passed1/0. Subsequent9-command gate81153 completed all0:
fmt/bothClippy, engine129, resource_system4, operations6, GoogleE2E26, IMAPE2E13,
release_isolation2; zero failed/ignored. The test proves worker health after
saturation, no premature attempt/send, then one applied attempt and one independent
accepted send without restart. All64 read requests complete and no calendar
notifications occur. Evidence: task14-operation-admission/{red,green,results,
counts,source-hashes,artifacts} (logs/JSON as appropriate). Refreshed daemon:
16352624bytes,SHA256a9170b24aa58eabc14e125d28a4f7b50e40609526d19114b54def74a1175d347;
CLI3506544bytes,SHA25623f75c75837bc64b25a40cdac3d5a3e0abcff436bbf8d75d161664ac3659d4b1.
Signed checkpoint `a29812fadee371890972506ef2acae41ec9d3f11` committed (exit0),
verified with git verify-commit (exit0), and pushed (HTTPS exit0). Exact ls-remote
hash matched. SSH initially failed128 (agent refused signing); gh auth status
outside the network sandbox confirmed valid existing KofTwentyTwo credentials.
A command-only credential helper enabled HTTPS without changing remote settings.
Staged Gitleaks reported no leaks; staged whitespace check passed. No remote CI
inspection/merge/release/install/live action. Task14 and the full goal remain open.

## Repeated-fetch memory investigation — bounded preallocation verified

The failed admission gate remains evidence of a real RSS-bound failure; no
assertion was removed or relaxed. Subsequent diagnostics1104 and97219 both passed
2 tests, but were used for investigation rather than declaring the issue resolved.
Threads22 and database/WAL sizes stayed constant. Content-free native heap/VM
summaries after cycles2/8 showed live heap3,924,688→3,927,584bytes while freed
large-allocation resident regions grew178.1→244.2MiB (16→26 regions). This points
to allocation churn/caching rather than retention of entire mail objects. Evidence:
`task14-queue-admission/diagnostic-{1,2}/`. Method reference: Apple's
[Analyze heap memory](https://developer.apple.com/videos/play/wwdc2024/10173/).

GoogleHttp::read now allocates for an already-validated Content-Length, falling
back to64KiB, so buffer growth no longer starts from an arbitrary first network
chunk. Header and streamed-body limits remain unchanged. Profiled shell42408
passed2: peakRSS257072KiB, empty-large region count5→8. Unprofiled37116 passed2:
peak262784KiB, idleRSS149472→216528KiB across8cycles. Exact full samples and logs
are in diagnostic-3-preallocated/ and normal-preallocated/. The14-command
gate at `test-results/task14-resource-admission/` (shell98949) passed all0,
including fresh production isolation. The operation-worker follow-up above remains. The original128MiB post-warm-up
RSS bound and exact payload/provider-effect assertions remain intact.

## Account-request admission — initial failure and focused evidence

`Coordinator::acquire` previously bounded only active work, retaining unlimited
callers waiting for account sequencing. New unit regression shell39812 exited101
after the65th caller failed to reject. A64-permit admission guard, held alongside
account/active permits and released on cancellation, makes the test pass
(shell99903/0). Existing active-account limit2 is retained. Independent authenticated
freebusy flood test shell42256 passed1/0:65 incoming RPCs yield1 Busy refusal,
64 exact observed provider requests, and a subsequent fresh request succeeds.
No sends/copies/notifications occur. Test/source locations: coordination.rs and
resource_system.rs; logs `test-results/task14-queue-admission/{red,green,system}.log`.
The12-command relevant/provider/production gate stopped (shell14707/1) after
fmt/bothClippy0, engine129/0 and resource_system3/0. ResourceE2E had1pass/1failure:
idleRSS [199312,218480,245504,304048,372688,425984,430816,515200]KiB exceeded
the unchanged128MiB post-warm-up variation bound. No later provider/release checks
ran; no refreshed production hashes. Prior passing resource runs do not override
this failure. Preserved profile: resource_e2e/runs/google-e2e-ZtHsmp. Subsequent allocation investigation and verified minimal fix are recorded above. This does not
complete spawned-sync admission or resource instrumentation.

## Task14 resource workloads — relevant gate verified

Six-command `python3 test-results/task14-resource-workloads/run-gate.py` completed
exit0 (shell34619): format, both workspaceClippy modes, resource_system2, actual
resourceE2E2 and securityE2E2; zero failed/ignored. Exact commands/statuses/counts,
source hashes and load/RSS samples are retained beside the runner. The system
workload ingests10,000 provider messages through100 HTTP pages and verifies every
ID exactly once across100 authenticated API pages, missing-body metadata and
restart preservation. Latest sync=27433ms, list=1638ms. A
second test lowers the provider payload bound to4096 bytes, retains metadata for
an8192-byte body and refuses raw download. Initial test expected the wrong API
status; it was corrected to existing NotFound after inspection, with original
failure in resource_system/missing-body-status-contract.log. Focused corrected
case shell62253 passed1/exit0 before the full gate. No production fix was needed.

Actual daemon/CLI resource tests verify eight fetch/download cycles of an exact
16MiB binary attachment (wire 22958764bytes), independently matching the
provider bytes and observing no sends/copies/notifications. On macos/aarch64,
latest sampled peakRSS=388656KiB, baseline=37568KiB.
Full samples/iteration latencies are in attachment-resources.json. The finite
repetition check allows128MiB idle-RSS variation after two warm-up cycles (two
maximum-payload buffers); it is a regression bound, not proof against every leak
or a universal speed target. Another subprocess test refuses a sparse64MiB+1
attachment, preserving the draft/version and independent remote state across
restart. Harness adds only an owned-daemon PID accessor; no production/schema/API
or dependency change. Queue admission/concurrency/byte/batch instrumentation and
full Task14 gate remain pending; these workload tests do not complete Task14.

## Task14 security system and subprocess coverage verified

Nine-command gate `python3 test-results/task14-security/run-gate.py` completed
exit0 (shell20194): formatting, both workspace Clippy configurations, security
system3, securityE2E2, multi-engine1, Google system21/E2E26, operations6. Zero
failed/ignored. Review then strengthened the HTML resource trap to a counted mock
route and proved an unauthenticated probe increments its counter before testing
the CLI. Four-command follow-up `python3 test-results/task14-security-trap/run-gate.py`
completed exit0 (shell79748): formatting/bothClippy and securityE2E2. Exact commands,
statuses, counts, final source hashes and audit JSON are retained in those paths.
No production implementation changed; four existing workspace crates were added
as test-support dev-dependencies, with no resolved dependency version changes.

`security_system.rs` enumerates all42 methods across6 services and rejects168
missing/wrong/retired/wrong-scheme authorization requests before payload validation.
`support/security_inputs.rs` verifies account crossover, header injection, malformed
provider IDs, oversized frames/declarations, inert filename metadata and unchanged
independent remote state. `security_e2e.rs` uses actual daemon/CLI subprocesses:
private body reaches FTS and survives restart;37 files including populated WAL,
backup and30 original pre-redaction process logs contain none of10 synthetic
canaries. Python ordinary SQLite and wrong-key SQLCipher independently reject both
database and backup, preserving original bytes. Explicit synthetic keystore/OAuth
input files are excluded, not product outputs. Hostile MIME/control/HTML display
stays inert, explicit downloads preserve exact bytes, and independently counted
remote requests/effects remain unchanged. The harness retains raw CLI logs until
audit assertions, then redacts on Drop. Initial test-only compiler, file-mode,
strict synthetic OAuth, command-name and invented null-HTML expectations were
corrected against existing contracts; failure logs remain in `security_e2e/`.
Local authorization has no TTL; retirement is key rotation plus daemon restart.
Expired/revoked Google OAuth remains covered by the passing Google system suite.
Resource instrumentation/load tests and the full offline gate remain required;
this is not full Task14, live-provider or remote-CI evidence.

## Three-engine convergence — relevant gate verified

`python3 scripts/verify.py --suite multi_engine_system` completed0 (shell30577):
build-test-harness0, one test passed, zero failures/ignored. Each engine has an
independent profile, credentials and account UUID while sharing only the external
stateful Google service. External mail additions/deletion/labels and Calendar
updates/deletion converge after staggered restarts. Paginated projections agree
with each other and independent remote mail/event state. Local draft stays in its
own profile; send, copy and notification counts remain separately zero. Final
remote state remains unchanged after all engines stop. Implementation:
`crates/nuncio-test-support/tests/multi_engine_system.rs` and shared provider
ownership in `tests/support/system.rs`; runner registration in `scripts/verify.py`.

First attempt had a test-only protobuf Serialize compile error; subsequent attempts
used an incorrect coverage name. They were corrected to generated PartialEq and
the existing strict `current` coverage contract. Original logs are retained in
`test-results/multi_engine_system/first-compile-*`, `wrong-coverage-*` and
`wrong-mail-coverage-*`. No product fix was required. Six-command broader gate shell43253 completed all commands exit0:
format/bothClippy, multi_engine_system1, google_system21, operation_system6,
zero failed/ignored. Commands, logs, counts and changed-source hashes are retained
in `test-results/task14-multi-engine/`. No production/dependency changes; previous
production artifacts remain applicable. Security/resource suites remain required;
no full Task14 claim.

Restore checkpoint57dd648eb383e2aeb726b05aac07d97b131270d8 was signed and pushed:
commit0, verify-commit0, push0, exact ls-remote match0. Staged Gitleaks and diff
checks passed. This signature/remote verification is not remote-CI evidence.

## Restore retry capacity — full relevant gate verified

`python3 test-results/task13-restore-retry/run-gate.py`: shell7100 terminal exit0,
all7 commands passed: formatting, both workspace Clippy configurations, engine128,
actual daemon/CLI recoveryE2E5, IMAPE2E13, release-isolation2 with a fresh normal
production build. Zero failed/ignored. `summarize.py` exited0 and recorded
results.json, counts.json, source-hashes.json and artifacts.json in that directory.
Daemon SHA256 `0f30533cedd8e78acd1196962a5ee908ccdd538cee905f0b7505f2ca26586170`,
16352624 bytes; CLI SHA256
`23f75c75837bc64b25a40cdac3d5a3e0abcff436bbf8d75d161664ac3659d4b1`,3506544 bytes.

The actual CLI regression first failed against the preceding daemon at attempt65
(exit5/conflict instead of exit2/invalid key), shell66336/101, red-e2e.log. With the
fix it completes65 failures and a corrected-passphrase restore without restarting.
It compares original profile/keys/backup, independently observed whole mock
provider state (including sends/copies/notifications), and new-profile startup.
Test: `crates/nuncio-test-support/tests/support/restore_retry_e2e.rs`; engine tests:
`crates/nuncio-engine/tests/restore_retry.rs`. Production changes:
`engine/recovery.rs` and `maintenance.rs`. No schema/proto/dependency change.
This closes the bounded retry defect; Task14 suites and Task15 deliverables are next.
The following focused entry is historical, not a currently pending verification.

## Restore retries — focused regression verified, broader checks pending

`cargo test --locked -p nuncio-engine --test restore_retry` reproduced cleanup
capacity exhaustion on attempt65: shell93449, exit101, one failed test;
`test-results/task13-restore-retry/red.log`. Recovery now retires prior jobs inside
the blocking restore worker while its input owns maintenance/profile admission.
Foreign-profile input is rejected before recovery. No schema/proto/dependency change.

`cargo test --locked -p nuncio-engine --test restore_retry --test maintenance
--test restore_worker_ownership --test restore_upload_cleanup`: shell75313, exit0,
8 passed, zero failed/ignored; `test-results/task13-restore-retry/green.log`.
Includes65 wrong-passphrase attempts followed by a correct restore without restart,
original keys/files, foreign-profile isolation and existing cancellation/cleanup
tests. Daemon/CLI regression, full relevant gate and fresh production artifacts
remain pending; this does not supersede the previous artifact hashes.

## Initial signed checkpoint and remote backup

James explicitly authorized initial and subsequent checkpoint commits/pushes after relevant checks. Commit `2480bf94cdcff15fcf0da886691bd872e38edf31` (`feat(rebuild): checkpoint Google and MailPlus engine and CLI`) contains the rebuild and approved design/audit documents, preserving the original workspace. `git commit -F rebuild/test-results/checkpoint-initial/COMMIT-MESSAGE.txt` exited0; configured OpenPGP signature verified with `git verify-commit HEAD`. `git push --set-upstream origin feature/nuncio-google-first-rebuild` exited0; `git ls-remote origin refs/heads/feature/nuncio-google-first-rebuild` returned the exact same hash. No merge, release, installation or live-provider acceptance action occurred. Remote CI status was not inspected or claimed.

Gitleaks8.30.1 staged scan with default rules and full redaction exited0; staged whitespace check exited0. Initial scanner reports flagged dense test-count prose and Calendar-role prose in TODO, not credentials. Those lines were clarified without rule exclusions; original redacted reports remain under `test-results/checkpoint-initial/`. All389 staged file blobs matched working-tree bytes before commit; per-file SHA256 is recorded in staged-sha256.json. Scoped Git attributes preserve EML CRLF and treat PDF as binary; staged fixture hashes match tested files (EML `e17f0fb14ab4f67071576512415f4c8c760f9b5850e6516b5c9cdc58374dc8eb`, PDF `ffe177d7751be4ef1d7d8f7476736e62edb5f6f7ff325c75ac206997b1a0a372`). These preflight checks do not replace the remaining Task15 dependency/license/security review.

## Startup ownership and relative restore — full relevant gate verified

`python3 test-results/task13-startup-cleanup/run-gate.py` completed all7 commands exit0 (shell87318): fmt, both Clippy configurations, full engine126, actual recoveryE2E4, actual IMAPE2E13, and release-isolation2 with a fresh production build. Zero failed/ignored. `summarize.py` completed0 and recorded exact commands/results/counts/source hashes/artifact hashes. Daemon SHA256 `f67a6d53b62cd88d80f3e5b8f51d39fb6d36c0e14de562ba92f79563cf49acb1`; CLI `23f75c75837bc64b25a40cdac3d5a3e0abcff436bbf8d75d161664ac3659d4b1`. The following focused/paused entries are historical.

Startup cancellation retains the source-profile lock inside blocking cleanup. Relative profiles now reach encrypted backup inspection, recover an actual persisted cleanup row, and can activate a new independent profile. Ancestor aliases such as macOS `/var` work; final owner symlinks and another owner's upload remain rejected. Existing stage/key/activation/lease tests and actual daemon/CLI crash/effect assertions remain intact. No schema/proto/dependency change. Initial fixture/Clippy/type errors and the alias regression are retained as gate-first-*, gate-second-* and gate-alias-failure-*; alias-focused.log passed13 tests after correction. Full Task13–16 remains incomplete; no package, install, live-compatibility or remote-CI claim.

## Resumed startup cleanup — focused fixes, relevant gate next

Startup cleanup now retains a shared profile lock until the blocking worker finishes even if Engine::open is cancelled. Focused `cargo test --locked -p nuncio-engine --lib engine::recovery::journal_tests`: shell82528/0, 4 passed, none failed/ignored; `test-results/task13-startup-cleanup/green-lock.log`. Existing directory/key protection cases remain asserted.

The strengthened relative-path regression failed at InvalidPath before encrypted inspection (shell82643/101, relative-strong-red.log). Journal creation and startup now canonicalize the owner with directory-identity validation; symlink owners still reject. `cargo test --locked -p nuncio-engine --test profile_lifecycle`: shell75532/0, 5 passed, none failed/ignored; relative-green.log. The test proves a pending journal row exists, then startup clears it and preserves original keys/manifest. Additional successful relative restore and owner-symlink cases added afterward; full relevant gate and refreshed production artifacts still pending. No schema/proto/dependency changes.

## September 11 executive-status checkpoint — new regression remains failing

The goal tool reports paused at the user's status request. Last passing gate remains `task13-maintenance-ownership` below; it does not establish that the current expanded suite passes. No production changes have followed that gate, so its binaries contain the newly demonstrated defect.

`cargo test --locked -p nuncio-engine --lib cancelled_startup_retains_profile_ownership_until_cleanup_worker_finishes` reproduced a product failure after correcting a test-fixture mutex lifetime: shell98698, exit101, 0 passed/1 failed/19 filtered, 0.12s. Exact output: `test-results/task13-startup-cleanup/red-corrected-fixture.log`. The source reopened while a cancelled startup's blocking cleanup remained active. Implementation locations: `engine.rs::open_inner` and `engine/recovery/journal.rs::recover`. Test: `engine/recovery/journal_tests.rs`. No fix applied yet. Initial fixture failure is retained in `red.log` (shell90964/101).

The separate `single_component_relative_profile_recovers_failed_restore_without_changing_original_keys` test passed (shell94618/0, 1 passed/4 filtered; `relative-red.log`), but asserts only that restore errors. It may fail before journal admission and is not accepted as proof of pending-job recovery. Strengthen it before relying on its result. SESSION-STATE/TODO identify the exact next action. No current full-suite, full-goal, package, live-compatibility or remote-CI success claim.

## Maintenance profile leases — verified schema22 follow-up

`python3 test-results/task13-maintenance-ownership/run-gate.py` completed all7commands exit0 (shell25031): Rustfmt, bothClippy, full engine122, actual recoveryE2E4, actual IMAPE2E13 and production-isolation2 with a fresh normal release build. Zero failed/ignored. `summarize.py` verified statuses/counts and recorded source/artifact hashes. Daemon SHA256 `f0bcf60ac3db98dd0e3d4361e7a2b01420b6d2eabc10b5d5aad15d5aae13f8dc`; CLI `23f75c75837bc64b25a40cdac3d5a3e0abcff436bbf8d75d161664ac3659d4b1`. Source locations and exact commands/logs/red-green evidence are retained in that directory. The following focused-work section records chronology; its pending gate is now complete.

BackupArtifact, BackupUpload and BackupInput now retain the source profile lock alongside admission, through blocking work/output/cleanup and Engine shutdown. Three regressions prove source reopening is Locked until the last object drops, then original keys/identity survive. Existing cancelled restore and cleanup-failure recovery remain passing. No schema/proto/dependency changes; prior44-migration and Google21/26 full gate evidence remains for the preceding journal snapshot. Next startup-cancellation/relative-path tests and earlier artifact crash cleanup are recorded in SESSION-STATE. No full-goal, package/install/live compatibility or remoteCI claim.

## Maintenance profile leases — focused red/green, full gate running

New `maintenance_ownership.rs`: `cargo test --locked -p nuncio-engine --test maintenance_ownership` initially exited101,0passed/3failed (shell22551): a returned backup, incomplete upload or completed input allowed source Engine reopening after shutdown while the artifact still existed. MaintenanceLease now owns both the existing transfer permit and a shared source profile-lock File, retained through output/input and blocking work. No schema/proto/dependency change. Focused shell33689 passed9tests, exit0, retaining original keys/identity and releasing ownership when each artifact drops. Commands/red/green evidence: `test-results/task13-maintenance-ownership/`.

Seven-command relevant gate is running (shell25031); do not treat the preceding schema22 full gate as proof of these newer changes or call its production artifacts current until refreshed. See SESSION-STATE for exact continuation.

## Schema22 restore journal — verified full recovery gate

`python3 test-results/task13-restore-crash/run-gate.py` finished all12commands exit0 (shell28944). Rustfmt, bothClippy, engine119, proto/CLI/daemon20, migrationE2E1, recoveryE2E4, Google system21/E2E26, IMAP system26/E2E13 and release-isolation2; zero failed/ignored. Production built fresh. Exact command/status/log, counts, source hashes and artifact hashes are retained there; `summarize.py` verified results and produced metadata. Daemon SHA256 `93203902219a013b002478c4ed46723903764af08db9f9f2e67edd8205837f4c`; CLI `23f75c75837bc64b25a40cdac3d5a3e0abcff436bbf8d75d161664ac3659d4b1`. No package/install/remoteCI/live compatibility claim.

The new recoveryE2E test reaches all five actual daemon SIGKILL boundaries: owned empty stage before export, completed stage, first key, second key, activation. Before activation it removes only owned staging/upload and new keys; after activation it preserves target/new keys. Original profile/keys/draft metadata/backup and independent Google state remain unchanged through repeat restarts. The preexisting recovery case separately verifies real PDF/wire bytes and held request identity through restored engine/API/CLI and independently counted submission.

`migration-case-index.json` references44 distinct current-run evidence records: source schemas0–21, SIGKILL before/after each next commit, final22 and two verified restarts each. Whole catalog/user rows, original encrypted bytes, payloads/requests, profile/keys and independent Google snapshot checks pass. Frozen migrations1–21 are unchanged; history22 adds only migration22. Scope remaining: earlier maintenance artifact ownership/crash windows and the other Task13–16 items in SESSION-STATE/TODO.

## Explicit upload cleanup — focused regression and fix

`cargo test --locked -p nuncio-engine --test restore_upload_cleanup` initially exited101: an owned upload directory made temporarily unwritable caused TempDir Drop cleanup to fail, yet restore reported success. Restore now explicitly validates/removes that upload and only retires its encrypted record after cleanup and parent fsync. Failure after activation reports activation uncertainty while retaining new keys/target/record. Once permissions are repaired, source restart cleans the upload without changing original or restored keys. Focused shell87827 passed5 tests (restore3, cleanupfailure1, detachedworker1), exit0. Evidence: upload-cleanup-red.log and upload-cleanup-green.log.

The first full schema22 gate stopped on an obsolete constructed schema18 test fixture that retained the new22 table. Corrected only the fixture downgrade; frozen migrations1–21 remain byte-for-byte unchanged, history22 appends22. A fresh12-command gate is now running; see SESSION-STATE. No full current gate/artifact success claim yet.

## Restore journal — current schema22 work

Feature build0 and actual daemon/CLI four-boundary restore SIGKILL test1passed/0failed/3filtered, shell64141: `python3 test-results/task13-restore-crash/run-red.py`. Original before-journal regression and a later sandbox loopback denial are preserved separately. Keys are removed for preactivation crashes, retained after activation; source draft/attachment metadata/original keys/backup and independent Google snapshot survive two restarts. This is focused evidence, not full current verification.

Schema22 now holds source-owned restore cleanup records, recorded before stage payload export and key writes. Backup/restore sanitize these records. Additional safety tests first failed on keys deleted before unexpected stage contents were detected (2passed/1failed, shell86514). Both directories are now fully preflighted via retained handles before key deletion. Shell6722 passed all3 tests, including seven replacement/uncertainty scenarios, repeat recovery, key-store failure/retry, activated key retention and backup authority exclusion. Initial type-annotation and a subsequent syntax-edit compile failure are retained in focused logs. Full schema22 migration, lint, provider regression, release isolation and artifact gates pending. The previous sections remain evidence for earlier source snapshots.

## Restore crash cleanup — current failing regression

`python3 test-results/task13-restore-crash/run-red.py`: feature build exit0; targeted actual daemon/CLI test exit101,0passed/1failed/3filtered in1.67s. Failure: `abandoned stage after restore_after_stage`. Runtime and logs retained in that directory. This confirms missing process-death cleanup; later key-write/activation scenarios in the same test have not executed yet. This failure predates the schema22 implementation above. The prior complete gates below refer to their source snapshots; current full verification and fresh production binaries are pending.

An earlier run stopped on incorrect CLI guidance after the daemon died: generic request_outcome_unknown/exit4. The new focused CLI test reproduced this, then passed after restore-specific error handling included tonic Unknown. Current subprocess output now reports restore_outcome_uncertain/exit5 before reaching the cleanup failure. This preserves named-target inspection guidance rather than encouraging blind retry after possible activation. Unit red/green logs and initial subprocess failure are retained. New restore checkpoints are feature-only; production marker exclusion tests still need extending before release verification.

## Detached restore ownership — verified schema21 gate

`python3 test-results/task13-restore-worker/run-gate.py` completed with all7 commands exit0: Rustfmt, both Clippy configurations,17 lifecycle/maintenance/restore tests,3 Google recovery E2E,13 IMAP E2E and2 production-isolation tests with a fresh release build. Zero failed or ignored. Commands/logs/counts/hashes are in that directory. Current daemon SHA256 is `1fa76deee9552e6de1fefc6df198c56022a697b8b327c335fef3c7131470b99e`; CLI is `0018628d4f56a533203bc6ff906cc8b1bea6291ae07dba6c0d82329cdac9da40`. This is a focused gate, not a new full-engine total or remote CI run.

`restore_worker_ownership.rs` reproduced the source profile reopening after the awaiting caller was cancelled and the original engine shut down, while restore key creation remained blocked. `Engine` now holds its profile lock in an Arc; `restore_backup` retains a clone inside the blocking closure through restore completion and upload cleanup. The test verifies that a second engine receives Locked during the blocked work, then both original and restored profiles reopen successfully with distinct identities and unchanged original keys. It uses a deterministic private SecretStore barrier and RAII release, without adding a runtime test hook. The original red and focused green1/0/0 are retained. Durable process-death cleanup is the next separate change; no cleanup journal or schema22 exists yet.

## Restore path identity — verified schema21 gate

`python3 test-results/task13-restore-paths/run-gate.py` completed with all7 commands exit0: Rustfmt, both Clippy configurations,8 restore tests,3 Google recovery E2E,13 full IMAP E2E,2 release-isolation tests and a fresh production build. Zero failed or ignored. Exact commands/statuses/logs, counts and hashes are retained in that directory. DDL and dependency versions are unchanged; no migration rerun was required. The previous complete engine suite was112 tests at the Sent gate; this focused change added2 tests and ran the relevant8 restore tests, not a new full-engine total.

The new filesystem regression initially failed because replacing a stage's parent allowed the replacement stage to be activated. `store/restore/directory.rs` now retains parent/stage directory handles, checks device/inode identity before activation, renames relative to the retained parent with NOREPLACE, and treats changed post-rename visibility as activation uncertainty. Cleanup refuses a moved/replaced pathname and retains the moved encrypted stage. `restore_path_races.rs` covers6 activation/drop × replaced-parent/replaced-stage/symlink-parent cases. `restore_profile.rs` adds parent replacement during keystore writes and verifies that only new keys are rolled back while unrelated files/keys and the original backup survive. Normal existing-target and key-failure cleanup tests still pass. Crash-abandoned stage/key cleanup remains separate pending work.

Red output and the one result-pattern compile correction are retained. Production daemon SHA256 is `ab6361bb7d1ca551698c543688404fc44bea6c530e17aa92e11ac0f50192b117`; CLI SHA256 is `0018628d4f56a533203bc6ff906cc8b1bea6291ae07dba6c0d82329cdac9da40`. These are local binaries, not installed packages or evidence of remote CI/live compatibility.

## Positive client Sent reconciliation — verified schema21 gate

`python3 test-results/task13-sent-reconciliation/run-gate.py` completed with all13 commands exit0. Counts: engine112, proto/CLI/daemon19, Google system21/E2E26, operation6, reconciliation3, recoveryE2E3, IMAP system26/E2E13, release isolation2; zero failed or ignored. Both Clippy configurations and Rustfmt passed. Exact commands, logs, counts and fresh production hashes are in that directory. Schema remains21; the DDL/migration fixtures are unchanged. This is local evidence, not a package, installation, remote CI run or live-provider compatibility result.

Client Sent observation requires an explicit request, retained SMTP acknowledgement, current mailbox epoch, durable pre-DATA UID floor, a unique candidate, and exact frozen Sent MIME length/SHA256 including Bcc. `providers/imap/sent/client.rs`, `operations/smtp.rs`, and `store/smtp/apply.rs` implement this. `ObservedClientSent` commits progress, mail projection, existing `sent_copy` positive-read receipt and operation completion in one transaction. Automatic unknown APPEND remains held; absence or ambiguity never authorizes delivery or copying. The original `smtp_accepted` acknowledgement remains separate. `append_receipt` now rejects invalid formats before persisting them.

`tests/smtp_client_observation.rs` tests12 proof/rollback cases. `imap_write_system/recovery_sent.rs` tests7 independent-service scenarios: both restored accepted-SMTP policies, and ordinary lost-APPEND exact/absent/duplicate/missing-Bcc/changed-epoch cases. `support/smtp_sent_reconciliation_e2e.rs` tests2 actual daemon/CLI scenarios, killing after the positive read but before the local completion transaction, then restarting and replaying. It checks exactly one remote SMTP delivery/Sent message, clientAPPEND1/serverAPPEND0, original acknowledgement vs positive-read receipts, real PDF/raw export, private Bcc fidelity, unchanged original backup and unrelated account state. Focused results were storage1/0/0 in0.48s, system2/0/0 in16.07s and subprocess1/0/0 in7.74s. The complete IMAP runs passed26 in132.28s and13 in69.46s.

Failure evidence is preserved under the same directory. The first implementation wrote a new receipt discriminator absent from schema21's validator; this was replaced with the existing receipt format and atomic publication. Test fixture corrections cover independent server-added Bcc, address formatting, expected epoch error and recipient order. A missing test dependency and premature manual-request fixture were corrected without adding dependencies or weakening assertions. The sandbox Docker-socket failure was diagnosed and rerun with narrowly approved local access.

## Restored IMAP writes — verified test-only gate

`python3 test-results/task13-imap-reconciliation/run-gate.py` completed with all6commands exit0: Rustfmt, both workspace Clippy configurations, separate Google reconciliation3, complete strict IMAP system24 and actual daemon/CLI IMAP12. Zero failed/ignored. Exact commands/statuses/logs/counts are retained in that directory. Production source/schema21 is unchanged from the prior full mutation gate; its binary hashes/isolation evidence remain current. No package, installation, remoteCI or live-provider claim.

New `imap_write_system/recovery.rs` contributes2 system tests with28 scenarios:8 read/star on/off cases and20 COPY/nativeMOVE/fallbackMOVE/Trash/restore cases. Backups either predate dispatch or retain a committedCOPYUID; originals either stop there or finish aftersnapshot. Observation and replay cause no remote writes. Queued old snapshots stayheld even if the original later copied; recordedCOPYUID permits positive resolution or explicit safe source removal. Independent mailbox/counter/SMTP-ledger reads verify exact message bytes, account isolation, no duplicateCOPY, no APPEND/delivery/unscopedEXPUNGE and preservation of an unrelated UID markedDeleted. First focused2tests passed66.57s,22filtered; initial flag-only1passed31.81s.

New `support/imap_reconciliation_e2e.rs` runs2 actual subprocess scenarios: restored read-flag and copied fallbackarchive. CLI backup/restore/reconnect/Observe/ResumeSafe, SIGKILL after remote acknowledgement before localreceipt, restart/replay and rawEML download retain original operation/request/payload, exact bytes and global attempts1–4. Remote effects remain exactlyoneSTORE and, fortransfer, oneCOPY/oneUIDEXPUNGE; SMTP/APPEND/unscopedEXPUNGE remainzero. The existing unrelated Deleted message survives. Initial full actualIMAP suite12passed61.61s; final gate12passed62.76s adds a post-download effect assertion. Shared backup_system/imap_effects helpers remain test-only, and the Google reconciliation suite was rerun after extraction. Positive clientSent discovery and restored acceptedSMTP evidence are next; these results do not establish that additional path.


## Restored mail/calendar mutations and per-request retries — verified gate

Expanded separate reconciliation_system passes3 tests (first27.00s; gate rerun26.97s), covering12 Gmail and16 Calendar restored-write scenarios plus the existing send scenarios. Actual recovery_e2e passes3 tests (first10.34s; gate rerun10.46s), including3 new subprocess mutation crash cases. First green logs are retained under task13-reconciliation-mutations/*-first-green. The broader13-command gate completed with every command exit0: Rustfmt, bothClippy, engine111, proto/CLI/daemon19, Google21+26, operations6, reconciliation3, actualrecovery3, strictIMAP22+11 and releaseisolation2; no failed/ignored. Exact logs/counts/artifact hashes are in task13-reconciliation-mutations. Fresh schema21 production binaries include the retry fix. MigrationDDL is unchanged; the preceding42-case crash gate remains the migration evidence. New IMAP restoration tests and shared test backup helper were added afterward and remain separate work.

Gmail read/star/archive/trash/restore/existing-label removal and Calendar create/update/delete/respond are checked against stale queued backups, both before an original write and after it applied with a lost acknowledgement. Default Observe leaves independent state and remote-write counts unchanged. Safe resumes use desired-state/ETag semantics; missing restored creates remain held. Independent Calendar notifications are counted for none/all policies, and positive event state never fabricates notification acknowledgement. Replay after restart preserves original intent and performs no mutation. Actual CLI safe resumes survive SIGKILL after Gmail/Calendar acknowledgement but before local receipt, with exactly1 remote write and no repeated notification. Calendar abandonment retains uncertain outcome/error and attempts through another restart without provider writes.

A regression reproduced at the eighth explicit read-only request: the lifetime operation ordinal exhausted the provider retry limit and prevented later safe resume. Retry allowance now starts at the active request's persisted first_attempt_ordinal; SMTP/IMAP storage and receipts still use global ordinals. Ten distinct observations followed by safe resume succeed, with all12 global attempts and first ordinal11 asserted. ActualCLI crash history retains ordinals1–4 and request start2. The lifetime1000-attempt and1000-request admission limits remain enforced. Red test log is task13-reconciliation-mutations/retry-limit-red (2passed/1failed, exit101). An initial test compilation used unavailable uuid; fixed using deterministic synthetic IDs without adding deps. No assertions or mock validation weakened.


## Restored-operation reconciliation — schema21

`python3 test-results/task13-reconciliation/run-gate.py` completed with all14commands exit0. Rustfmt, both workspace Clippy modes, full engine111, proto/CLI/daemon19, Google system21/subprocess26, operation system6, separate reconciliation system1, actual recovery2, actual migration1, strict IMAP system22/subprocess11 and release isolation2 passed; zero failed/ignored. Exact commands/statuses, copied logs, counts and artifact hashes are retained in that directory. Migration E2E exercises42 before/after-commit SIGKILL cases across migrations1–21. Fresh production daemon/CLI are under target/production/release, schema21. Local executables and checks do not imply packaging, installation, remote CI or live-provider compatibility.

The versioned authenticated API/CLI now admits a durable, scoped reconciliation request against the original operation. Default Observe forbids mutations. ResumeSafe permits only provider-specific safe continuation; restored old queued sends/COPY and absent restored calendar creates stay held. Request replay returns current original state without another admission; changed fields conflict. A second restore retains old requests as inactive history. Request and attempt admission are bounded at1000. Actual Google lost-admission-ack plus SIGKILL before receipt recovers the original with exactly one independent send. CLI now reports unknown/cancelled transport outcomes as request_outcome_unknown/exit4 instead of falsely claiming failed admission. Production binaries exclude the admission/migration checkpoints and test controls.

The separate Google system test covers stale queued backups whose originals either never dispatched or later sent with a lost acknowledgement, authenticated/scoped/idempotent admission and positive-only evidence. Actual Google CLI tests cover negative holds and positive recovery; strict IMAP subprocess tests cover both Sent policies, stale backup after SMTP acceptance, Observe/ResumeSafe/restart/replay with unchanged independent SMTP DATA/delivery/APPEND counts. These are send paths; other restored write kinds are being added separately in task13-reconciliation-mutations and are not covered by this passing gate.

Retained failures: missing storage API/CLI command during implementation; an exhausted1000-attempt request admission; HTTP2 Unknown incorrectly classified as failure (temporary enum-only diagnostic removed); current-schema expectations still20 and a synthetic schema18 fixture retaining the new21table; sandbox EPERM on local sockets. Corrected expectations/fixture generation and requested authorized loopback access. No assertions or provider validation were weakened. Schema history through20 remains frozen; the new21 file appends only migration21. The newly expanded mutation tests were added after this complete gate; their results remain separate.


## Rich historical send history — schema20, test-only follow-up

`python3 test-results/task13-operation-history/run-gate.py` exits0; all4commands exit0: Rustfmt, bothworkspaceClippy modes, historical_migrations4 and historical_operation_history2. Six tests pass, zero failed/ignored. The earlier full106-engine gate preceded these two new tests; no full108-engine claim. Exact commands/statuses/logs and counts.json are retained. Production source/binaries remain those verified by the prior migration gate.

The new fixtures exercise all11 supported operation schema generations10–20 with22 scoped send records across Google/IMAP. They preserve finished/unfinished attempts, positive send and separate Sent-copy receipt rows, all operation states, abandon/resend/manual-confirmation decisions and replacement links, immutable per-operation MIME/Message-ID and SMTP progress/floors. Migrations compare all old user-table columns. Restores compare immutable data and exact permitted changes: ten pending operations are held, unfinished attempts close without fabricated evidence even when the new clock precedes their start, historical provenance remains, new provenance captures old state/version, and source ciphertext is unchanged. Public operation/history/payload reads retain identities and evidence; direct dispatch/reconcile attempts fail, reconnecting account state yields no ready work, and ordinary restart recovery finds no unfinished attempt to re-enable. This is storage-level evidence, not an independent provider effect test, live compatibility or every write-kind recovery.

Initial compilation101 used unsupported rusqlite Value.as_ref/u64 reads; pinned ValueRef::from and signed SQL reads corrected the test. First focused green2tests passed4.28s; final combined gate6tests also passed. No production assertions or mock validation were changed. Next work is explicit restored-original reconciliation with independent remote effects, then cleanup/races/resource/release work, as detailed in SESSION-STATE.

## Migration SIGKILL and fresh production isolation — schema20

`python3 test-results/task13-migration-crash/run-gate.py` exit0; all eight commands exit0. Rustfmt/bothClippy, full engine106, proto/CLI/daemon19, actualmigrationE2E1, actualGoogle recovery1, productionisolation2 pass with zero failed/ignored. One migration test exercises40 distinct checkpoints: before and after each commit from schema0→1 through19→20, with two normal daemon/CLI restarts per case. Frozen DDL/catalog/user rows and migration ledger show exactly the old or next complete generation; precommit death retains original ciphertext. CLI reads retain raw EML/PDF/drafts/queued scoped request IDs; attempts remain empty, profile/synthetic key bytes unchanged and the independent Google mock snapshot has no changes or requests. These migration cases do not independently inspect an IMAP server; prior strict IMAP/Synology suites remain separate evidence.

`migration-cases.json` retains40 case records, frozenDDL/ciphertext hashes and checked invariants. The gate stores exact commands/statuses/logs, counts.json and artifact hashes/sizes in artifacts.json. Fresh local production daemon and CLI build45.51s under target/production/release. Actual production binaries reject test flags/environment before profile access and exclude migration checkpoint markers present in the feature daemon. These are local executables, not a finished package or installation; no remote CI/live provider ran.

Initial intended test failed101 at missing migration-1-before-commit; first complete green passed1 test/40cases in32.40s. Retained intermediate errors: local File OpenOptions shadow compile101 and fixture CLI JSON/command mistakes. Corrected test calls use published storage/items/content JSON fields and mail draft show. No mock validation/assertions weakened. Gate reruns the complete40 cases after evidence collection and release isolation were added. Historical operation-history follow-up is now test-only work in progress (SESSION-STATE).

## Historical recovery and preservation — source schema 20

`python3 test-results/task13-historical/run-gate.py` finished exit 0; all eight commands exit 0. Rustfmt and both workspace Clippy modes pass. Full engine **105**, proto/CLI/daemon **19**, actual repair E2E **2**, actual Google recovery **1**, and actual IMAP E2E **11** (57.22 seconds) pass with zero failures or ignored tests. Exact commands, statuses, copied suite logs and counts.json are retained in that directory. Feature executables are current for this production source; normal production artifacts remain stale schema 12. No remote CI or live provider ran.

A later test-only follow-up adds failure injection at every migration ledger boundary. `failure-follow-up/results.json` records the separately completed historical suite (four tests passed, 7.92 seconds) and subsequent fmt/both Clippy checks, all exit 0. The four tests exercise 20 schema upgrades, 20 historical backup restores, 19 failed upgrades and missing-required-table negatives. One test was added after the full 105-test engine run; this is not a claimed full 106-test run.

Frozen SQL history in tests/fixtures/migrations is independent of the current migration runner at test execution; it was captured from rebuild sources once, not independently sourced from a provider. Old encrypted stores retain all pre-existing user-table columns, mail/FTS/calendar identity, drafts, queued requests, configuration and MIME/PDF blobs. Historical backup restore preserves source ciphertext, clears credential references/cleanup, holds pending sends with provenance and preserves immutable fields. The frozen multipart fixture is 1,424 bytes and carries the real 612-byte PDF, checked by Python and Rust decoders.

Historical backup restore first failed with Database(1), retained in nuncio-historical-backups-first.log. Inspection unconditionally queried drafts/operations before their schema introduction. Counts now use version 7/10 boundaries; absent required tables in newer schemas still fail. The regression then passed all 20 versions. Actual SQLite abort triggers reject each next migration ledger insert after its DDL: original encrypted bytes, full catalog and data rows remain unchanged, and retry succeeds after the test removes its trigger. This proves transaction rollback, not migration process-death recovery.

`repair_preservation.rs` checks relational corruption refusal with unchanged database/WAL bytes, revision and draft. Actual `repair_e2e.rs` checks failed startup and repair for corrupt-header/truncated profiles: no data/key/profile replacement, no provider request, correct failure statuses and successful reopening after the test restores its retained original. Initial storage test compilation used a wrong getter name and unsupported unsigned SQLite type; the fixture was corrected. No validation/assertions were relaxed.

Remaining: deterministic migration SIGKILL, richer old operation histories, restored-operation reconciliation, cleanup/races, adversarial/resource/egress/release verification and live acceptance. See SESSION-STATE for the exact next action.


## Full schema20 projection repair gate

`python3 test-results/task13-repair/run-gate.py` exit0, all13commands0: Rustfmt, both workspaceClippy modes, engine101, proto/CLI/daemon19, repair_system2, actualrepair_e2e1, Google system21/actualE2E26, operation system6, IMAPsystem22/actualE2E11 (56.50s), actualrecovery_e2e1. Zero failed/ignored. Exact commands/statuses/logs, nested suite copies and counts.json are in test-results/task13-repair/. Feature daemon/CLI schema20 verified; production artifacts remain stale schema12. RemoteCI/live providers did not run.

Repair tests cover exact calendar checkpoint identity sets, unpublished new calendars, full account atomic publication/rollback under real SQLite constraint failure, cancellation and SIGKILL/restart, preserved drafts/queued sends, preview with no provider requests or revision changes, incompatible ordinary-sync admission and compatible repair joining. Independent Google snapshots and Dovecot/Mailpit state/counters verify no sends, copies, flag changes or notifications caused by repair. Both IMAP TLS modes passed. Full mail promotion repairs orphan/duplicate FTS rows without crossing accounts. Shared calendar catalog publication invalidates coverage/cursors on permission changes and retirement while retaining cached data.

Failures retained: FTS actual2/expected1 and calendar coverage current/expectedunavailable reproduced production defects before fixes; corresponding red/green logs copied. IMAP system firstcompile used unavailable uuid helper (fixed to synthetic constant); firstbehavioralrun21pass1fail compared global progress revision as immutable. It now checks monotonic revision and exact items/coverage/paging, while preview still requires unchanged revision. New repair API admission test initially used an incorrect method name; aligned with actual RefreshAgenda and FailedPrecondition contract. No mock validation or production assertions weakened.

Official [CalendarList roles](https://developers.google.com/workspace/calendar/api/v3/reference/calendarList) also document writerWithoutPrivateAccess. The mock role control rejects it and write authorization permits only owner/writer; its read/write semantics remain unverified, and it remains an explicit Task14 compatibility item. Full R01–R16 completion, all historical migrations, restored-operation reconciliation and release verification remain outstanding.

## Projection repair in progress — schema20

Focused storage calendarrepair2 + ordinaryprojection1 + restore3 passed `/tmp/nuncio-calendar-repair-rollback.log`; repair_system1 passed `/tmp/nuncio-repair-system-first.log`; CLI argument test1 passed `/tmp/nuncio-repair-cli-first.log`. `test-results/repair_e2e/results.json` build0/suite0, actual Google daemon/CLI1 passed,0failed/ignored,2.12s. Independent remote mail/calendar snapshots and zero accepted sends confirm repair makes no provider writes even across SIGKILL/restart with queued mail. Preview generates no provider requests/revision changes. No full schema20 gate yet; production artifacts remain schema12.

Orphan FTS regression failed exit101 (actual2 versus expected1 search rows), `/tmp/nuncio-repair-search-red.log`. Full promotion now rebuilds all scoped search rows inside the transaction. Two mail_projection tests passed exit0, `/tmp/nuncio-repair-search-green.log`, including rollback under actual SQL constraint failure and other-account preservation. New implementation and exact next steps in SESSION-STATE.

## Latest recovery bounds and export protection gate

`python3 test-results/task13-recovery-bounds/run-gate.py` exit0, all7commands0: Rustfmt, both workspaceClippy configurations, fullengine97, proto/CLI/daemon18, actual recovery_e2e1, actual imap_e2e10 (57.18s). All zero failed/ignored. Exact commands/statuses/logs and copied nested evidence/counts.json are retained in that directory. Source schema19, feature binaries current; production schema12 artifacts still stale. No remoteCI/live-provider/package claim.

CLI mail raw/attachment/body now use shared output_file.rs protected-path validation before download. New actualGoogleE2E regression failed101,25pass1fail: absent active store.db-journal was created and command returned0. Authenticated status + shared canonical path refusal fixes it; repeated actualGoogleE2E26passed0failed/ignored (26.57s), build0/suite0. Evidence in task13-export-protection/{behavioral-red,green}. Existing-file/identity/atomic completion checks preserved.

Store backup_io.rs adds finite input/copy/hash bounds and checked available-space estimates, with actual SQLCipher export page limits for backup/rekey. Tests cover bounded reads from an endless/growing source, shortinput, zero/oversize rejection before reads, propagated StorageFull, estimate overflow/exhaustion, sparse1TiB+1 input with no copied files/original changes, and real SQLCipher export failure at2pages while preserving source payload and respecting outputsize. Preflight is not a reservation; these do not prove process-crash cleanup, all disk/migration errors or every restorekind.

Focused failures retained: missing API compile101; wrong pinned rusqlite type/signature compile101; SQLCipher page_size Text decode caused backup0pass2fail101; isolated probe reproduced InvalidColumnType cipher_page_size. Bundled source112162–112177 confirms string return. A real capped export returned generic SQL error1 with exact database-or-disk-full text, as sqlcipher_export wraps it using sqlite3_result_error at113304–113308. Test now asserts the exact wrapper plus source/output invariants. Subsequent full97engine passes include all new tests. Temporary probe removed, logs retained. No assertions or mock validation weakened.

Manual worksheet prepared at docs/MANUAL-ACCEPTANCE.md with named approval fields, command/action requirements, independent observations and cleanup record. Every live case remains unapproved/unverified per current mock-only instruction. Full goal remains incomplete; next Task13 projection repair/dry-run and broader migration/reconciliation work are in SESSION-STATE.


## Maintenance API/CLI gate and Synology stale-snapshot recovery

`python3 test-results/task13-maintenance/run-gate.py` finished exit0; all12 commands0. Exact commands, statuses, logs, nested suite copies and counts are retained in `test-results/task13-maintenance/`. Rustfmt and normal/feature workspace Clippy passed. Engine93, proto/CLI/daemon18, Google system21, actual Google E2E26, operation system6, IMAP system21, actual IMAP E2E9, actual recovery E2E1; zero failed/ignored. This verifies source schema19 and feature executables, not current production packages or remote CI.

New Maintenance engine/API/CLI locations: engine `maintenance.rs` and `engine/recovery.rs`; `proto/nuncio/v2/maintenance.proto`; daemon `maintenance.rs`; CLI `backup.rs`, `backup/{secret,upload}.rs`. Owned semaphore permits remain live through blocking reads/writes/inspection/restore after caller cancellation. Authenticated streams enforce ordered length/hash/framing, private staging, one active transfer, 256KiB chunks/1TiB declared bound, 30s upload idle limit. Secrets are redacted/zeroized and accepted via protected stdin JSON. Existing destinations and canonical aliases to protected files, including absent journals, are refused for backup output. Profile status now reports absolute canonical protected paths for relative data directories.

Failures retained without weakening assertions: first actual recovery test tried deleting consumed `.arm` marker (fixture fixed to `.entered`); second exposed direct wall-clock restore instead of the injected engine clock (production clock plumbing fixed). Third actual recovery run passed1. Expanded API malformed streams (empty/header/order/size/length/hash/wrong key) reached the positive restore path, then exact macOS path expectations failed `/var` vs canonical `/private/var`. First broad gate likewise stopped at engine maintenance3pass1fail. Exact expected parent paths now canonicalize; focused engine4/API1 pass, followed by the complete12-command gate above. First gate artifacts remain in `canonical-path-failure/`; early recovery failures in `e2e-first-fixture-failure/` and `e2e-clock-failure/`.

Newer test-only follow-up: `python3 scripts/verify.py --suite imap_e2e` build0/suite0,10passed0failed0ignored,68.78s; copied logs/results in `test-results/task13-smtp-recovery/`. `support/smtp_recovery_e2e.rs` runs both Sent policies through actual daemon/CLI: snapshot while queued, original sends afterward with lost SMTP acknowledgement, restore old queued snapshot, preserve configuration/draft/PDF/request identity, reconnect and crash/restart without repeat delivery/APPEND or fabricated receipts. Explicit duplicate-risk resend alone produces delivery2 with a distinct Message-ID and identical real PDF. Independent Mailpit envelope/wire hashes and Dovecot Sent content/counts are checked before and after restore/restarts. This does not establish a general original-operation reconciliation API or live MailPlus compatibility.

Latest follow-up in progress: mail raw/attachment/body exports lack the backup command's future protected-path check. A new actual Google CLI regression targets absent `store.db-journal` in an isolated synthetic profile. Handle/log and next action are in SESSION-STATE. No current aggregate success claim for subsequent edits.


## Task 13 restore profile lifecycle — schema 19

`test-results/task13-restore-profile/results.json`: all four commands exit 0: Rustfmt, normal workspace Clippy, feature workspace Clippy, full engine tests. Actual aggregate: **88 passed, 0 failed, 0 ignored**, recorded in counts.json. Gate ran with narrowly authorized loopback access for existing synthetic protocol tests. Independent PDF checks `pdfinfo` and `pdftotext` both exit 0; exact commands/output retained in pdf-checks.json and logs. No remote CI or live provider ran.

`Engine::restore_profile`, `profile/restore.rs`, and `tests/restore_profile.rs` add fresh profile/database/API identities, initialized private metadata, activation without starting provider workers, and rollback of newly owned keystore entries. Three profile tests cover successful reopen with distinct keys and unchanged usable original profile; partially persisted key writes returning errors; explicit reporting when key deletion fails; and a concurrent target created after key writes. The target and original secrets remain unchanged, new keys roll back, and retry against the existing target writes no additional keys. Focused profile/lifecycle tests passed five tests, then expanded profile checks passed three. Initial missing API/error variant compilation failed with exit 101; retained red log. Full Maintenance API/CLI and actual recovery E2E remain outstanding.


## Task13 restore storage foundation — schema19

`test-results/task13-restore-core/results.json`: all4commands0 (Rustfmt, normal/featureworkspaceClippy, fullengine85passed0failed0ignored); counts.json and logs retained. Initial engine run under sandbox failed10 existing local-socket tests with EPERM (3passed10failed101); narrow authorized loopback rerun `python3 test-results/task13-restore-core/resume-engine.py` exit0. This is local evidence, not remote CI or live-provider compatibility.

New store/restore.rs, restore_schema.sql and tests/restore.rs stage/rekey/migrate/sanitize a backup and atomically activate with rustix no-replace rename. Focused backup2/restore3 pass, PDF-expanded restore3 pass. Exact durable table rows, request IDs/fingerprints, real PDF/frozen MIME, existing positive receipts, pending holds after account reconnection/startup recovery, source/target preservation, wrong keys, corrupted/truncated/future backup, schema18 migration success/failure, sidecars/permissions/drop cleanup and existing empty-directory/file/symlink targets are checked. PDF fixture independently validated by Poppler pdfinfo/pdftotext. Profile key initialization, Maintenance API/CLI and actual recovery E2E remain required.

Initial missing-API compile101, TempDir::keep must-use compile101, then behavioral0pass2fail101 SQLCipher code1. Diagnostic isolated readonly main connection propagating to attached output ('attempt to write a readonly database'). Only the owned staging copy now opens READ_WRITE; caller's source is untouched. Diagnostic source removed, log retained. Subsequent focused runs exit0. No assertions weakened. Rustix1.1.4 was already locked transitively; added direct fs dependency using offline Cargo resolution. Schema assertions explicitly advanced18→19 in backup/calendar_schema/google_system; actual Google suite on schema19 still pending until the next cross-API gate.


## Task13 backup core in progress

New engine store/backup.rs (create_backup/inspect_backup) and tests/backup.rs. SQLCipher export with separate recovery passphrase, explicit metadata/user_version/application_id, read-only key validation and cipher/SQLite/FK integrity checks, ciphertext streaming hash and RAII private temp directory. Existing tempfile dependency promoted from optional to normal; test-harness feature retained. Restore/engine/API/CLI/stream limits and adversarial files remain unimplemented. The focused storage gate below is now verified; broader recovery behavior remains unimplemented.

Initial test compile101 absent API and fixture subject Option instead of String; corrected fixture. First implementation compile101 rusqlite does not implement FromSql for u64; changed signed SQLite reads to checked output conversion. Retained first logs in test-results/task13-backup-core. Second compile101 retained: one unspaced revision:u64 declaration escaped the replacement, then corrected. Focused shell95864 /tmp/nuncio-backup-core-3.log exit0,2passed. Added uppercase raw-key regression /tmp/nuncio-backup-kdf-red.log exit101,0passed1failed; validate_passphrase now rejects lowercase and uppercase SQLCipher raw forms for64/96hex lengths. Expanded fixture adds320KB binary draft attachment, frozen payload and uncertain operation attempt; /tmp/nuncio-backup-core-4.log exit0,2passed0failed/ignored,0.62s. Standalone normal workspaceClippy exit0 at /tmp/nuncio-backup-clippy.log. Formatting-only cleanup followed; four-command gate test-results/task13-backup-core/results.json finished exit0: fmt/bothClippy/fullengine82passed0failed0ignored, all4commands0. counts.json retained. Backup storage only; restore/API/CLI/recovery E2E still outstanding. A final pinned-code review found SQLCipher also accepts160hex (encryption key + HMAC key + salt), beyond64/96; new focused regression failed101,0passed1failed (nuncio-backup-kdf-160-red.log). Validation now includes160 for both x/X; repeated four-command gate passed all4commands0/fullengine82passed0failed0ignored. Previous green preserved under before-160hex. Bundled source: libsqlite3-sys0.38.2/sqlcipher/sqlite3.c:111191–111218.


## SMTP capability/configuration gate complete

`python3 test-results/task12-smtp-profiles/run-gate.py` exit0; all5commands0: Rustfmt, both Clippy modes, IMAPsystem21 (68.29s), actualIMAPE2E9 (52.66s), allzero failed/ignored. Source changes in this follow-up are tests only; the preceding confirmation gate supplies current engine80/protoCLI8 and Googleoperations6/system21/E2E26 evidence. Counts and exact commands/logs are retained in task12-smtp-profiles.

Missing UIDPLUS: client Sent fails before DATA/APPEND and remains failed on replay/restart; server Sent works with both TLS modes and through actualCLI with independent exact raw-byte/receipt checks. Changed SMTP endpoint: queued immutable intent transmits nothing to either server while mismatched, then exactly one message on the original transport after explicit config restoration. First gate failed an assertion that treated newest-first attempt index0 as ordinal1; fixed selection by ordinal and added exact identity_mismatch/no-receipt/applied-count assertions. First gate log/results retained in first-gate-failure. No production code/validation changes to manufacture passing profiles. Task13 backup/recovery is next; Task14 cross-cutting risks remain.


## SMTP capability/configuration profiles in progress

New System tests: client Sent without UIDPLUS must reject before DATA/APPEND and retain failed state after restart; changed SMTP endpoint cannot receive queued content, restoring captured endpoint permits one submission. Server-positive fixtures now include hidden UIDPLUS with both TLS modes. New actualCLI profile tests both Sent policies without UIDPLUS and independently checks raw bytes/receipts/remote counts after restart. No production changes in this follow-up.

First focused command cargo test --locked -p nuncio-test-support --test imap_system --target-dir target/test-harness smtp_profiles -- --nocapture exit101,1passed1failed0ignored/19filtered,92.38s. Endpoint fixture paused at operation_after_attempt while holding account lane; config RPC correctly waited and timed out. Use operation_before_dispatch before coordinator acquisition; add explicit10s config-RPC bound. Original failure log retained task12-smtp-profiles/system-first-fixture-failure.log. Gate pending. Mock Drop still blocks during failure cleanup (tracked Task14); no production concurrency controls weakened.


## SMTP manual confirmation gate complete — schema18

`python3 test-results/task12-smtp-confirmation/run-gate.py` exit0, all9commands0: Rustfmt, both workspaceClippy modes, engine80, protoCLI8, IMAPsystem19, Googleoperations6, Googlesystem21, actualGoogleE2E26. Separate `python3 scripts/verify.py --suite imap_e2e` build0/suite0,8passed0failed0ignored; copied logs/results under task12-smtp-confirmation/imap-e2e-suite. Allfailed/ignoredcounts0, counts.json recorded. Current feature binaries/schema18; production still schema12/stale. No live-provider/remote-CI/package claim. Next action is in SESSION-STATE; subsequent entries retain historical work/failures.


## SMTP manual confirmation work in progress

Additive proto OperationConfirmation.smtp_acceptance_note field6 maps through daemon to storage. IMAP confirmation checks immutable account/mailbox/UIDVALIDITY/UID floor, exact frozen Message-ID, non-Prepared SMTP phase and explicit separate acceptance note. It records manual_confirmed without inventing receipts or changing SMTP progress. Existing Google canonical evidence remains unchanged when field absent.

Focused behavioral red: prior shell49006 exit101 InvalidInput at valid SMTP confirmation under Google-only restriction. Minimal implementation focused shell73851 exit0,4passed. Expanded negative/active-attempt/version/reopen assertions rerun after lost shell98326 output: cargo test --locked -p nuncio-engine --test smtp_confirmation --test operation_resolution -- --nocapture exit0,4passed,0failed/ignored; /tmp/nuncio-smtp-confirmation-focused.log. System/API focused test exit0,1passed with four independent policy/decision scenarios (12.02s),18filtered; /tmp/nuncio-smtp-confirmation-system-run.log. First command exit101 used an unsupported package-scoped feature flag; corrected to existing dev-dependency features. First actual full IMAPE2E build0/suite101,7passed1failed0ignored: new conflicting-decision assertion expected4 but existing documented CLI maps FailedPrecondition to5/conflict. Corrected test to assert both5 and structured conflict, without production changes. Failure retained under test-results/task12-smtp-confirmation/imap-e2e-first-failure. Full-current gate pending.

## Latest verified SMTP resend checkpoint — schema18

`python3 test-results/task12-smtp-resend/run-gate.py`: exit0, all7commands0. Rustfmt, normal/featureworkspaceClippy, engine79, IMAPsystem18, operation_system6, actualGoogleE2E26. Separate `python3 scripts/verify.py --suite imap_e2e`: build0/suite0,7passed0failed0ignored,38.98s; its logs/results were copied to task12-smtp-resend/imap-e2e-suite. Counts.json covers these distinct commands; zero failed/ignored. Current feature artifacts schema18, production Task11/schema12 stale. All12 preceding server-Sent gate checks also0; see task12-smtp-server/results.json. No process running, liveprovider/remoteCI/package/install claim.

Changes: original SMTP resend created a queued replacement without SMTPintent; new storage red101 NotFound demonstrated it. Capture now occurs in the same transaction as newoperation/blobs and originalresolution, with current configuration, newMessage-ID fingerprint and no reused acceptance. Fullengine79 includes malformed-Sent rollback, deleted draft/reopen/privateBcc/recipient/phase/idempotency assertions. New actualCLI test covers bothSentpolicies, lostoriginalack, duplicate-riskflag refusal, deleted draft/binarysource, exact newcontent/body/binary/recipients, distinctMessage-ID, independent twoSMTPdeliveries only after explicit newdecision, correctSentcopies and secondrestart/decisionreplay. New separate API test covers foreignaccount, false-risk, concurrent identicaldecisions, restart and independentcounts/receipts. Initial fixture compile errors (nonDebugvalues/private read_blob and temporaryRPCclients) corrected without changing productionvisibility/Debugderives or suppressing lints.

Full goal remains incomplete. SMTP positive manual confirmation is stillGoogle-only; Task12 remainingprofiles and Tasks13–16 continue. See SESSION-STATE for exact next action. Older sections below preserve history and superseded next-action prose, not current instructions.


## Server-Sent gate passed; SMTP resend now in progress

`python3 test-results/task12-smtp-server/run-gate.py`: exit0; all12commands0. Pythonlint/format,Rustfmt,normal/featureClippy,engine78,protoCLI4,independentIMAPcontract2(wrapping18Python),IMAPsystem17,actualIMAPE2E6,Google21/26. Zero failed/ignored. Exact command/status logs, copied nested logs/results and counts.json retained. Current production artifacts stillschema12; featurebinaries schema18. No remoteCI or live-provider claim.

Resend follow-up is newer: store smtp_resend.rs initiallycompile101 from test-onlyDebug/privateAPI errors, corrected; then behavioralred101 NotFound for replacementSMTPintent. insert_resend captures SMTPintent/fingerprint within originalresolution/newoperation/blob transaction. Focused cargo test --locked -p nuncio-engine --test smtp_resend --test operation_resolution -- --nocapture exit0,4passed0failed0ignored; later added malformedSent rollback assertion stillneeds rerun. Actual IMAPE2E new resend test running shell2928, bothSentpolicies/draftdeletion/duplicate-risk/restarts/independentcounts andbinary/frozenbody. Separate API system resend plus focused gate next.


## Schema18 extended system profiles and independent AutoSent fix (gate running)

`python3 test-results/task12-smtp-server/run-gate.py` running shell29487: Python lint/format and Rustfmt0; remaining checks pending. First gate failed Python import order1; corrected without suppression, original results-first-lint-failure.json and python-lint-first-failure.log retained. Gate includes both Clippy modes, protoCLI, engine, independentIMAP contracts, system/E2E for bothproviders. No aggregate success claim until inspected.

Expanded server Sent system regression first101: observed immediate dispatch uncertainty before the reconciliation retry timestamp existed. Wait now requires the specified settled/scheduled reconciliation state and retains exact state/timestamp assertions. Next101 after29.72s: Dovecot auto=subscribe recreated Sent afterrename, yielding epochchange rather thanmissingfolder. Now6profiles: missingcopy, alteredbody, duplicate, explicitUIDVALIDITY, recreatedSent with independent empty/replacementepoch checks, and a true missing customconfiguredfolder. Existing positive sends cover both TLS transports; gate pending.

Two further system runs exposed an independentAutoSent bug (105.02s including failurecleanup, then7.35s with explicit asynchronous cleanup): ambiguous owner determination at auto_sent.py:116. Old observer used tags, which Mailpit adds after its message transaction and message headers can influence. Pinned source https://github.com/axllent/mailpit/blob/v1.31.1/internal/storage/messages.go confirms committed Username metadata and later tags. Switched observer to validated authenticatedUsername, no inferred ownership from tags. Original failure diagnostic retained test-results/imap-contract/runs/mock-mailplus-CQUrkL/services/auto-sent-error.json; earlier genericfailure mock-mailplus-yWqPfx. No product validation relaxed.

`python3 tests/imap/test_auto_sent.py`: diagnostic requirement red1(FileNotFound) then0,1passed12.804s; misleading alpha+beta X-Tags added gives red1 at ownerselection, then Username fix gives0,1passed12.624s. Existing independent rawbytes/accountcopy/count/assertions and deliberate pausedDovecot unknownAPPEND no-repeat assertions retained. Diagnostic records bounded type/sourcepositions only, no exceptiontext/locals/content; canary assertion retained. Full independent suite now required by gate.


## Schema18 current checkpoint (supersedes older next-action prose)

Strict SEARCH transcript fixture omitted required INBOX. Initial run hung (exact owned PID94529 stoppedSIGTERM, Cargo101); drop-before-join exposed earlyEOF101. Correct LIST fixture plus5s peer timeout: cargo test --locked -p nuncio-engine --lib providers::imap::sent::server -- --nocapture exit0,1passed/12filtered,10cases,0ignored,0.01s. No production validation/assertion relaxed.

Focused engine fingerprint/client/server storage4passed0. Actual python3 scripts/verify.py --suite imap_e2e build0/suite0,6passed0failed0ignored,37.16s, test-results/imap_e2e/. Includes serverSent crash afterSMTPreceipt/afterserverproof/beforepublication, duplicatecopy, lostDATAack, secondrestarts/requestreplay and independent remote SMTP/Sent byte/count/envelope evidence. Feature binaries schema18; production stillschema12.

New negative serverSent system test running shell7178 (missing/altered/duplicate/epoch/folder); positive send fixture expanded to implicitTLS+STARTTLS, not yet run. BothClippy/fullschema18gates pending after large-enum boxing fix. Historical results below are dated and cannot establish current aggregate success.


## Server-Sent schema18 work in progress (newer than all gates below)

Previous goal turn was verified progress, not a wait/blocker. Current turn implements server-managed Sent. `SentFingerprint` added under domain/submission/sent_fingerprint.rs: version1 SHA256 of selected parsed identity headers and exact raw body; optional Bcc omission allowed but unexpected/different Bcc rejected. Duplicate selected identity headers rejected, trace headers ignored, 1MiB header/512-header/64MiB MIME bounds. mail-parser0.11.9 serde feature enabled in Cargo.toml and Cargo.lock via offline Cargo. New unit tests initially compile101 missing type, then2passed0; empty Subject test failed101 and was fixed (empty optional header is valid).

Schema18 migration smtp_floor_schema.sql adds nullable sent_floor; no guessed backfill. start_smtp_data now takes a validated observed ImapMailboxState and commits current UIDNEXT before content. Server path refreshes that observation after SMTP envelope/DATA354 negotiation. Intent optionally stores versioned fingerprint (only server policy; old intents deserialize absent). Client APPEND mapping additionally respects a present fresh floor. Typed record_server_sent requires known accepted SMTP phase, exact account/mailbox/epoch, UID>=fresh floor and matching content fingerprint; durable server_sent_observed receipt precedes atomic Sent publication. Generic final Sent application independently checks raw content again. Existing schema17 client records remain compatible; old server rows lacking fingerprint/floor remain uncertain.

`providers/imap/sent/server.rs`: bounded UID SEARCH by 4096 message-sequence windows (max1million messages) intersected with captured UID floor and selected UIDNEXT, exact candidate metadata/body fetch; >1 matching Message-ID placement ambiguous, zero/content mismatch unconfirmed; recheck candidate set after body read. Worker calls it only after known SMTP acceptance, never to infer unknown delivery. New checkpoint operation_after_server_sent. First real `server_managed_sent` system rerun exit0,1passed/15filtered,3.72s; independently one server copy, SMTPacceptance1 and clientAPPEND0. This predates subsequent fresh-floor refresh/capture-only-server refinements, so rerun targeted system later. Feature workspace Clippy failed101 large enum after fingerprint growth; boxed PreparedWrite SMTP intent rather than suppressing lint.

**Immediate: poll shell22302** for `cargo test --locked -p nuncio-engine --test smtp_server_operation --test smtp_operation --test sent_fingerprint -- --nocapture`. New server storage regression exercises pre-accept prohibition, pre-DATA floor excluding older same-content UID, server-policy no APPEND, bad content rollback, proof/floor persistence through reopen and separate receipts. Then implement actual server-Sent subprocess success/crash/duplicate/lost-ACK cases and separate negative system scenarios; current new search still needs strict bounded protocol tests. No full schema18 gate yet. Existing focused client fixture remains unchanged except shared configuration helper; seven client SMTP crash cases passed at schema17.

Current follow-up: APPEND rejection gate `test-results/task12-smtp-rejection/results.json` all6checks0: fmt/bothClippy, engine13 (lib+SMTP storage), IMAPsystem15, actualIMAPE2E5, zero failed/ignored. Seven SMTP process-death boundaries include death after rejected-copy receipt. Copied nested logs/results/counts retained.

New server-Sent regression newer than that gate: failed101 after14.39s, one test executed, at uncertain/smtp_server_sent_unconfirmed (nuncio-system-8nWGXf). Before waiting for local completion, it independently verified server AutoSent completed1, remote Sentmessage1, SMTPaccepted1 and clientAPPENDrequests0. No production server-Sent implementation yet. An earlier edit script failed without saving and its filtered test command ran0; explicitly not evidence. Existing client scenario was factored into the same helper with every original byte/count assertion retained; focused smtp_submission rerun exit0,1pass/15filtered,0ignored,2.42s. No process running.

## SMTP client-Sent schema17 checkpoint and APPEND rejection follow-up

Nine-check gate `test-results/task12-smtp-client/results.json` latest statuses all0: Rustfmt, normal/feature workspaceClippy, proto-cli4, engine74, IMAPsystem15, actualIMAPE2E5, Googlesystem21, actualGoogleE2E26; all zero failed/ignored. Counts and copied nested logs/results retained. First attempt failed only stale Google system schema16 assertion (actual17,20pass1fail); preserved results-at-first-failure and both failure logs; resumed affected/remaining Google suites with resume-gate.py. This gate predates the subsequent fourth APPEND-rejection system profile/seventh subprocess crash scenario.

Earlier focused SMTP failures: integration compile101 from unconsumed methods until worker integration; localhost bind sandbox101 then scoped rerun engine IMAPunits11pass0; Clippy101 item-after-tests order corrected then0; actualE2E first4pass1fail due valid angle-bracket Bcc header, corrected to independent parsed exact address/count while retaining absent-wire-Bcc and hash assertions; lost-ACK system101 revealed plain text lacking finalCRLF, isolated frozen_message red101 then fixed at freeze (wire/Sent consistency), two frozen tests and smtp storage pass0; SMTPsystem2pass0; actualIMAPE2E5pass0 with six process-death boundaries.

APPEND rejection follow-up: new before-APPEND NO scenario red101 after22.97s at imap/before uncertain/imap_sent_copy_identity_unknown, synthetic profile nuncio-system-1uUqzD. Strict matching-tag NO with no positive mapping now yields explicit rejected evidence; store records imap_append_rejected and resets only copy phase, leaving SMTPaccepted. Parser adds conflicting positive-map+NO case, test1pass0/11filtered; storage1pass0 includes no reset before marker/after proof and forbids SMTP repeat. Edit script assertion caused partial changes and unused-field compile101; completed missing edits, no suppressions. Focused six-command gate running shell65378 at test-results/task12-smtp-rejection/results.json. Current aggregate status remains pending until all results inspected.

No production package/live-account/remote-CI claim; production executable artifacts still Task11/schema12. Feature binaries rebuilt at schema17. Relevant RFCs: https://www.rfc-editor.org/rfc/rfc5321.html (DATA replies/transparency), https://www.rfc-editor.org/rfc/rfc6531.html (SMTPUTF8), https://www.rfc-editor.org/rfc/rfc4315.html (APPENDUID), https://www.rfc-editor.org/rfc/rfc3501.html (failed APPEND atomicity), https://www.rfc-editor.org/rfc/rfc9051.html (UIDNEXT lower bound).

## SMTP schema17 work in progress

Client SMTP is now dispatched through the engine/API worker. Schema17 captures SMTP endpoint/principal, Sent policy and local mailbox/UIDVALIDITY/UIDNEXT with frozen send intent. Durable phases prepared→started→accepted→appending→copied enforce no repeat DATA after uncertainty/acceptance and no repeat APPEND without proof. Complete negative DATA replies have a typed reset; generic rejection cannot reset started/accepted SMTP. Arbitrary sent_copy receipts can no longer mark a send applied; exact frozen Sent bytes, placement proof and positive mailbox observation publish atomically. Server Sent policy still returns visible uncertainty and requires implementation.

New locations: store/smtp{.rs,/progress.rs,/apply.rs}, smtp_schema.sql; providers/imap/{smtp/submission.rs,sent.rs}; accounts/imap.rs::smtp_session; operations/smtp.rs and worker readiness. SMTP validates envelope/CRLF/extensions and dot stuffing; strict APPEND collector preserves validated single UID/epoch proof, checks tags and bounds. Configured Sent folder must already be discovered. Production artifacts are still old/schema12; feature artifacts are being rebuilt.

Commands: focused storage compile initially101 due new unconsumed SMTP methods (WIP dead_code); those methods are now consumed with no suppressions. Unit run without escalation101 due denied localhost binds (1pass2fail); scoped localhost rerun `cargo test --locked -p nuncio-engine --lib providers::imap -- --nocapture` exit0,11pass/1filtered,0ignored. Updated `cargo test --locked -p nuncio-engine --test smtp_operation -- --nocapture` exit0,1pass, strengthening DATA marker, independent delivery/copy, foreign placement, byte mismatch rollback and reopen invariants. Focused full API `... --test imap_system smtp_submission -- --nocapture` exit0,1pass/13filtered,0ignored; independent Mailpit delivery and Dovecot Sent exact bytes/counts. Engine Clippy initially101 items_after_test_module; moved open_smtp before tests; rerun exit0.

Currently actual `python3 scripts/verify.py --suite imap_e2e` running shell22349, fresh feature build and six SMTP process-death boundaries with Bcc/binary MIME checks. Added system lost DATA/APPEND ACK and explicit pre-DATA rejection regression, not yet run. Storage negative DATA transitions and server-Sent support still need focused coverage. No full-current gate claim.

Current checkpoint: Nine-command Trash/restore gate complete: `test-results/task12-trash/results.json` all9commands0. Rustfmt, normal and feature workspaceClippy, proto-cli4, engine69, IMAPsystem13, actualIMAPE2E4, Googlesystem21, actualGoogleE2E26. Zero failed/ignored. Fresh feature builds and copied nested suite logs/results retained. Native and hidden-MOVE fallback Trash/restore both pass, including before-publication SIGKILL, no-op and full-resync origin retention. Source/test schema16; production artifacts still Task11/schema12. No process running; no live compatibility or remote CI claim. SMTP is next.

## Task11 independent Calendar permission groundwork

`python3 scripts/verify.py --suite google_mock_contract`: build0/suite0, **24 passed**, zero ignored; feature workspace Clippy0. New raw HTTP contract covers reader403 with no state/version/notification effect, forbiddenForNonOrganizer guest flags, allowed attendee-copy content edits, own RSVP preserving others/unknown private properties. Initial behavioral red was200 versus403. Role configuration includes the standalone mock control. No production Calendar write adapter exists yet; these are mock-conformance results only.



## September11 Google mail gate complete; Task11 active

`python3 test-results/task10/run-gate.py`: **all12 commands exit0**, including fresh production/test artifacts, fmt, normal/feature Clippy, normal workspace118 tests, feature workspace118 tests (overlapping), separate mock23/system16/E2E21/operation6/release1. All zero ignored. Full command/log/status records: test-results/task10/results.json.

A subsequent format-contract correction requires action files to contain integer schema_version1 and rejects unknown fields. Two genuine reds exposed unversioned archives being accepted and unknown fields being ignored on a serde internally tagged unit variant. Archive now uses an empty struct variant with unchanged JSON meaning. `python3 test-results/task10-action-version/run-gate.py`: **all11 commands exit0**: refreshed production/test builds, fmt, both Clippy modes, normal/feature action-file unit1 each, storage mail_changes1, E2E21, operation_system6, release isolation1. Existing E2E also verifies invalid files leave zero operations. Evidence: test-results/task10-action-version/results.json. Full workspace118 counts precede the added unit; do not claim a new full119 run.

Current local production executables: target/production/release/nunciod and nuncio-cli, SQLCipher schema11, plus target/test-harness/debug counterparts. They are development artifacts; no install, signing, packaging, live provider check or remote CI run occurred. Task10 is offline-verified; Task09 still needs Calendar typed intent/state coverage, Task11 Calendar writes is next, Synology and Tasks12–16 remain. All R01–R16 remain incomplete or awaiting broader evidence.



## Gmail mutation checkpoint (Task10 full gate running)

Current source uses schema11. New domain/mail_change.rs, store/mail_changes.rs and mail_change_schema.sql implement typed immutable desired state, request fingerprints, a durable per-message sequence, and acknowledgement/projection transactions. The owned worker handles mail_change under the same account lane as sync/send. providers/google/send.rs now shares bounded HTTP response classification as WriteResult and Accounts::gmail_write; it still disables implicit retries. Mail.ChangeMessage/GetCapabilities and CLI mail change/capabilities expose this through the public boundary. Source, API and CLI all remain local-development artifacts.

- `cargo test --locked -p nuncio-engine --test mail_changes --test operation_pages --test operation_recovery --test smtp_operation`: exit0, **4 passed**, zero ignored. New storage test proves account/request identity, source disappearance/replacement, insertion order with equal timestamps, delayed inverse actions, receipt validation rollback, atomic observed labels, unknown-label preservation and restart.
- `python3 scripts/verify.py --suite operation_system`: build0/suite0, **6 passed**, zero ignored. New mutation loop checks authentication/crossover/missing action/system-label misuse, before400/404/401/429/503 and after disconnect/malformed/truncated/503. Remote labels and absence of sends are independent evidence; lost responses reconcile with one POST.
- `python3 scripts/verify.py --suite google_e2e`: build0/suite0, **21 passed**, zero ignored. New actual CLI tests exercise all9 label states, repeat identity/no extra POST, dedicated trash/untrash paths, complete local/remote memberships; crashes before HTTP/before receipt/after provider application and external label edits while down; reply/reply-all/forward with binary attachments, thread/headers/recipient arrays and four simultaneous duplicate requests; numeric and HTTP-date one-hour backoff through death/wake while a second account sends.
- Feature workspace Clippy exit0 before the last reply/backoff tests. Full normal/feature gate now running via test-results/task10/run-gate.py; inspect results.json before claiming full success. Production binaries remain old until its build-production entry succeeds.

Behavioral red: missing CLI capabilities returned2 rather than0 (google-e2e-Fh57GT). Storage test initially used an invalid fixture shortcut to set account connected; corrected to prepared credential/connect_google transaction. Initial system changed-request assertion expected Aborted, but the existing public contract uses FailedPrecondition; expectation corrected without changing implementation. Initial E2E snapshot accessor referenced the wrong harness API and Clippy requested collapsing a condition; both compile/lint corrections, not behavioral evidence.



Latest continuation: google_e2e build0/suite0, **17 passed**, zero ignored; feature workspace Clippy0. New send_crash_e2e proves frozen prepared-forward intent survives source deletion/full sync, cancellation before dispatch produces zero sends, pre-HTTP death stays uncertain with zero sends, and post-acceptance/pre-receipt death reconciles exactly one. Editing a queued draft cannot change transmitted content. Actual CLI WatchChanges proves resolution/resend ordering; targeted operation_system google_send_system passes1/4filtered with enqueue/start/finish revisions. The full suite previously passed5 before the added stream assertions. Historical evidence below remains dated; no full Task09/10 gate or new production build yet.


## Current Google send checkpoint — Task 10 active, full goal incomplete

`nuncio-engine/src/operations.rs` now owns the write worker, shutdown, per-account/global sequencing, bounded composition and crash reconciliation. `providers/google/send.rs` and Accounts::gmail_send post only committed frozen MIME. Explicit HTTP retries are disabled; journal attempts own request retries. Typed outcomes distinguish positive rejection, accepted submission and uncertainty. Positive Sent reconciliation matches unique account-scoped Message-ID, selected headers and exact body bytes. The CLI supports mail send --wait, operation wait/list/attempts/show/cancel/resolve; failures preserve error.operation. System status exposes operation_worker_error at new proto field9 (existing sync remains8). Resend/audited resolution now run through actual subprocesses. Gmail mutations, Calendar writes, all Synology transports and later tasks remain.

Verified commands at the send checkpoint:

- `python3 scripts/verify.py --suite google_mock_contract`: exit0, **23 passed**, independent raw HTTP/provider contracts. Its normal dependencies still exclude engine/proto.
- `python3 scripts/verify.py --suite google_e2e`: exit0, **15 passed** at the initial complete send/recovery/resolution checkpoint. Independent mock records verify To/Cc/Bcc, binary bytes, stable frozen Message-ID and exactly one acceptance after repeat/restart. Four crash scenarios kill the daemon after mock acceptance while withholding its acknowledgement: positive read, missing Sent evidence then abandon, explicit manual confirmation, explicit resend. Original request never causes another send; resend causes exactly one additional acceptance with new outer Message-ID and unchanged content, and repeats after another restart create no third send. Logs/results in test-results/google_e2e/.
- `python3 scripts/verify.py --suite operation_system`: exit0, **5 passed** after elapsed-deadline fix and explicit HTTP retry/sender changes. Separate system tests cover before400/429 and afterdisconnect/malformed/truncated/500; accepted-send counts accompany each local state check.
- `cargo test --locked -p nuncio-engine --test smtp_operation --test operation_resolution --test operation_pages --test operation_recovery --test operation_storage --test frozen_message`: exit0, **8 passed** at the initial worker checkpoint. Updated elapsed-deadline pages/recovery/smtp tests: exit0, **3 passed**.
- Normal workspace Clippy exit0 before the final retry/sender refinements. Feature workspace Clippy exit0 after explicit retry ownership/sender validation. Latest 401 refresh case: `cargo test --locked -p nuncio-test-support --test operation_system google_send_system`, exit0, **1 passed /4 filtered**; exactly one initial OAuth exchange plus one refresh and two separately journaled dispatch attempts. Full system/E2E/feature Clippy reruns after this last change are in progress; see SESSION-STATE and the suite results before making newer claims.

Failures found and fixed: missing mail send --wait (CLI2); missing operation wait for replacement (CLI2); an immediate Retry-After expired before SQL commit, leaving running state (full system profile nuncio-system-6sybQ1, deterministic pages test InvalidInput). Nonnegative past deadlines now mean immediate eligibility at commit time; negative deadlines remain invalid. A 401 originally paused auth and left retry_wait, fixed by refresh and a later separately journaled retry (profile nuncio-system-gZacIL). The first request-count assertion counted aggregate rows rather than their count field; fixed to sum RequestCount.count while retaining the exact expected2 (profile nuncio-system-LHW4NG). Header assertion originally required bare addr-spec; now independent mailparse parsing checks exact address and count, accepting RFC-valid brackets. No timing/assertion protections were removed. Compiler-only errors from proto tag collision, stop receiver borrow, channel ownership and AppError fields were corrected without treating compilation as behavioral red evidence.

A successful Gmail response is recorded as provider acceptance, not final-recipient delivery. Conservative MIME matching may leave live-normalized messages uncertain. Sources checked: [Gmail send](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages/send), [message/search fields](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages), [error and sending-limit semantics](https://developers.google.com/workspace/gmail/api/guides/handle-errors). reqwest0.12.28 local source documents default protocol retries; production Google client now explicitly uses retry::never. No real providers or remote CI ran. Current test artifacts are schema10; production artifacts still Task08/schema6.

## Earlier Task 09 checkpoint — superseded above

The historical checkpoints below are superseded where they describe missing list/history/resolve. All work is offline, uncommitted, schema 10. Operation listing/attempt history are bounded and revision/query/account scoped; receipt observations have timestamps. ResolveOperation has versioned explicit abandon/confirm_applied/resend decisions, one immutable audit, and retry idempotency. Manual confirmation remains visibly manual; current confirmation evidence is Google-send-specific. Resend atomically links a new identity, retaining frozen MIME content after draft deletion. Both send/resolve RPCs stop on shutdown/deadline. CLI requires a decision file and a separate duplicate-risk flag for resend. SMTP partial acceptance is now durably separate from Sent-copy confirmation; no SMTP transport is implemented yet.

- `cargo test --locked -p nuncio-engine --test operation_pages --test operation_recovery`: exit 0, 2 passed after list/history. `cargo clippy --locked --workspace --all-targets --features nunciod/test-harness,nuncio-cli/test-harness -- -D warnings`: exit 0 at list/history and again after resolution API/CLI wiring, before SMTP additions.
- `cargo test --locked -p nuncio-test-support --test operation_system`: exit 0, 4 passed after resolution guards. Auth and queued-state rejection for ResolveOperation; previous CRUD/upload/preparation/concurrent enqueue/read/history assertions retained.
- `python3 scripts/verify.py --suite google_e2e`: exit 0, build and suite both 0; 13 passed. Fresh actual binaries include resolution/list/history/shutdown wrappers. CLI rejects queued abandonment, requires explicit duplicate-risk flag, and still rejects resend of queued work even with that flag. Independent sends remain zero. Logs/results: test-results/google_e2e/. Later SMTP receipt and desired-metadata edits are not included in that build yet.
- `cargo test --locked -p nuncio-engine --test smtp_operation --test operation_resolution --test operation_recovery --test frozen_message`: exit 0, 6 passed. Audit idempotency/scope/stale version/active-attempt conflict; unchanged real attempt records under manual confirmation; resend after draft deletion, byte-identical MIME bodies with changed outer identity/date; SMTP receipt survives close/reopen/recovery and is required before Sent-copy completion. Later desired send metadata edits require rerun.
- Behavioral red: operation_recovery exit 101 when an Applied Google receipt without provider ID was accepted; corrected to require provider evidence and reject manual_confirmation via automatic attempt finish. Behavioral red: smtp_operation exit 101 when copy alone proved send; fixed with durable SMTP acceptance receipt and transition guard. Compiler-only reds for new resolution APIs, test delete_draft signature and blob_chunk return shape were corrected, not counted as behavioral evidence.

Positive system/E2E resolution, full operation outcome matrix, typed mail/calendar intent, dispatch/mutations and the final Task 09 normal/feature/production gates remain. Production artifacts are still Task 08/schema 6. No claim of live compatibility or remote CI.

## Historical Task 09 checkpoints — superseded above

Latest checkpoint: Engine::send_draft now freezes an immutable draft snapshot and durably enqueues it. It checks request identity before touching mutable drafts, serializes bounded MIME work, rechecks identity after admission so concurrent identical requests all return the same operation, and retains the composition permit through the queued SQL write. At most 64 send admission waiters; one composition memory slot. SendDraft has a 30-second/shutdown-aware RPC wrapper; a lost local acknowledgement requires retrying the same request identity. New authenticated Operations GetOperation/CancelOperation and CLI `mail send --account --draft --request-id [--version]`, `operation show/cancel --account --operation` exist (`operations` alias accepted). Sending currently only enqueues: no provider dispatcher or --wait exists yet. Paused/disconnected accounts can queue offline. Complete list/history/resolve methods still pending.

Latest observed checks: operation_storage + operation_recovery + frozen_message exit 0 (3 passed). Feature workspace Clippy before the latest API test additions exits 0. `cargo test --locked -p nuncio-test-support --test operation_system` exits 0 (4 passed, zero failed/ignored). It tests 8 simultaneous identical SendDraft RPCs returning exactly the same operation; auth on SendDraft and both Operations methods; changed request payload conflict; draft edits/deletion and restart preserving queue identity; paused account yields independently zero accepted sends. Initial new system compile used nonexistent DisconnectAccountRequest; corrected to actual AccountRequest. Actual `python3 scripts/verify.py --suite google_e2e` exits 0 with a fresh schema-10 test build and 13 subprocess cases, including paused queue identity through daemon force-kill/draft deletion and cancellation, with zero provider sends. E2E logs/results at test-results/google_e2e/. The final SendDraft shutdown wrapper was added during that build run, so its exact binary inclusion is not established; source system tests cover the latest source. Latest feature Clippy initially failed because shared Operations client helpers were considered dead code in google_system’s private support module. Made that reusable test support module public, matching operation_system; no assertion or production lint was removed. Feature Clippy rerun exits 0. Production artifacts still schema 6; no Task 09 full normal/feature gate or production rebuild yet.

Next action: implement bounded operation listing and attempt/receipt history, complete outcome/state tests (rejected retry/definitive fail, conflicts, unresolved outcomes), and audited resolution. Keep abandonment visibly uncertain, only acknowledge positive success evidence, and require explicit duplication-risk acknowledgement plus a fresh request identity for resend; retain frozen content independent of deleted drafts. SMTP accepted/send-copy substeps still need durable partial receipts before Task 12 dispatch. Then Google send/mutation executor (Task 10), Calendar writes (11), full independent Synology services (12), maintenance/bounds/artifacts (13–15), and deferred live worksheet (16). Task 06 pending-operation preservation through projection reset and Task 08 operation transition stream integration are still owed. Do not mark Task 09 or goal complete.

Operation journal foundation is now implemented in store/{operation_schema.sql,operations.rs,operation_attempts.rs}; source schema 10 and exact schema assertions updated. EnqueueSend atomically inserts unique (account_id,request_id), canonical request fingerprint, desired send intent, immutable wire/Sent blobs and metadata, then one operation change revision. It checks the draft snapshot version on first insertion; identical retries return the original operation even after draft edits/deletion, while a changed expected-version/request fingerprint conflicts. Queued cancellation is durable/idempotent. No Operations RPC/CLI or Engine send facade exists yet.

`cargo test --locked -p nuncio-engine --test operation_storage`: exit 0, 1 passed. It tests concurrent duplicate requests, changed payload conflict, stale first enqueue rollback, ownership, frozen bytes surviving draft edits/deletion and reopen, cancellation and exact operation revision. Initial missing-interface compile red was followed by a borrow error in the blob lookup closure; cloning its ID fixed the borrow without changing semantics. Feature workspace Clippy then exited 0.

New operation_recovery test first failed compilation on absent attempt/recovery interfaces. Added Dispatch versus Reconcile admission, atomic attempt/state changes, typed outcomes, bounded receipts, and startup recovery before servers/jobs. Running work becomes uncertain with needs_reconciliation; ordinary dispatch/cancellation cannot bypass it. A rejected reconciliation read cannot establish non-delivery. Positive receipt can finish reconciliation. RetryRepeatable currently permits only mail_change, never send/calendar notifications/copy. Schema has durable attempt/receipt/resolution tables, but resolution actions, list/pagination, partial SMTP receipt substeps and full outcome matrix testing are not implemented. Operations remain storage-only; no remote actions have occurred. `cargo test --locked -p nuncio-engine --test operation_storage --test operation_recovery` exits 0 (2 passed, zero failed/ignored). Feature workspace Clippy exited 0 at that checkpoint.

Next action: finish attempt/outcome tests and audited resolution, expose operation inspection/list/cancel/resolve and send enqueue through Engine/proto/CLI, retain composition admission permits through frozen MIME queue writes, add request-identity lookup before freezing on retries, and validate via separate operation_system plus actual subprocess E2E. Then implement real Google executor in Task 10 (no blind resend) and Calendar mutations in Task 11. Old production artifacts schema 6; test binaries schema 9; source schema 10. Full mocks for both providers remain the scope; Synology strict local servers are still Task 12. Do not mark Task 09 or the goal complete.

Frozen MIME work has begun: added mail-builder 0.5.0 with default features disabled to workspace/engine; `cargo fetch` exits 0 (one new locked package, no installation). `domain/submission.rs::freeze` validates sender/recipients, fixes caller-supplied Message-ID/Date, preserves reply headers, clears Gmail thread targeting on changed reply subjects, creates multipart alternative/related/mixed structure, and bounds encoded wire MIME at 64 MiB. Leaf attachment base64 is built with the existing base64 crate plus bounded MIME line wrapping so inline text bytes are not newline/charset-normalized. Nested message/rfc822 and multipart attachments retain raw 8bit bytes; future strict SMTP must validate/announce their transport requirements. SMTP wire excludes Bcc using mail-parser header offsets while its separate local Sent copy retains Bcc; Gmail submission retains Bcc for envelope delivery. No outgoing provider calls or journal integration exist yet.

`cargo test --locked -p nuncio-engine --test frozen_message`: initially failed missing module after dependency fetch, then failed an attachment-order assumption (inline PNG appears before cafe.txt in the related MIME structure). Corrected the assertion to identify the exact filename/CID, retain exact byte checks, assert attachment count and charset, and prove the private Bcc address is absent from SMTP wire. Current targeted result exits 0 (1 passed). Feature workspace Clippy exits 0 before that last test-only assertion adjustment. Frozen bytes are not yet stored/queued; next action remains operation schema/domain/store/API/CLI and enqueueing immutable send snapshots. API/CLI E2E binaries still represent the verified draft-preparation source before this pure frozen-MIME module addition. R05/R08 are not complete.

Latest Task 09 checkpoint: reply/reply-all/forward preparation now works through engine, authenticated PrepareDraft, and actual CLI. Schema 9 adds draft context and attachment/upload MIME parameters. Domain prepare.rs retains Reply-To arrays, visible To/Cc, self exclusion, Message-ID/References (including single In-Reply-To fallback), plain/HTML forward bodies, exact non-UTF-8 attachment bytes and inline Content-ID/disposition. Prepared drafts are created with their blobs/context in one SQLCipher transaction; a per-profile memory permit stays owned through parsing and the queued write, including cancellation. Original raw bytes are resolved with account ownership, length/hash checked, and parsed off the async reactor. Draft edits preserve reply context; source resync cannot rewrite local draft tables.

Evidence: `cargo test --locked -p nuncio-engine --test draft_domain --test draft_storage --test draft_upload --test draft_prepare --test mime --test calendar_schema` exits 0 (10 passed, zero failed/ignored). Explicit upload begin→concurrent edit→finish proves publication rechecks version. Decoded header injection is rejected. Feature workspace Clippy exits 0. New independent raw HTTP/mailparse fixture contract passes (1, 22 filtered); full mock suite is now 23 cases but has not been rerun together at this checkpoint. `cargo test --locked -p nuncio-test-support --test operation_system` exits 0 (3 passed), including auth/isolation/offline prepare, context-preserving edit, upload failures and held-stream shutdown. Initial prepare system compile used nonexistent h.system(); corrected to existing h.authenticated(). `python3 scripts/verify.py --suite google_e2e` exits 0 with fresh test binaries, 12 actual subprocess tests passed. Reply/forward E2E stops the independent mock before preparation, checks unchanged request counts and no sends, then kills/restarts the daemon and compares complete drafts. No live compatibility or remote CI claim. Source/test binaries schema 9; production artifacts still Task 08 schema 6. Fixture provenance and operating commands updated in fixtures/README.md and docs/RUNNING.md.

Next concrete action: durable operation journal and frozen MIME for send, followed by dispatch in Task 10 and Calendar writes in Task 11. Native apps excluded. Implement atomic account/request uniqueness, desired state plus intent, complete state transitions, durable attempts/receipts, cancellation before dispatch, restart reconciliation and explicitly audited uncertain/conflict resolution. Do not blindly resend. Mail-builder 0.5.0 official docs were read (stored reference) but dependency has not yet been added. Potential draft polish to include before final acceptance: attachment remove/download through API/CLI; bound maximum valid aggregate draft response (default gRPC 4 MiB may be less than worst-case metadata+body); test prepared draft survival when original message is deleted/full-resynced; stress cancelled parser/storage lifetimes in Task 14. Full normal/feature workspace gates remain pending for Task 09. All Tasks 10–16 remain. Earlier checkpoint paragraphs below are historical, superseded where they say preparation is missing.

Task 09 is in progress. Durable draft CRUD and chunked attachment upload now work through the engine, authenticated Mail API, and actual CLI. Schema 8 adds encrypted draft_uploads/draft_upload_chunks staging; source is in engine/store/{drafts,draft_uploads}.rs and draft_*_schema.sql, engine.rs, proto mail.proto, daemon {drafts,mail}.rs, CLI {drafts,draft_upload}.rs. CLI uses regular local files, hashes before streaming, and sends 256 KiB chunks with expected size/hash/version. No daemon file paths. Four uploads/profile, 64 MiB aggregate draft attachments, 30-second RPC deadline. RAII guards reserve cleanup queue space and hold worker ownership; cancellation enqueues cleanup and startup removes crash leftovers. Only complete digest/version-validated data publishes an immutable blob, attachment and one draft revision atomically. No attachment removal/download or reply/forward preparation yet; operation journal and all writes remain pending.

Latest checks: cargo test --locked -p nuncio-engine --test draft_upload exits 0 (1 passed); cargo test --offline -p nuncio-test-support --test operation_system exits 0 (2 passed); feature workspace Clippy exits 0. Commands run from rebuild/. Upload system coverage includes authentication, malformed order/chunk/offset, incomplete EOF, wrong digest, concurrent stale edit, zero-byte attachment, held-stream shutdown within five seconds and restart. Initial system compile failed because tokio-stream was missing from test-support dev-dependencies; reused the existing workspace dependency. New CLI futures-util likewise reuses the lock dependency. Latest `python3 scripts/verify.py --suite google_e2e` exits 0 with fresh test binaries and 11 passing actual subprocess cases (zero failed/ignored); logs in test-results/google_e2e/. Draft case verifies attachment size/hash, stale edit/delete, wrong account, header injection, daemon force-kill/restart and full provider projection reset preserving the entire draft. No sends recorded independently. Earlier missing save/attach reds remain historical evidence. Normal/feature full workspace gates have not yet been rerun for Task 09; production artifacts still verified Task 08/schema 6, test binaries now schema 8.

DraftContent validation uses email_address 0.2.9: addr-spec, no injected header/control characters, 1000 recipients and 1 MiB combined body. Drafts allow incomplete composition. CRUD domain/storage tests previously passed (one each). Required operation_system suite and shared OAuth helpers exist. All earlier Task 08 evidence below remains valid for that checkpoint, not full current source verification.

Next concrete action: reply/reply-all/forward creation with self exclusion, preserved threading and attachment MIME parameters, then durable operation journal. Inspection confirms domain/mail.rs already transfer-decodes attachment bytes without charset conversion and mime.rs has the non-UTF-8 cafe-byte regression. The earlier warning that this code still normalized attachment bytes was stale. Forwarding must still preserve charset/disposition parameters (currently only MIME essence is stored in attachment metadata); add independent fixture/system/subprocess assertions. Draft editing must not modify frozen enqueued MIME. Task 06 still owes operation survival; Task 08 still owes operation-transition revisions. Tasks 10–16 and CI egress denial remain. Full offline mocks for both providers are current scope; no live setup requested or live calls authorized. Do not stop at this draft milestone.

## Task 08 — reader runtime verified offline; operation-transition checks pending Task 09

Final `test-results/task08/run-gate.py`: exit 0, eleven subcommands all exit 0. Fresh local builds, fmt, normal/feature Clippy, normal/feature workspace **85 passed / 0 failed / 0 ignored each**, separate mock **22**, system **16**, actual daemon/CLI E2E **10**, release isolation **1**. Exact commands/statuses/logs in test-results/task08/results.json. Production artifact paths target/production/release/{nunciod,nuncio-cli}; schema 6. Initial schema-assertion failure retained separately. Full --all still awaits IMAP suites; no live/remote-CI claims. RUNNING.md/TESTING.md describe polling, retry, cancellation, wake controls and failure receipts. Earlier checkpoints below are historical.

Task 08 current checkpoint: schema 6, owned background scheduler, per-account sequencing, coalescing, change stream, and detailed per-scope status are implemented. Normal production polling defaults to 60 seconds with two active accounts. Retry-After seconds and HTTP-date parsing is implemented (httpdate reuses an existing lock dependency); Gmail/Calendar hints are scoped, OAuth hints cover both APIs, and explicit queued sync waits outside permits and remains cancellable. Sync success/failure/deadlines survive restart. Scheduler uses monotonic deadlines with forward wall-clock catch-up; feature-only clock-offset-ms models suspend without real sleeping. Test harness epochs now advance across subprocess/system restarts.

Latest evidence: feature workspace Clippy exit 0; focused status/change stream 1 passed; schedules/changes/sync_runs 3 passed from prior checkpoint; scheduler System tests 2 passed; exact HTTP-date mock conformance 1 passed (20 filtered); retry parser unit 1 passed. Full actual google_e2e now passes 9 tests (runner exit 0), including crash-preserved one-hour backoff, simulated wake, expired access refresh, and mail/Calendar convergence, with independent zero send/notification assertions. Initial retry red: deadline 1772895601348 vs minimum 1772895606000 (system nuncio-system-4tyjRu). Initial wake red: 8 passed/1 failed, missing clock-offset consumption (google-e2e-MtUba9). These behavioral reds are resolved. The old full-reconciliation System assertion now also verifies exactly one scoped sync_schedule revision in addition to exact sync_run records and unchanged complete mail projection/coverage.

Next concrete action: finish the new OAuth HTTP-date System regression (running full google_system), improve CLI --wait failure receipts so the run remains inspectable, review/test status and cancellation semantics, then run fresh Task 08 normal/feature/full/separate gates and rebuild local production binaries. Production artifacts still represent Task 07 until that fresh build. Tasks 09–16 remain; do not stop at the reader milestone. Task 06 draft/operation preservation is owed after Task 09. Task 03 CI egress enforcement is owed with 12/15. Task 14 keeps memory/stalled stream/worker lifecycle stress. Live acceptance is deferred by user steering: full mocks for Google and Synology for now; no credentials or live calls. Historical narrative below records earlier checkpoints and is superseded by this paragraph where it describes missing Task 08 pieces.


Change log/read stream/schema 5 and transactional sync progress are implemented. Targeted engine command `cargo test --locked --manifest-path rebuild/Cargo.toml -p nuncio-engine --test sync_runs --test changes --test worker_lifecycle --test calendar_projection --test mail_projection` exits 0 (5 passed). WatchChanges auth/replay/future cursor/shutdown System test exits 0. CLI watch first failed at missing command (2 vs 0), then passes in full actual E2E (6 passed; runner exit 0). The crash test preserves complete mail-data/coverage equality and independently asserts each changed global revision is progress for that run. Initial system full rerun had the equivalent obsolete revision equality (25 vs 21), which is now replaced with full response equality plus revision-history verification; rerun pending. No background scheduling is implemented yet. See SESSION-STATE for exact next action and worker teardown evidence/limits.

Full separate google_system now exits 0 (13 passed) after validating progress revisions independently while preserving full mail response equality. Actual google_e2e then passed 7 after shared per-account sequencing and coalescing. The new automatic-polling E2E is the current red: 7 pass / 1 fails at the 8-second initial convergence assertion; no scheduler exists yet. Artifacts google-e2e-r5pGEa; earlier coalescing red google-e2e-cnbeRK. Feature workspace Clippy exits 0 after coordination and the new shared production/test clock. No Task 08 completion claim.

Schema 6 schedule storage: `cargo test --locked --manifest-path rebuild/Cargo.toml -p nuncio-engine --test schedules --test sync_runs --test changes` exits 0, 3 passed / 0 failed / 0 ignored, after missing-interface compile red. This proves durable scoped retry state, success/reset timing, and independent Calendar deadlines; it does not prove HTTP guidance handling or automatic dispatch, which remain pending.

## Task 07 — verified offline

- Initial actual Calendar E2E failed at missing calendar refresh: CLI **2** vs expected **0**, inner test **101** / runner **1**; four prior E2Es pass. Artifacts: `google_e2e/runs/google-e2e-BiBqVG`. This initial red is resolved by the implemented RPC/CLI; full Task 07 gates follow.
- Independent mock gained calendar removal and `calendar/all-day-2.json` with provider ID `alldate02`; the fixture occupies 2026-10-31 and 2026-11-01, ending exclusively on 2026-11-02. `python3 rebuild/scripts/verify.py --suite google_mock_contract`: **exit 0, 20 passed / 0 failed / 0 ignored**. Removal/account isolation, event 404, and date overlap use raw HTTP independent of production domain types. Initial removal test failed compilation on its absent control before implementation.
- `cargo test --offline --manifest-path rebuild/Cargo.toml -p nuncio-engine --test calendar_domain`: **exit 0, 2 passed**. Date/offset/TZID/sparse cancellation/unknown fields, date-window validation, exact DST midnight boundaries. Initial test compile failed on absent module. Direct engine chrono/chrono-tz dependencies reuse existing workspace/lock packages. Targeted feature engine Clippy exits 0.
- Calendar schema 4 in store/calendar_schema.sql: `cargo test --locked --manifest-path rebuild/Cargo.toml -p nuncio-engine --test calendar_schema --test mail_projection --test store_lifecycle` exits 0, **8 passed / 0 failed / 0 ignored**, following the calendar_schema red on missing table. Fmt exits 0. This checks composite ownership/encryption/reopen and prior mail projection/lifecycle behavior; At that checkpoint Calendar facade/adapter were still absent.
- Added store/calendar.rs; `cargo test --locked --manifest-path rebuild/Cargo.toml -p nuncio-engine --test calendar_projection --test sync_runs --test mail_projection`: exit 0, **3 passed**, after new Calendar facade test failed compilation on missing methods. Targeted feature engine Clippy exits 0. Calendar HTTP/application compiled next; separate RPC/CLI tests remain pending. Common run method renames preserve Gmail cursor preconditions and expand recovery to Calendar staging.

- Calendar local queries passed calendar_projection/mail_projection/sync_runs (3 tests, exit 0). RPC/CLI wiring then passed `python3 rebuild/scripts/verify.py --suite google_e2e` (5 passed, exit 0). New Calendar System scenarios first exposed the missing catalog-timezone fallback (runtime failed versus succeeded) and a wrong fault path in the test. The provider matcher now targets an independently observed route. The production fix persists event-response timezone and resets canonical state on timezone changes.
- `cargo test --locked --manifest-path rebuild/Cargo.toml -p nuncio-test-support --test google_system calendar_cases`: exit 0, 3 passed / 9 filtered, after fixes. Canonical masters/recurrence/attendees/reminders, moved/cancelled exceptions, paging/isolation/authentication, stale expansion, tombstones, expired reset and removal failures are exercised. `cargo test --locked --manifest-path rebuild/Cargo.toml -p nuncio-engine --test calendar_projection --test calendar_domain --test calendar_schema`: exit 0, 4 passed. Feature workspace Clippy exit 0. The final E2E assertions added after its earlier pass still need the fresh gates below.
- Mock optional Calendar catalog omission control is independently tested: absent catalog timeZone leaves authoritative events timeZone intact. Targeted contract test exit 0 after missing-control compile red. Control is also exposed on standalone stdin as omit_calendar_fields; no production test route.

- Final Task 07 gates recorded by `python3 rebuild/test-results/task07/run-gate.py`: **exit 0**, all eleven subcommands exit 0. Fresh isolated test-harness and production builds; fmt; normal/feature Clippy; normal/feature full workspace **70 passed / 0 failed / 0 ignored each** (overlapping configurations); separately run mock contract **20**, system **12**, actual daemon/CLI E2E **5**, release isolation **1**, all passing. Exact commands/statuses in `test-results/task07/results.json` and named logs. E2E includes default rolling refresh, switching agenda windows, offline master get and all-day November dates after provider shutdown/daemon restart, and unchanged independent HTTP request counts. Local binaries at target/production/release/{nunciod,nuncio-cli}; nothing installed, no remote CI/live acceptance. Overall --all still awaits IMAP suites.

## Task 06 — reconciliation tested; cross-feature preservation pending

- `python3 rebuild/scripts/verify.py --suite google_system`: exit 0, **9 passed / 0 failed / 0 ignored**. Initial-list response is withheld while independent remote state changes; the engine catches additions, removals, and label changes. Both page caps 1 and 10 pass, including cursor expiration during the incomplete generation and an observed second profile capture.
- `--suite google_e2e`: exit 0, **4 passed / 0 failed / 0 ignored**. The new case force-kills the real daemon before content staging, before projection promotion, and after promotion. Restarts retain old complete data or the new committed projection, preserve message IDs/membership, and reconcile to the independently observed provider cursor. Request counts prove retained history is used after restart; no sends occurred. First red run failed to enter the checkpoint because E2eHarness did not pass its barrier directory. Failure preserved at `google_e2e/runs/google-e2e-leBEny`; adding that feature-only configuration fixed it.
- The subprocess case now passes all three crash boundaries with both page caps 1 and 10 (six actual kill/restart scenarios). Task 06 still requires preservation assertions after drafts/operations exist in Task 09. No draft/operation functionality is claimed yet. Runtime evidence copied to test-results/task06/{google_system,google_e2e}-*.log; both suites exit 0.

## Task 05 — verified offline

- Implemented engine Gmail HTTP ingestion and credential coordination, mail sync ownership, domain MIME, store staging/promotion/queries/blobs/sync runs, System sync RPCs, Mail RPCs, and CLI sync/mail commands. `RUNNING.md` documents use and current limits.
- Final `test-results/task05/{fmt,clippy,clippy-test-harness,tests,tests-test-harness}.log`: all **exit 0**. Full workspace **59 passed / 0 failed / 0 ignored** in each normal/feature configuration (overlapping totals). `run-gate.py` and `results.json` preserve exact commands/statuses. Separate suite logs were copied into this directory before later Task 06 additions: mock contract **19**, system **8**, actual E2E **3**, production isolation **1**, all passing.
- E2E covers cap-1 pages, two accounts with duplicate remote IDs, stopped actual mock listener, daemon force-kill/restart, cached read/search, account ownership denial, exact MIME/PDF hashes, and unchanged independent provider request counts. System coverage includes explicit missing fields/oversized payload states, on-demand fetch, inert HTML, query-token binding, anonymous rejection on all new RPCs, and transactional projection/cursor behavior. Task 06 additionally validates changes and cursor expiry during initial scans.
- Local production artifacts rebuilt and tested at `target/production/release/{nunciod,nuncio-cli}`. Test builds remain `target/test-harness/debug`. No installation, commit, live account access, or remote CI ran. Final overall `--all` still cannot pass until IMAP suites exist; R01–R16 remain incomplete as a whole.

- Initial actual subprocess red connected through real OAuth, then failed at missing sync: CLI exit **2**, expected **0**; runner **1** / inner **101**. A later run successfully ingested three messages but failed a test reference typo `m001` vs canonical `m-001`; only that reference was corrected. Historical failure artifacts remain under google_e2e/runs; current suite logs now pass.
- `cargo fetch --manifest-path rebuild/Cargo.toml`: exit 0; adds mail-parser 0.11.9 and hashify 0.2.9. The dependency is separate from the independent mock's mailparse implementation. [Official parser docs](https://docs.rs/mail-parser/0.11.9/mail_parser/) checked September 10.
- `cargo test --offline --manifest-path rebuild/Cargo.toml -p nuncio-engine --test mime`: exit 0, **3 passed / 0 failed / 0 ignored**. `domain/mail.rs` distinguishes original text/plain from HTML-derived search text, retains duplicate headers and exact PDF/text-attachment bytes, and rejects oversized/unparseable inputs. The initial compile failed on the absent module; this is not counted as behavioral red evidence.
- Schema 3 adds the mail/staging/blob/FTS tables in `store/mail_schema.sql`. The mail_schema test first failed on the absent messages table (exit 101), then passed. `cargo test --offline --manifest-path rebuild/Cargo.toml -p nuncio-engine --test mail_schema --test store_lifecycle --test account_storage`: exit 0, **8 passed / 0 failed / 0 ignored**. It checks cross-account foreign keys, scoped FTS, encryption, reopen, and prior lifecycle/credential invariants.
- Targeted engine Clippy with test-harness and schema fmt: exit 0. These early schema/MIME tests did not establish the vertical; subsequent system/E2E checks above do. Adversarial parser memory bounds and stalled-stream shutdown remain Task 14 work.

Task 05 additional targeted commands: `cargo test --locked --manifest-path rebuild/Cargo.toml -p nuncio-engine --test blobs` exits 0 (1 passed); `... --test sync_runs` first exits 101 on missing-start-cursor assertion, then exits 0 (1 passed) after enforcing the full/delta run precondition. Earlier targeted engine Clippy after blobs exits 0. These establish storage primitives, not the pending Gmail vertical. A failed assertion teardown additionally emitted SIGSEGV; successful rerun exits normally and adversarial shutdown investigation remains tracked for Task 14.

`... -p nuncio-engine --test mail_projection` exits 0 (1 passed), after compilation initially failed on absent Store interfaces. Failure artifacts from the fixture typo: `google_e2e/runs/google-e2e-VlQIWO`.

## Task 04 — verified offline

- Engine `accounts.rs`, `providers/google/{http,oauth}.rs`; schema 2 in `store/{accounts,migrations,worker,mod}.rs`; Accounts protobuf/service/CLI; `docs/RUNNING.md`. CLI remains independent of engine/storage.
- Before production consumption, independent mock conformance was extended for stable OpenID userinfo, optional client-secret validation, canonical email scope spelling, and refresh rotation. The userinfo test first failed 404 vs 401, then passed. Mock normal dependencies still exclude production engine/proto; only harness dev-dependencies compose them.
- Actual daemon/CLI red: connect-google exited 2 because the account command did not exist. Later system red: unfinished browser HTTP headers exceeded the unchanged five-second shutdown assertion; owned bounded Hyper connections fixed the cause. Canonical userinfo.email scope caused false scope denial; the documented equivalent is now accepted without relaxing required mail/calendar permissions.
- Separate mock conformance: **18 passed**. Separate real-engine/SQLCipher/RPC system: **6 passed**. Actual daemon/CLI subprocess E2E: **2 passed**. Production release isolation: **1 passed**. Each suite has zero failed/ignored and exits 0 through `python3 rebuild/scripts/verify.py --suite NAME`.
- Evidence includes two-account identity isolation, authenticated rejection on every Accounts method, state/Host/PKCE checks, consent denial, expired codes, missing scopes, one refresh for concurrent calls, rotated-token persistence across daemon death, revoked credentials pausing only one account, stable-subject reconnect, disconnect credential removal with retained account identity, secure-store write failure compensation, deletion cleanup across engine restart, explicit post-commit cleanup warning, and lost rotated-refresh acknowledgement requiring reauthorization. One-use/reused-code behavior is independently established by mock conformance.
- Request Debug output is explicitly redacted. E2E scans process logs for synthetic secret canaries and database/WAL bytes for plaintext credentials/address. Production accepts no test configuration. OAuth provider HTTP uses fixed HTTPS URLs, no redirects/proxies, bounded bodies, and redacted typed errors; only the test feature substitutes validated numeric-loopback endpoints and requires synthetic credentials.
- Final logs `test-results/task04/{fmt,clippy,clippy-test-harness,google_mock_contract,google_system,google_e2e,release_isolation,tests,tests-test-harness}.log`: all **exit 0**. The logs record command argv and exit statuses; individual suite directories hold nested build/test logs. Full workspace: **47 passed, 0 failed, 0 ignored** in each normal/feature configuration, overlapping rather than additive totals.
- Local production artifacts rebuilt and tested: `target/production/release/{nunciod,nuncio-cli}`. Test artifacts: `target/test-harness/debug/{nunciod,nuncio-cli}`. They provide lifecycle/account functionality, not completed mail/calendar functionality. No installation, commit, remote CI execution, or live account access occurred.
- R02 Google prerequisite and R15 account security have offline evidence; Synology credentials, all remaining product tasks, and live acceptance remain pending. This is not full R02 or overall goal completion.

## Task 03 — runtime verified, CI egress enforcement pending

- Implemented `nuncio-test-support/src/process.rs`, `tests/support/system.rs`, `tests/{google_system,google_e2e,release_isolation}.rs`, `nuncio-engine/src/test_controls.rs`, daemon readiness/test configuration, and `scripts/verify.py`. Usage and remaining network-enforcement obligation: `docs/TESTING.md`.
- `python3 rebuild/scripts/verify.py --suite google_e2e`, `--suite google_system`, and `--suite release_isolation`: exit 0. Actual executable status, JSON/stderr, missing daemon exit 4, authenticated health, force-kill/restart, and production rejection of test flags/environment pass. No mock return object substitutes for a process exit.
- `cargo test --locked --manifest-path rebuild/Cargo.toml --workspace` and the same with `--features nunciod/test-harness,nuncio-cli/test-harness --target-dir rebuild/target/test-harness`: exit 0, **38 passed / 0 failed / 0 ignored in each configuration** (overlapping). Explicit absolute binary paths and test artifact parent were set by the logged Python gate.
- Fmt and normal/feature Clippy: exit 0. Evidence: `test-results/task03/{fmt,clippy,clippy-test-harness,google-system,release-isolation,tests,tests-test-harness}.log` and `test-results/google_e2e/`.
- Behavioral failures before fixes: daemon rejected the missing readiness flag; E2E daemon rejected the newly requested test configuration; combined workspace run inherited a harness artifact variable and got production invalid-input exit 2 instead of offline exit 4. The failed log is retained as `task03/tests.previous.log`; child environment isolation fixes the cause without relaxing production checks.
- Test clock and checkpoint tests prove fixed time, release, shutdown cancellation, and path rejection; production-keystore test configuration fails before profile creation. Actual provider/operation consumers of those seams are added in their implementation tasks.
- `python3 rebuild/scripts/verify.py --all`: **exit 2**, required `imap_contract.rs` missing. Required suites are not skipped. Container egress enforcement is pending, so Task 03 is not fully checked off yet.
- `docker info --format '{{.ServerVersion}}'`: exit 0 with narrowly approved socket access, server 29.7.2. No test containers have been started yet.
- Local artifacts `target/test-harness/debug/{nunciod,nuncio-cli}` and `target/production/release/{nunciod,nuncio-cli}` exist. They currently implement foundation behavior, not the final product. No normal-environment installation or remote CI execution occurred.

## Task 02 — verified

- Independent implementation: `crates/nuncio-test-support/src/google/{oauth,gmail,mail_state,calendar,calendar_state,calendar_recurrence,paging,faults,state,wire,mod}.rs`. Normal dependencies do not include the engine or protobuf crates.
- Standalone service: `src/bin/mock-google.rs`; usage and explicit mock compatibility limits: `docs/MOCK-GOOGLE.md`. Synthetic fixtures and official-source provenance are under `fixtures/`.
- `cargo test --manifest-path rebuild/Cargo.toml -p nuncio-test-support --test google_mock_contract`: exit 0; **17 passed, 0 failed, 0 ignored**. Includes actual mock subprocess readiness/reset/shutdown.
- Observations include raw MIME/PDF bytes, metadata/full/raw separation, account identity and labels, sparse string history IDs, reusable account/query-scoped pages, history and Calendar cursor expiry, OAuth code/refresh/PKCE/scope enforcement, DST/all-day/cancellation/instance behavior, conditional Calendar writes, free/busy timezones, and notification recipients.
- Every required fault category is exercised: small caps, overlap, reorder, remote deletion between list/get, invalid cursors, 401/403/404/410/412/429/500/503, Retry-After, malformed JSON, a received body prefix followed by truncation, latency, disconnect before acceptance, apply-then-drop, and apply-then-withhold. Accepted-send bytes and invitation effects are checked through independent control snapshots.
- Behavioral red evidence observed before fixes: initial OAuth route 501 vs 302; missing Gmail/Calendar routes and mailbox state; an injected disconnect still returning a complete acknowledgement; page overlap absent; encoded calendar ID 404; nested typo accepted; invented numeric history ID accepted; unknown scope accepted; truncation initially delivered no prefix; generated recurrence instance get returned 404; requested UTC response remained Chicago. Compiler errors during new interface additions were corrected and are not counted as behavioral test evidence.
- Full gate logs in `test-results/task02/{fmt,clippy,clippy-test-harness,google-mock-contract,tests,build-test-harness,tests-test-harness}.log`; each records the argv and exit status. All exit 0. Normal workspace: **33 passed, 0 failed, 0 ignored**. Test-harness workspace: **33 passed, 0 failed, 0 ignored**. These configurations overlap; do not sum them as unique tests.
- The gate rebuilt daemon/CLI test artifacts in `target/test-harness` and reran their actual subprocess tests. Loopback tests used authorized sandbox escalation after occasional bind restrictions. No live provider or normal OS-keyring access occurred.
- R13 has independent mock conformance evidence; R14 has mock fault prerequisites. Engine/API/CLI mail/calendar operations and full system/E2E acceptance remain pending. No claim of live Google or Synology compatibility, remote CI execution, or overall goal completion is made.

This file records rebuild checks only. Existing-root test counts are baseline context, not evidence of implemented rebuild requirements.

| Date | Check | Result | Scope |
|---|---|---|---|
| 2026-09-10 | `rustc --version`, `cargo --version` | 1.97.1 / 1.97.1, exit 0 | Approved toolchain available |
| 2026-09-10 | `git worktree add .worktrees/nuncio-google-first-rebuild -b feature/nuncio-google-first-rebuild` | Exit 0, HEAD a268fd5 | Authorized isolation; no commit |

All requirement acceptance remains pending. Live Google/Synology acceptance is not authorized and has not run.

## Task 01 — verified

- Red tests: encrypted store open (exit 101), profile initialization (exit 101), daemon serving/CLI exits (exit 101), and actual subprocess mock-keyring startup (exit 101) failed on their missing behavior before implementation. A later newer-schema regression failed byte equality before the journal-mode ordering fix.
- `cargo fmt --manifest-path rebuild/Cargo.toml --all -- --check`: exit 0.
- `cargo clippy --manifest-path rebuild/Cargo.toml --workspace --all-targets -- -D warnings`: exit 0.
- Same Clippy command with `--features nunciod/test-harness,nuncio-cli/test-harness`: exit 0.
- `RUST_TEST_THREADS=2 cargo test --manifest-path rebuild/Cargo.toml --workspace`: exit 0; counts below are measured from the log.
- Production-package test-harness build and full workspace tests using separate `rebuild/target/test-harness`: exit 0. `NUNCIO_E2E_DAEMON` points to that actual daemon; the CLI test executes both binaries with temporary profile and synthetic keystore. Normal builds reject the test secret flag.
- Evidence: `test-results/task01/{fmt,clippy,clippy-test-harness,tests,build-test-harness,tests-test-harness}.log` relative to rebuild. Tests requiring loopback were run under narrowly approved sandbox escalation. No live services or real keychain credentials were used.
- Implemented areas: `nuncio-engine/src/{store,profile.rs,secrets,engine.rs}`, `nuncio-proto/proto/nuncio/v2/system.proto`, `nunciod/src`, `nuncio-cli/src`; tests in each crate. The CLI has no engine dependency.

R01 foundation is demonstrated; full R01 resilience is also exercised again in Task 14. R12/R15 are partial until remaining API/security features and acceptance tests exist. No other requirement is complete; task completion is not overall goal completion.

tests: 16 passed, 0 failed, 0 ignored (overlapping configurations, not unique-test totals).

tests-test-harness: 16 passed, 0 failed, 0 ignored (overlapping configurations, not unique-test totals).


## Task11 Calendar domain and durable intent foundation — September11

- `cargo test --locked -p nuncio-test-support --test google_mock_contract calendar_permissions`: baseline partial-RSVP1pass; new unknown attendee fixture failed400vs200, exit101; strict-request/retained-provider validation split fixed it:1pass23filtered, exit0. Invalid partial RSVPs assert unchanged independent remote objects/version/notification count.
- `python3 scripts/verify.py --suite google_mock_contract`: build0/suite0,24passed0failed0ignored, logs/results test-results/google_mock_contract. Feature workspace Clippy0 at the first intent-test checkpoint.
- `cargo test --locked -p nuncio-engine --test calendar_changes`: initial missing-module red101; implemented typed bounded actions:2passed, exit0.
- `cargo test --locked -p nuncio-engine --test calendar_intents`: initial absent-interface red101; implementation private reexport compile error corrected. Latest combined `cargo test --locked -p nuncio-engine --test calendar_intents --test calendar_changes --test calendar_projection --test mail_changes`:exit0,2+2+1+1passed. Transaction rollback/source/source-disappearance/ordering/create/delete/reopen checks included.
- `cargo test --locked -p nuncio-engine --test calendar_schema`:exit101, old11 assertion vs new12; schema expectations updated. New migration12 is exercised by intent tests; targeted schema rerun pending. No full workspace/system/E2E Task11 run claimed. Calendar writes remain store/domain only until executor/API/CLI integration.

- Latest Calendar intent gate: `python3 test-results/task11-intents/run-gate.py`,all4 exit0 (fmt,both workspace Clippy modes,all engine tests). Count recorded by log summary; Calendar domain3/intents6 included. New behavioral reds for explicit organizer flag, timezone normalization and submillisecond equality resolved. CalendarRetryEvidence missing-interface red resolved; positive proof and delayed-read tests pass. `calendar_schema` updated12 rerun1pass/exit0. Earlier focused counts in this file remain historical, not latest full workspace claims.


## Task11 initial Calendar HTTP/API system verification

Typed ChangeEvent and four action messages, engine enqueue, Calendar HTTP/credential wrapper and operation executor implemented. Initial system red after API enqueue: operation stayed queued, exit101 (profile /var/folders/75/lgqpm2d11_s7jfn7kvx4vxwm0000gn/T/nuncio-system-wmmFAW). Fixed by dispatcher/admission integration. Targeted create test1pass/exit0. Expanded `cargo test --locked -p nuncio-test-support --test google_system calendar_write_system`:exit0,2passed16filtered. First test includes create/dedup/auth/independent invitation and412 conflict; second loops8 lost-disconnect action/policy scenarios. Test method naming/missing unused UUID dependency errors corrected using existing ListAttempts and one fixed UUID per isolated profile. Feature Clippy0 before expansion. New task11-api gate currently running; source no longer store-only. Calendar CLI/freebusy/crash subprocess coverage pending.

- `python3 test-results/task11-api/run-gate.py`: **8commands all exit0**, exact logs/statuses `test-results/task11-api/results.json`. Fresh feature daemon/CLI build,fmt,both workspace Clippy modes; engine49,independent mock24,Google system18,operation system6. Zero ignored; overlapping counts. Includes exact one mutating HTTP call after lost acknowledgements for create/update/delete/RSVP with none/all notification policy. Production/release and actual Calendar subprocess E2E were not run by this gate.


## Task11 Calendar CLI and subprocess checkpoint

- CLI strict action parser:missing-function compile red101, helper signature/test Debug issues corrected;1unit passes. Added null-field behavioral red, corrected present-value deserializer;1unit passes. No engine dependency added to CLI. calendar change maps typed fields to the public RPC.
- `python3 scripts/verify.py --suite google_e2e`: fresh build0/suite0,22passed before crash-loop addition; includes actual Calendar create/dedup/timed edit/clear/RSVP/delete and independent notification recipient/count checks. Existing send/label subprocess regressions passed too.
- Added18 Calendar crash scenarios. Initial suite22passed1failed,inner101/runner1,artifact runs/google-e2e-ERX6s6. Test incorrectly expected newETag after Google's empty DELETE acknowledgement. Corrected only that branch to exact honest tombstone plus independent remote deletion/newETag/version/count/notification assertions; other paths still require full remote/local equality. Official source docs/RUNNING. Rerun session38788 currently pending.

- Calendar crash rerun: `python3 scripts/verify.py --suite google_e2e` build0/suite0,**23passed0failed0ignored**, including18 Calendar SIGKILL/restart cases. Exact remote mutation count/version/notifications and state/identity assertions pass. Logs/results `test-results/google_e2e/`; the earlier failed artifact remains preserved. This proves offline mock behavior only; no live compatibility or remote CI claim.


## Task11 free/busy, recurrence and conflict acceptance

- Independent mock freebusy contract and recurring parent/instance contract: each1passed/exit0; fullmock total25 pending new gate. Domain freebusy1passed/exit0 after missing-module red101. Typed RPC system test1passed after missingproto red101.
- Actual CLI freebusy initial no-env harness failure101, then proper-env missing-command red101 (CLI2 vs0), preserved temp nuncio-google-e2e-ijgcWJ. Build lacked FreeBusyResult Serialize:101, whitelist corrected. `python3 scripts/verify.py --suite google_e2e`:freshbuild0/suite0,**26passed0failed0ignored**. Includes recurringseries+singleDSTedit, staleETag412 exit5, permission guards, strictfreebusyfiles/partialcoverage, independent remoteeffects.
- `cargo test --locked -p nuncio-test-support --test google_system calendar_write_system`:5passed16filtered/exit0 after harness shutdown-name compile error101. Eight Calendar writefaults, six freebusyfaults, persisted429restart, previous create/conflict/eightlostacks all checked. `cargo clippy --locked --workspace --all-targets --features nunciod/test-harness,nuncio-cli/test-harness -- -D warnings`:exit0.
- FullTask11 gate started: test-results/task11/run-gate.py;12commands/results.json/logs. Pending until finalresults. No live/remoteCI claim.


**FullTask11 gate completed:** `python3 test-results/task11/run-gate.py`, all12commands exit0. Fresh feature and production builds,fmt,bothworkspaceClippy modes, normal142/feature142tests (overlapping), separate mock25/system21/E2E26/operation6/release1. All0failed/0ignored. Exact commands and exit statuses in test-results/task11/results.json; complete logs beside it. Production binaries now schema12 includeCalendarwrites/freebusy and pass release-isolation test; no installation/packaging/signing/live/remoteCI claim. Task11plan/TODO checked as offline implementation milestone; fullgoal/R01–R16 remainincomplete.

Task12setup: scopedDocker inspection0, officialregistrytagmetadata0; pinned pulls Dovecot2.4.5 c807be4fb5a97d9c3a90770569d3a6c4cbdcb36742ad41f90409cbd929166553 and Mailpit1.31.1 98b916bd3c8d61f7633a52d3ea2f58d00620cb01ca57ab59edde68c347a95365 both0. Temporarynetworknone doveconf -n0 confirmsDovecot2.4.5; wrong -f service filter89 (read-only diagnostic). New Python independenttest firstmissingservicesmodule1. Composition startup then failednumeric-loopbackguard1; do not weaken guard. Addedretainedtest-results/imap-services/runs diagnostics before nextrun. Noadapter/protocolconformanceclaim yet.


Task12 independentservices checkpoint: `python3 tests/imap/run-tests.py` **bothcommands exit0: server2tests, proxy6tests**. Logs/commands/results.json in test-results/imap-services; source tests/imap. Startupdiagnosis: internalbridgeemptyportbindings (retainedguards) resolveddedicatedbridge; DovecotEPERMshippedfilecapabilities resolveddocumentedSYS_CHROOTonly. ServerrawSMTPassertioncorrectedstricttraceheaderprefix+exactsubmittedMIMEsuffix; server2pass. Initialproxy2pass; expanded4passbutunhandledEOFwasprinted, soaddedasync-errorassertion+exactcontinuationtest:5testsfailed2,96.836s. Fixedduplicatecontinuation, expectedEOFhandlingandTLSshutdownbound; gate5proxytests0. SASL/IDLEnewtestfailedmissingcontinuationdialogue; implementedandlatestgate6proxytests0. Oldfailedfocusedtestsession19560 mayneedfinalpoll; no liveprovider/productionadapter/Rustsystem/E2Ecompatibilityclaim.


## Task12 independent controller and Rust harness checkpoint

`python3 tests/imap/run-tests.py`: all4 commands exit0, server2/proxy8/control1/standalone1 (12 Python contracts). New missing-controller and missing-standalone reds each1; external folder control red1, then pass. New implicit TLS and after-accept COPY regressions failed2/exit1; separate Mailpit TLS service and positive-accept-only fault consumption fixed both (2pass/exit0). Retained exact commands/statuses/logs: test-results/imap-services/results.json and siblings. No live traffic.

`cargo test --locked -p nuncio-test-support --test imap_contract`: missing-module red101, then2passed0failed0ignored/exit0,28.18s. One Rust test wraps Python contracts, so counts overlap. `cargo fmt --all` exit0; `cargo clippy --locked -p nuncio-test-support --all-targets -- -D warnings` exit0. Rust artifacts under test-results/imap-contract/runs. This proves independent provider infrastructure, not production Synology engine/API/CLI. Absent-extension follow-up currently has a new failing regression; latest all-green count predates it.


Task12 mock gate `python3 test-results/task12-mocks/run-gate.py`: all9 commands exit0; both Python checks/Rustfmt/both workspaceClippy modes/imap_contract2 (wraps Python15)/Google mock25/system21/actualE2E26. No failed/ignored. Follow-up adds server-managed Sent and independent SMTP envelope evidence, unknown-token sanitization, local Docker socket pinning and timeout descendant cleanup. `python3 tests/imap/run-tests.py --output test-results/task12-mocks-followup/python`: all6 commands0,17 Python contracts (1+3+9+1+1+2), cleanup statuses retained. New reds: hidden ENABLE enabled features1; convenience-client FETCH modifier framing1; unknown IMAP token appeared in observations1; timeout child ignoring TERM survived1; auto-Sent reset remained failed1; capture Bcc prefix mismatch1. All corrected with targeted passing checks, retaining exact wire/effect assertions. Mailpit GitHub update checking now disabled; earlier tests did not establish absence of background version checks. Full six-command Rust follow-up still pending session38196; no production Synology compatibility claim.

Full Task12 mock follow-up completed: `python3 test-results/task12-mocks-followup/run-gate.py`, all6 commands exit0. Python lint/format, Rust fmt, both workspace Clippy modes, required imap_contract2 passed (includes all17 Python contracts; overlapping counts). Exact results/logs in test-results/task12-mocks-followup/results.json and test-results/imap_contract; per-test server cleanup statuses retained. No production Synology adapter or live-compatibility claim.


## Task12 account transport and API

Account configuration domain2passed0, CLIconfigunit1passed0. cargo fetch --locked --offline0 verifies new dependency lock/cache. Feature workspaceClippy0 at first account implementation. Latest direct imap_system2passed0failed0ignored/exit0 (normal target with engine test feature): independent Dovecot/Mailpit TLS auth, failed trust/credentials/secret-write, identity pin/reopen/disconnect, temporary auth outage preserves credentials, independent SMTP effects0. No mail sync/write compatibility claim. New schema13 expectations updated in calendar_schema/google_system; regression rerun pending. Full details/implementation map in SESSION-STATE.

New mock regression: hidden capabilities left duplicate spaces, which async-imap's strict parser rejected. Python spacing assertion failed1, removal fix and focused Python contract1passed0; no grammar relaxation in production. RFC3501 section9 supports exact capability separators. First system failed101; after fix1passed0. Temporary auth failure test initially Unaunthenticated versus expected Unavailable101, corrected explicit negative-proof classification: system2passed0. Actual imap_e2e first build0/test101 due test's wrong output wrapper, correction/rerun pending. No full gate, live provider or remote CI claim.


Account integration gate completed: `python3 test-results/task12-accounts/run-gate.py`, **all12 commands exit0**. Both Python checks, Rustfmt, both workspace Clippy modes; engine52,CLI/proto4, independentIMAP2 (wraps Python17), IMAPsystem2, actualIMAPE2E1, Googlesystem21, actualGoogleE2E26. All0failed/0ignored; counts overlap. Fresh feature binaries/schema13 built; production remains schema12. Exact commands/statuses and copied nested suite logs/results in test-results/task12-accounts. First actualIMAPE2E rerun build0/suite0 after correcting result wrapper. No gate currently running. Next concrete action: cover remaining bounded authentication/LOGIN edge cases, then IMAP placement and mailbox sync implementation. Live acceptance remains deferred/unverified.

Account follow-up: SMTP transport2passed/exit0 after a first-challenge temporary-auth behavioral failure (1failed/1passed101). Tests independently exercise reply framing bounds and each LOGIN challenge over local byte streams; these are units, not full provider compatibility evidence. IMAP identity/modifiedUTF7/UID guard/flags domain4passed/exit0 after missing-module red101. Implementation paths and next integration action are in SESSION-STATE. Account12-command gate predates these additions; fresh focused Clippy pending/recorded next.


Task12 transport gate all6commands0 (engine59/system2/actualE2E1). Then matching-completion regression exposed async-imap collectors accepting partial data followed by NO (behavioral101); command::complete now checks tag/status before returning typed data. Focused unit1pass/exit0 covers NO/BAD/wrongtag/disconnect/BYE/positiveOK. ResponseData private and capability notClone caused two compile101s, corrected public typed visitor. Latest test-results/task12-completions/results.json all6commands0, engine60/system2/actualE2E1 plus fmt/bothClippy. Nested logs copied. New imap_projection test added afterwards is intentionally red101 on missing domain/storage APIs; no full projection implementation or latest-all-workspace passing claim.


Task12 projection/schema14: `cargo test --locked -p nuncio-engine --test imap_projection` latest **3passed0failed0ignored/exit0**. First implementation compile101 on rusqlite u64 row extraction (same101 after missed text replacement), fixed i64. Additional nonselectable/catalog tests behavioral2failed1passed101; fixed SQL NULL obsolete-placement comparison and unknown catalog guard; all3pass0. `cargo fmt --all`0; `cargo clippy --locked -p nuncio-engine --all-targets -- -D warnings`0. Full regression gate pending. No live/remote CI claim.

Task12 network ingestion: first new independent system test returned101/InvalidArgument because IMAP sync was unimplemented. Added IMAP-only authenticated sessions, folder/EXAMINE/checkpoint discovery, bounded UID inventory and metadata/body fetch, cached-MIME delta staging, absence deletion and shared MailSync/scheduler dispatch. Initial compile101 on nonexhaustive parser attribute, then system101 on doubled extension-attribute backslash; corrected using downloaded imap-proto parser source. Diagnostic helper compile101s on unused/mutable handle corrected. First profile passed but test requested unsupported mock IDLE hiding (101); corrected to four documented hideable extensions, retaining all assertions. Focused system test1passed0 for both complete and no-MOVE/UIDPLUS/CONDSTORE/QRESYNC profiles, exact remote raw equality/flags/deletes/epochs. IDLE absence is not claimed.

New independent proxy regression proved metadata-only FETCH prematurely consumed a truncated-body fault (Python1error/exit1 after initial wrong cwd/class invocation1). Proxy now preserves truncation until a literal; focused independent test1passed/exit0. Production grammar and assertions retained.

`python3 scripts/verify.py --suite imap_e2e`: fresh build0/suite0, actual test1passed0failed0ignored. Expanded real daemon/CLI test proves stdin account auth/config, exact MIME/PDF bytes, durable draft, and three SIGKILL/restart boundaries before staging, before publication, after publication across UIDVALIDITY resets. Independent remote mailbox count/UIDVALIDITY and SMTP0 checked. Logs/results `test-results/imap_e2e`. New system empty-inventory/fault checks pending. Engine suite prior to reader changes passed63 with scoped local socket permission after sandbox-only EPERM101; latest full reader regression gate pending.


Task12 read gate initial run stopped after7 successes at new system folder test: `test-results/task12-reads/results.json`; Python checks/fmt/bothClippy/engine63/imap_contract2 passed. System4passed1failed101: decoded quoted LIST name retained escape bytes, then EXAMINE double-escaped it. Downloaded imap-proto0.16.7 `parser/core.rs::quoted` returns original escaped bytes; literals cannot be indiscriminately unescaped. Added original-frame visitor to strict command collector and narrow LIST string decoding, preserving literal backslashes. Focused quoted/literal regression1passed0; independent folder rerun pending. This failed gate did not run later Google/system/E2E entries; do not claim all-green.


Folder follow-up: first name-decoder system rerun101 because async-imap ResponseData owns a buffer block with trailing response/spare bytes; unit extended to include trailing bytes failed101. Decoder now obtains the parser-consumed prefix before decoding quoted or literal strings. Focused unit1passed0 and `cargo test --locked -p nuncio-test-support --features nuncio-engine/test-harness --test imap_system encoded_quoted -- --nocapture`1passed0. Failed gate remains `task12-reads`; nested logs copied there. New complete gate path `task12-reads-recheck` pending.


**Task12 read gate complete:** `python3 test-results/task12-reads-recheck/run-gate.py`, all11commands exit0. Pythonlint/format, Rustfmt, both workspaceClippy modes, engine64; independentIMAP2 wraps all17 Python contracts; IMAPsystem5, actualIMAPE2E1 with three SIGKILL boundaries, Googlesystem21, actualGoogleE2E26. No failures/ignored, counts overlap. Fresh feature binaries/schema14 verified; production artifacts stillTask11/schema12. Exact commands/statuses and copied nested logs/results are beside gate. No install/live-compatibility/remote CI claim. Next: explicit IMAP fetch, then durable writes.


Explicit IMAP fetch: new system regression returned101/InvalidArgument on missing support. Implemented targeted fetch with epoch checks and fetch-only promotion that cannot add unobserved messages or change a mailbox epoch/checkpoint. Focused system1passed/exit0; storage imap_projection4passed0; actual `mail fetch` added to E2E and `python3 scripts/verify.py --suite imap_e2e` freshbuild0/suite0, test1passed (includes prior3SIGKILL cases). Full follow-up gate pending; inspection identified inefficient sparse-UID inventory scanning to fix first.


Sparse UID paging: synthetic protocol test failed101 on old `UID SEARCH UID 1:4096` versus required bounded sequence intersection; replaced with `UID SEARCH 1:1 UID 1:4294967294` for one extant message. RFC3501 section6.4.8 verified directly at https://www.rfc-editor.org/rfc/rfc3501.html#section-6.4.8. Focused unit1passed0; `python3 scripts/verify.py --suite imap_system` freshbuild0/suite0,6passed0failed0ignored. Expanded unit adds expunge/duplicate/out-of-range/zero/missing-result cases. Seven-command fetch/paging gate at `test-results/task12-fetch` running; no all-green claim after latest expansion yet.


**Fetch/paging follow-up gate complete:** `python3 test-results/task12-fetch/run-gate.py`, all7commands exit0. Fmt, both workspaceClippy configurations, engine66, IMAPsystem6, actualIMAPE2E1 (including refetch and three prior SIGKILL cases), Googlesystem21. Zero failed/ignored. Exact commands/statuses and copied nested logs/results beside gate. Feature artifacts schema14; production remains schema12. No live compatibility or remote CI claim.


IMAP flags regression: `cargo test --locked -p nuncio-test-support --features nuncio-engine/test-harness --test imap_system imap_flag_intents -- --nocapture` initial compile101 on unused dependency assumption; deterministic request UUIDs fixed test compilation. Rerun101/1failed at expected missing IMAP read/star capabilities. Full local services only. Implementation pending.


Task12 flags implementation: focused `cargo test --locked -p nuncio-test-support --features nuncio-engine/test-harness --test imap_system imap_flag_intents -- --nocapture`1passed/0 after correcting attempt ordering assertions to explicit ordinals (initial101 applied-vs-uncertain because API is descending). New `unavailable_flag_reconciliation`1passed/0 proves failed reconciliation remains uncertain, no-op avoids STORE, stale UIDVALIDITY conflicts, account isolation. Independent remote counters/flags validated. `cargo test --locked -p nuncio-engine --test imap_projection imap_flag_journal`1passed/0 after initial compile101 on incorrect method names; receipt rollback/FIFO/account/replay/reopen guards. `python3 scripts/verify.py --suite imap_e2e` build0/test0,1passed (expanded with three flag-write SIGKILL cases alongside three existing read crashes). `cargo clippy --locked --workspace --all-targets -- -D warnings`0. Full flags gate pending. Source map and limits in SESSION-STATE; no schema increment.


**Task12 flags gate complete:** `python3 test-results/task12-flags/run-gate.py`, all8commands exit0. Rustfmt/both workspaceClippy modes, engine67, IMAPsystem8, actualIMAPE2E1 (six read/write SIGKILL boundaries), Googlesystem21, actualGoogleE2E26. Zero failed/ignored. Fresh feature builds; exact commands/statuses and copied nested logs/results alongside gate. Schema14, production remains Task11/schema12. No live compatibility or remote CI claim.


Next Task12 transfer test: `cargo test --locked -p nuncio-test-support --features nuncio-engine/test-harness --test imap_system archive_uses_native -- --nocapture` exit101,1failed/8filtered. Expected unsupported enqueue (`InvalidArgument`); no transfer production code added. Test independently requires native MOVE or safe COPY/UID EXPUNGE fallback, retention of unrelated Deleted UID2, exact copied raw bytes, local destination publication and request replay. Flags8-command green gate predates this new red; do not represent the entire current system suite as green. No running process.


Independent native MOVE contract added: `cd tests/imap` then `python3 -m unittest test_proxy.IndependentFaultProxy.test_native_move_lost_ack_preserves_copyuid_and_unrelated_deleted_messages -v`1passed/exit0. First invocation exit1 wrong cwd; next exit1 byte-vs-string capability assertion (imaplib capabilities are str), corrected. No mock production changes needed. Real Dovecot NO for missing target leaves fault pending; next MOVE accepted1/request2, target raw exact, source unrelated Deleted retained, COPYUID survives final-ack loss. Full follow-up gate `task12-move-mock` pending.


**Independent MOVE follow-up complete:** `python3 test-results/task12-move-mock/run-gate.py` all5commands0: Pythonlint/format, Rustfmt, feature workspaceClippy and independentIMAPcontract2. Rust contract wraps all18 Python tests; six Python module subprocesses and all owned service cleanups exited0. Exact commands/statuses, Rust logs and Python per-module logs/results copied beside gate. This verifies mock MOVE evidence, not production archive support: the new archive system regression remains red101. No process running; production transfer implementation is next.


Schema15 transfer storage groundwork: new `cargo test --locked -p nuncio-engine --test imap_projection transfer_intent` initial compile101 missing types/methods; then1passed0. `cargo clippy --locked -p nuncio-engine --all-targets -- -D warnings`0. Updated calendar_schema/Google status schema expectations15; `cargo test --locked -p nuncio-engine --test imap_projection --test calendar_schema`6+1passed0. Added post-copy Rejected guard assertion; focused1failed101 proved unsafe classification was allowed. Guard implemented; combined rerun exec76040 pending. Archive transfer dispatch/projection is not wired yet; prior gates cover schema14 only.


Transfer post-copy rejection guard combined rerun: calendar_schema1 + imap_projection6 allpassed/exit0. Expanded transfer application test compile101 on missing result/outcome; implementation first compile101 on nested function visibility and missed public re-export, corrected. Focused transfer_intent1passed/exit0 now checks atomic destination install/source removal/receipt and unchanged coverage checkpoint. `mail_projection.rs` installs decoded/raw/encrypted blobs/headers/attachments/FTS; exact stored copy proof and completed source-removal phase gate application. Transfer dispatcher remains unwired; no provider write or archive system success claim. Latest engineClippy check follows.


Schema15 Archive integration: `cargo clippy --locked -p nuncio-engine --all-targets -- -D warnings` exit0; feature workspace Clippy0 before final test additions. Focused `cargo test --locked -p nuncio-test-support --features nuncio-engine/test-harness --test imap_system archive_uses_native -- --nocapture`1passed0 for native MOVE and hidden-MOVE fallback. New already-Archive assertion initially failed101/NotFound; implemented positive-read no-op and stable attachment upserts; focused rerun1passed0. `cargo test --locked -p nuncio-engine --lib copyuid_evidence -- --nocapture`1passed0 (nine reply cases). Actual `python3 scripts/verify.py --suite imap_e2e` initially build0/suite101 twice at archive independent INBOX count2 rather than1. Root cause confirmed in test: stale entered marker reused from flag test, contrary to TestControls arm contract. Shared arm helper clears old entered/release markers; fresh run build0/E2E1passed0. Archive test includes actual daemon SIGKILL before receipt and before dispatch, exactly1 then2 remote MOVEs, exact raw download, and distinct placements with identical MIME retained. Failure log preserved `task12-archive/stale-barrier-failure.log`. New five-boundary transfer E2E pending exec25109; do not claim full current suite green.


Archive crash follow-up: first new `python3 scripts/verify.py --suite imap_e2e` build0/suite101 (1passed1failed) because operation_after_imap_copy did not exist. Added feature-gated boundary after durable COPYUID receipt, threaded stop receiver, corrected test operation get→show. Fresh rerun build0/suite0,2passed. New test loops five crash scenarios; original includes eight read/flag/archive scenarios, counts overlap. Known-copy fallback finishes without recopy or broad EXPUNGE; unknown mapping remains uncertainty over two daemon restarts with exactlyone remote COPY/MOVE. Separate `cargo test --locked -p nuncio-test-support --features nuncio-engine/test-harness --test imap_system transfer_lost_ack -- --nocapture` initial compile101 Option collection name, then runtime101 missing shutdown-before-restart; corrected test harness usage, focused1passed0 across3profiles (native earlyCOPYUID+disconnect, fallback lostCOPYACK, unsafe noMOVE/noUIDPLUS refusal). Eight-command archive gate running exec13946; fmt/bothClippy0 so far. No completed full-current-gate claim yet.


Eight-command archive gate complete: `test-results/task12-archive/results.json`, all8commands exit0. Rustfmt/both workspaceClippy, engine69, IMAPsystem10, actualIMAPE2E2, Googlesystem21, actualGoogleE2E26; zero failed/ignored. Fresh feature builds and copied nested logs/results retained; test counts overlap. Source/test schema15; production artifacts still Task11/schema12. No live compatibility or remote CI claim. No running process. Next explicit move/copy API/CLI using captured destination collection identity, then trash/restore origin metadata and SMTP.


Explicit folder move/copy now implemented after actual CLI red101 (2existingpassed/1newfailed, unsupported action exit2). Added MailAction Move/Copy and additive proto oneof fields9/10 with MailFolderChange; daemon/CLI mapping, account-scoped captured destination UUID/epoch and IMAP move_copy capability. Gmail explicitly rejects folder actions. Schema remains15; existing Archive/flag intents unchanged. Capture shares the proven transfer executor/projection. New actual E2E `imap_folder_e2e.rs` proves copy to quoted/Unicode/backslash folder, copy in same folder with distinct placements, explicit move back and same-folder no-op, exact request replay/raw bytes and remote COPY2/MOVE1. Fresh `python3 scripts/verify.py --suite imap_e2e` build0/suite0,3passed. Separate system `folder_actions`1passed0 proves foreign/empty/missing destination rejection, missing remote folder conflict and zero COPY/MOVE/STORE/EXPUNGE. Feature workspaceClippy0. Nine-command full folder gate running exec86339; poll before changing source. No full-current-gate claim until result. Next trash/restore with original folder metadata, then SMTP and Tasks13–16.


Nine-command folder gate complete: `test-results/task12-folders/results.json` all9commands0; Rustfmt/bothClippy, proto-cli4, engine69, IMAPsystem11, actualIMAPE2E3, Googlesystem21, actualGoogleE2E26. No failed/ignored. Fresh feature builds and copied nested evidence retained. Schema15 source/test; production remains Task11/schema12. No running process. Next trash/restore with original placement metadata, then SMTP.


Schema16 Trash/restore: initial `python3 scripts/verify.py --suite imap_e2e` build0/suite101 (3passed1failed) at unsupported IMAP Trash enqueue. New migration/origin capture+publication implemented; fresh build0/suite0,4passed native Trash/restore crash case plus prior transfers. Feature workspaceClippy initially101 protocol fixture missing restore_origin field; added None, rerun0. `cargo test --locked -p nuncio-test-support --features nuncio-engine/test-harness --test imap_system restore_requires -- --nocapture`1passed0 across3origin failure profiles. `... copies_into -- --nocapture`1passed0, copy/restore origin survives restart/fullsync and preserves distinct identical messages. Full9-command trash gate running exec18194; actual Trash E2E now additionally loops no-MOVE fallback. No full schema16 gate claim yet.


Nine-command Trash/restore gate complete: `test-results/task12-trash/results.json` all9commands0. Rustfmt, normal and feature workspaceClippy, proto-cli4, engine69, IMAPsystem13, actualIMAPE2E4, Googlesystem21, actualGoogleE2E26. Zero failed/ignored. Fresh feature builds and copied nested suite logs/results retained. Native and hidden-MOVE fallback Trash/restore both pass, including before-publication SIGKILL, no-op and full-resync origin retention. Source/test schema16; production artifacts still Task11/schema12. No process running; no live compatibility or remote CI claim. SMTP is next.


SMTP work in progress, newer than schema16 gate: added full system regression `imap_write_system/smtp.rs` (client-Sent policy). First compile101 unavailable hex helper; replaced with standard hash formatting. Focused rerun101 reaches durable queued send and times out because IMAP send dispatch is disabled. Test independently requires SMTP DATA count/envelope/wire hash, actual Mailpit capture, Dovecot Sent bytes/count, local Sent projection and separate smtp_accepted/sent_copy receipts. Do not weaken it or claim all-current-tests-green.

New bounded SMTP component `providers/imap/smtp/submission.rs` with typestate Session→Prepared: envelope/RCPT/DATA354 first without content, caller must commit a dispatch marker, then dot-stuffed bounded64KiB DATA write and complete final2xx/4xx/5xx classification; missing/malformed ACK unknown. Canonical CRLF/1000-byte lines, payload bounds, SMTPUTF8 checks for every parsed MIME header including nested messages, 8BITMIME for body. Existing auth now returns a Session internally and probe still reports capability tuple without sending. New submission_tests initially compile101 missing types, implementation unit run pending exec16098. Production caller/journal integration not yet written; normal build may report unused submission methods until wired. Full product SMTP system remains intentionally red.

Next immediately: poll16098/fix component failures; implement schema17 immutable SMTP/Sent intent + durable phases and worker consumer. Preserve separate SMTP accepted and APPEND proof. Prepared(before DATA bytes) can retry after crash; Started without SMTP receipt stays unknown; accepted SMTP never repeats for Sent failure; SentStarted without APPEND proof never blindly repeats; known APPENDUID permits positive target observation/publication. Add negative-reply evidence to reset only proven unaccepted SMTP, and independently test those invariants. Server AutoSent and broader E2E remain required, not waived.

Primary references read September11: RFC5321 sections4.2.5/4.5.2 (2xx after terminator accepts responsibility;4xx/5xx prohibit subsequent delivery; add one dot at each line start), RFC6531 section3.2 (SMTPUTF8 required for internationalized envelope or headers at any MIME depth), RFC4315 APPENDUID/UIDNOTSTICKY. URLs https://www.rfc-editor.org/rfc/rfc5321.html , https://www.rfc-editor.org/rfc/rfc6531.html , https://www.rfc-editor.org/rfc/rfc4315.html . Missing APPENDUID is not acceptance proof of identity; no guessed repeated copies.


Remaining Calendar-role check: current official Google documentation includes
writerWithoutPrivateAccess for non-private event writes, while private events are
visible only as busy blocks and cannot be modified under that role. Production
CalendarAction::provider_patch and the independent mock write policy currently
accept owner/writer only. This explicit compatibility gap remains pending after
the resource checkpoint; tests must independently enforce private-event limits.
Sources: [CalendarList](https://developers.google.com/workspace/calendar/api/v3/reference/calendarList)
and [calendar sharing](https://developers.google.com/workspace/calendar/api/concepts/sharing).
