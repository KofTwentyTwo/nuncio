# Nuncio rebuild

Independent Rust workspace for the approved Google-first engine and CLI rebuild.
It includes SQLCipher storage, authenticated local gRPC, Gmail/Calendar reads and
writes, IMAP folder/flag operations, SMTP with separate delivery/Sent-copy receipts,
encrypted backup/restore, migration, repair, and resource budgets. Software
checkpoint `6ff9bb9` passed the complete offline gate, all ten Linux/macOS CI jobs,
and repeatable local Apple Silicon packaging; documentation checkpoint `7390e77`
also passed all ten CI jobs. Current account-management additions use schema 23
and seven additive account RPCs (49 total). The current full offline gate passed
all 34 commands: 322 tests in each workspace configuration, zero failed/ignored,
22 script regressions, and all 411 captured source hashes unchanged. Fresh
packaging, current-source hosted CI, and the first eligible testing download
remain pending; [the report](docs/IMPLEMENTATION-REPORT.md) records the evidence.

- [Run locally](docs/RUNNING.md) and [verify/use local packages](docs/PACKAGING.md)
- [Account management](docs/ACCOUNT-MANAGEMENT.md)
- [Download a testing build without compiling](docs/TESTING-INSTALL.md) — first eligible CI archive pending
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
no new testing download, public API distribution, or release is claimed available
until its respective checks and delivery steps complete.
