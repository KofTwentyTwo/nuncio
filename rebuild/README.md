# Nuncio rebuild

Independent Rust workspace for the approved Google-first engine and CLI rebuild.
It includes SQLCipher storage, authenticated local gRPC, Gmail/Calendar reads and
writes, IMAP folder/flag operations, SMTP with separate delivery/Sent-copy receipts,
encrypted backup/restore, migration, repair, and resource budgets. Current schema-23 account management uses 49 authenticated RPCs.
Guided `account add` includes hidden passwords, optional advanced MailPlus
settings and bundled Google registration support. The latest verified testing
build is `f14b02a`, with individual-account help and permanent installer paths.
All ten hosted rebuild jobs, seven security jobs and actual public installation/
update checks passed. The retained reproducible local candidate is 82a92a0;
its full guided-setup gate passed 328 tests per workspace configuration.
[The report](docs/IMPLEMENTATION-REPORT.md) distinguishes these source-specific
results and the pending live/native acceptance.

Install the latest Apple Silicon testing build without cloning or compiling:

```sh
curl -fsSL https://raw.githubusercontent.com/KofTwentyTwo/nuncio/feature/nuncio-google-first-rebuild/install-testing.sh | bash
```

Requires macOS 15+, Python 3.11+ and authenticated GitHub CLI 2.100+;
[details and custom prefix](docs/TESTING-INSTALL.md).

The permanent commands are in `~/.local/opt/nuncio-testing/bin/`. Rerun the
installer to update them; each verified update keeps this path. Start the daemon
with `~/.local/opt/nuncio-testing/bin/nunciod --profile laptop-qa`, then add an
account using `~/.local/opt/nuncio-testing/bin/nuncio-cli --profile laptop-qa account add`.
The daemon prints running activity at **info** level; add `--log-level debug`
for more detail. [Logging options](docs/RUNNING.md#running-logs).
[Guided setup](docs/ACCOUNT-SETUP.md) uses direct prompts and hidden passwords.
Follow the [Google setup walkthrough](docs/GOOGLE-SETUP.md) to register the app
once and connect using a private local client file. Shared registration has not
yet been bundled into testing builds.

- [Run locally](docs/RUNNING.md) and [verify/use local packages](docs/PACKAGING.md)
- [Account management](docs/ACCOUNT-MANAGEMENT.md)
- [Download a testing build without compiling](docs/TESTING-INSTALL.md) — actual CI download/install verified
- [Offline testing](docs/TESTING.md)
- [Mock Google service](docs/MOCK-GOOGLE.md)
- [API contract](docs/API.md), [API publication proposal](docs/API-PUBLICATION-PLAN.md), and [task progress estimates](docs/PROGRESS.md)
- [Development security scanning](docs/SECURITY-CI.md) and [open PR dispositions](docs/OPEN-PR-REPORT.md)
- [Proposed milestones after engine/CLI completion](docs/POST-ENGINE-ROADMAP.md)
- [Implementation report](docs/IMPLEMENTATION-REPORT.md)
- [Requirement evidence matrix](docs/REQUIREMENTS.md) and [compatibility limits](docs/COMPATIBILITY.md)
- [Session state](docs/SESSION-STATE.md), [remaining work](docs/TODO.md), and [verification evidence](docs/VERIFICATION.md)
- [Approved specification](../docs/superpowers/specs/2026-09-10-google-first-rebuild.md) and [implementation plan](../docs/superpowers/plans/2026-09-10-google-first-rebuild.md)

All automated provider acceptance uses synthetic local services. Live Google,
Synology MailPlus, and native-keystore acceptance remain deferred and unverified;
native apps are excluded. Earlier failures and later corrections remain in the
verification history. The old archive does not qualify new account-management
source. The testing installer and API publication proposal are repository work;
current source-specific download verification is in the report. Public API distribution and a formal
release remain separate future work.
