# Offline verification

Run from the isolated worktree root:

```sh
python3 rebuild/scripts/verify.py --suite google_mock_contract
python3 rebuild/scripts/verify.py --suite google_system
python3 rebuild/scripts/verify.py --suite google_e2e
python3 rebuild/scripts/verify.py --suite imap_contract
python3 rebuild/scripts/verify.py --suite imap_system
python3 rebuild/scripts/verify.py --suite imap_e2e
python3 rebuild/scripts/verify.py --suite multi_engine_system
python3 rebuild/scripts/verify.py --suite security_system
python3 rebuild/scripts/verify.py --suite security_e2e
python3 rebuild/scripts/verify.py --suite resource_system
python3 rebuild/scripts/verify.py --suite resource_e2e
python3 rebuild/scripts/verify.py --suite release_isolation
python3 rebuild/scripts/verify.py --all
```

The runner builds actual daemon/CLI test binaries under `rebuild/target/test-harness`; release checks build selected production packages without test features under `rebuild/target/production`. Every subprocess result, command, and exit status is recorded in `rebuild/test-results/<suite>/`. Required missing suites fail with exit 2. Named suites cover independent Google and IMAP/SMTP contracts, authenticated system tests, actual CLI reads/writes/crash recovery, migration/backup/repair, three-engine convergence, security and resource workloads. Resource instrumentation and the prior full Task14 gate have passed; full34-command Task15 integration gate58495 passed; local repeatable packaging has passed and hosted CI remains pending; consult SESSION-STATE and VERIFICATION for exact evidence. Never skip required suites to claim full verification.

The independent mock's normal dependencies exclude engine/proto. Its HTTP conformance tests establish provider behavior separately. System tests compose the real engine, SQLCipher, provider HTTP, authenticated RPC, and mock. E2E tests execute the real daemon and CLI; they wait for a private readiness file and authenticated health, use ephemeral ports, and force-kill/restart while preserving profile and remote state. Remote send/copy/notification assertions must use independent provider controls, not the local database or CLI alone.

Child commands clear inherited environments; test HTTP clients disable ambient proxies. Synthetic keystores and profiles are temporary and never use the normal OS keychain. Bounded command/readiness deadlines prevent hanging tests. Failed E2E runs retain profile, readiness files, and redacted logs; successful runs clean their temporary artifacts. Synthetic secret files in retained failures remain private test data and must not be published. Logs redact bearer credentials, mock tokens, and hex key canaries. Test output may include synthetic email content.

With the explicit `test-harness` feature, the daemon accepts `--test-secrets-file FILE --test-config FILE`. Test configuration requires a numeric loopback HTTP `google_base_url` and a synthetic secret store. Optional settings are `request_timeout_ms`, `poll_interval_ms`, `max_payload_bytes`, `now_unix_ms`, and `barriers_directory`. Unknown fields, remote URLs, invalid bounds, and a production keystore fail before profile initialization. The clock can be fixed without pausing Tokio around SQLite. To arm an operation checkpoint, clear old markers, create `NAME.arm` in the private barrier directory, wait for `NAME.entered`, then create `NAME.release`. Checkpoints time out or cancel on shutdown. Operation and sync checkpoints are implemented in the provider/journal paths.

Production binaries reject test flags and `NUNCIO_TEST_*` settings before profile creation. The runner's `NUNCIO_E2E_*` and `NUNCIO_RELEASE_*` variables identify built binaries; they are not forwarded to children. Independent pinned IMAP/SMTP container services and network-level egress denial are implemented; full integrated local verification passed. Current passing local tests do not establish that CI egress is enforced, remote CI ran, or either live provider is compatible.

Focused manual-sync/fault harnesses explicitly set the test-only background_sync setting false, keeping fault ordinals and crash barriers deterministic. E2eHarness::start_with_polling(seed, Some(interval_ms)) enables actual scheduling at a short interval for scheduler tests. TestConfig defaults background_sync true; production has no flag to disable it. The injected epoch now advances using elapsed monotonic time for sync timestamps, so retry deadlines can expire normally. All provider state remains independently controlled by the mock.


Scheduling system and subprocess cases explicitly enable short polling; focused synchronization tests disable background polling through feature-only TestConfig so manual fault boundaries stay deterministic. The synthetic epoch advances across restarts. A bounded `clock-offset-ms` file in the configured private barriers directory advances the synthetic wall clock (maximum 366 days); the wake test advances the independent mock clock separately. This exercises catch-up without pausing Tokio around SQLite. Production builds expose none of these controls.

Mock observations attribute known authorization codes/access/refresh credentials using a private fingerprint-to-account ledger, including expired/revoked attempts. Authentication still uses the separate live grant state. Snapshots reveal account/method/path/count only, never credentials or body/query values. Raw HTTP conformance checks validate this separation before system tests use it to assert that paused accounts stop all requests. Retry faults support either numeric seconds or an exact HTTP-date header.

