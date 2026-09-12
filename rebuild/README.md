# Nuncio rebuild

Independent Rust workspace for the approved engine-and-CLI rebuild. The implementation includes SQLCipher storage, authenticated local gRPC, independent Google and MailPlus test services, Google mail/calendar reads and writes, IMAP folder/flag operations, SMTP with separate delivery/Sent-copy receipts, encrypted backup/restore, migration and projection repair. Resource budgets and process-local status are exposed through the engine, API and CLI. The v2 contract and first local package are verified; the full34-command integrated offline gate passed; repeatable local packages passed; hosted CI and provider acceptance remain pending.

- [Run locally](docs/RUNNING.md) and [verify/use local packages](docs/PACKAGING.md)
- [Offline testing](docs/TESTING.md)
- [Mock Google service](docs/MOCK-GOOGLE.md)
- [API contract](docs/API.md) and [task progress estimates](docs/PROGRESS.md)
- [Requirement evidence matrix](docs/REQUIREMENTS.md) and [compatibility limits](docs/COMPATIBILITY.md)
- [Session state](docs/SESSION-STATE.md), [remaining work](docs/TODO.md), and [verification evidence](docs/VERIFICATION.md)
- [Approved specification](../docs/superpowers/specs/2026-09-10-google-first-rebuild.md) and [implementation plan](../docs/superpowers/plans/2026-09-10-google-first-rebuild.md)

All automated acceptance uses synthetic local services. No live-provider compatibility or remote CI execution is claimed. Native apps are outside this workspace's implementation goal.
