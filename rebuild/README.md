# Nuncio rebuild

Independent Rust workspace for the approved Google-first engine and CLI rebuild.
It includes SQLCipher storage, authenticated local gRPC, Gmail/Calendar reads and
writes, IMAP folder/flag operations, SMTP with separate delivery/Sent-copy receipts,
encrypted backup/restore, migration, repair, and resource budgets. Current schema-23 account management uses 49 authenticated RPCs.
Guided `account add` includes hidden passwords, optional advanced MailPlus
settings and bundled Google registration support. The full 35-command offline
gate passed: 328 tests per workspace configuration, zero failed/ignored. Signed/
pushed `82a92a0` passed reproducible Apple Silicon packaging and the Linux/macOS
scheduler regression. All ten hosted rebuild jobs, seven security jobs and the actual public testing
installation passed; [the report](docs/IMPLEMENTATION-REPORT.md) records exact evidence.

Install the latest Apple Silicon testing build without cloning or compiling:

```sh
curl -fsSL https://raw.githubusercontent.com/KofTwentyTwo/nuncio/feature/nuncio-google-first-rebuild/install-testing.sh | bash
```

Requires macOS 15+, Python 3.11+ and authenticated GitHub CLI 2.100+;
[details and custom prefix](docs/TESTING-INSTALL.md).

Add an account with `./bin/nuncio-cli --profile laptop-qa account add`.
[Guided setup](docs/ACCOUNT-SETUP.md) uses direct prompts and hidden passwords.
Google sign-in requires the one-time Nuncio app registration, which has not yet been created.

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
the testing download is verified at `82a92a0`. Public API distribution and a formal
release remain separate future work.