Security subprocess tests inspect original daemon/CLI logs before harness redaction, scan populated encrypted storage/WAL/backup/temp outputs, and independently test ordinary-SQLite and wrong-key rejection. Resource workloads report10,000-message ingestion/pagination timings and owned-daemon RSS while repeatedly fetching/downloading16MiB attachments. A finite post-warm-up growth allowance catches retention regressions; it is not a universal memory/speed guarantee. Only owned test daemon PIDs are sampled. The external Python sampler reads numeric macOS libproc or Linux procfs RSS/thread counts, avoiding privileged `ps` under Seatbelt. Sampling runs on a blocking worker so it cannot delay the current-thread mock server. Real-allocation and exited-process regressions verify the observer independently on both platforms. The large-attachment fixture uses the existing production 30-second HTTP deadline, with an independent 1.25-second response delay regression. Fast fault tests retain their one-second synthetic deadline. Payload sizes, eight transfer cycles, remote-effect assertions and the 128 MiB post-warm-up RSS growth bound are unchanged. Resource and security tests never authorize live-provider access.

For a focused macOS allocation investigation, run
`REBUILD_ALLOCATION_PROFILES=1 python3 rebuild/scripts/verify.py --suite resource_e2e`.
This optional test-only setting records content-free `heap` and `vmmap` summaries
for the owned daemon after attachment cycles2 and8, with30-second tool deadlines.
It is never forwarded to production processes and does not change assertions.
Use normal runs for timing evidence because profiler observation adds overhead.
Resource JSON is written before the RSS assertion so failed runs retain samples,
thread counts and database/WAL sizes. Passing a diagnostic rerun does not erase an
earlier failure; preserve the failed gate and establish its cause.


Resource status has an actual daemon/CLI regression that holds two Google requests,
observes a third account-sequenced job, cancels it before provider access, and checks
counter reset while cached messages survive restart. A separate mixed-provider
system case holds Google and Dovecot exchanges, requires another Google identity
request to wait, then compares independent unchanged mailbox/effect observations.
The10,000-message workload compares stored-page counters with independently counted
HTTP pages. IMAP/SMTP subprocess checks retain resource snapshots alongside their
existing exact delivery/copy/MIME crash evidence. Current verification status and
any failed attempts remain in VERIFICATION; this description is not a full-gate claim.

## Egress enforcement and CI groups

Fetch locked Rust dependencies (both workspaces), pinned server images and the
RustSec advisory database before running offline checks. Six CI jobs call the same
runner with `--job`: rebuild-lint, rebuild-mock-contract, rebuild-system, rebuild-e2e,
rebuild-imap, rebuild-release-check. `--all` retains both workspace test configurations
and all18 named suites, and includes the independent Python mail-service suite,
script regressions, dependency review and separate generated-client checks.
`python3 scripts/egress.py --evidence test-results/full-egress.json -- python3 scripts/verify.py --all`
is the integrated macOS command from rebuild/. Each result preserves command, log
and exit status. Script tests inject an actual failing subprocess into each job
and require early failure; required missing suites also fail closed.

The macOS wrapper uses a process/child sandbox permitting loopback and Unix
sockets while denying external outbound traffic. Hosted Linux runners install
owned per-user IPv4/IPv6 reject chains and temporary TEST-NET probe routes, then
remove only those objects. Parent and child loopback checks must pass; external
probes must fail and Linux packet counters independently confirm both rejections.
Local Linux validation uses tests/egress/Dockerfile and check.py inside a disposable
network-disabled namespace; it does not prove a hosted CI run occurred.

Independent Dovecot/Mailpit containers attach only to one internal network. Docker29
does not publish ports for that topology, so a pinned, unprivileged HAProxy relay
provides six fixed TCP forwards over a separate ingress network. The relay has no
IP forwarding and no dynamic/open-proxy destinations. TLS and protocol bytes pass
through unchanged. Startup fails if provider isolation, relay topology, forwarding
controls or numeric-loopback publishing differ. A separate Docker-metadata test
checks those properties. Only fixture-local connections reach the providers.

Python fixture style uses Ruff0.16.7 (tests/imap/ruff.toml); install tooling only
into an isolated test environment or ephemeral CI runner. Never upload retained
synthetic secret stores/profiles to CI artifacts: workflow retention includes
command logs/results and numeric egress evidence only. Passing local checks does
not establish live-provider compatibility or remote CI success.

For an isolated local Ruff installation, pass its absolute executable path in
`REBUILD_RUFF`; no normal-environment installation is necessary. Set
`NUNCIO_ADVISORY_DB` to an absolute path containing the fetched cargo-deny database
when using a non-default location. The runner preserves these two tool settings
and strips inherited provider/test-account settings before composing synthetic
fixtures. Review command results.json and the egress envelope together: intentional
exit17 child tests must pass their enclosing regression, while any failed required
suite stops the runner with failure.
