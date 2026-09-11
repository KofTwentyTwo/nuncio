# Nuncio rebuild

Independent Rust workspace for the approved engine-and-CLI rebuild. The current implementation includes SQLCipher storage, authenticated local gRPC, the reference CLI, independent Google and MailPlus test services, Google mail/calendar reads and writes, and IMAP account setup, mail reads, and durable read/star flag changes. Folder transfers, SMTP sending, maintenance, and final artifacts remain in progress.

- [Run locally](docs/RUNNING.md)
- [Offline testing](docs/TESTING.md)
- [Mock Google service](docs/MOCK-GOOGLE.md)
- [Session state](docs/SESSION-STATE.md), [remaining work](docs/TODO.md), and [verification evidence](docs/VERIFICATION.md)
- [Approved specification](../docs/superpowers/specs/2026-09-10-google-first-rebuild.md) and [implementation plan](../docs/superpowers/plans/2026-09-10-google-first-rebuild.md)

All automated acceptance uses synthetic local services. No live-provider compatibility or remote CI execution is claimed. Native apps are outside this workspace's implementation goal.
