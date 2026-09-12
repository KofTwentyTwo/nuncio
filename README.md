# Nuncio

**A local-first mail and calendar engine written in Rust.**

> **Status: pre-alpha; active implementation is in [`rebuild/`](rebuild/README.md)
> (September 12, 2026).** The current engine/API/CLI, account-management additions,
> and testing installer passed all 34 offline gate commands, with 322 tests in
> each workspace configuration and no failures or ignored tests. Signed/pushed
> `164b021` also passed fresh repeatable local packaging. All ten current-source rebuild CI jobs, seven security jobs
> and the actual testing-download/temporary installation passed.
> Earlier schema-22 checkpoints passed Linux/macOS CI. Live Google,
> Synology MailPlus, and native-keystore acceptance remain unverified.
> See the [implementation report](rebuild/docs/IMPLEMENTATION-REPORT.md) for the
> exact evidence and [current work](rebuild/docs/SESSION-STATE.md) for progress.

## What Nuncio is

A daemon (`nunciod`) owns encrypted SQLCipher storage, credentials, synchronization,
and durable operations. The active rebuild supports Google Gmail/Calendar and
IMAP/SMTP mail through an authenticated `nuncio.v2` gRPC API on numeric loopback.
The reference `nuncio-cli` consumes that API independently of engine/storage code.
Native applications are outside this workspace's implementation scope. Contacts,
JMAP, and CalDAV/CardDAV remain outside the active rebuild's approved provider set.

Install the latest Apple Silicon testing build without cloning or compiling:

```sh
curl -fsSL https://raw.githubusercontent.com/KofTwentyTwo/nuncio/feature/nuncio-google-first-rebuild/install-testing.sh | bash
```

Requires macOS 15+, Python 3.11+ and authenticated GitHub CLI 2.100+;
[details and custom prefix](rebuild/docs/TESTING-INSTALL.md).

For Apple Silicon laptop testing, the [testing installer](rebuild/docs/TESTING-INSTALL.md)
downloads an eligible verified CI package without compiling locally. The first real download/temporary installation is verified at `164b021`; it
selects a fully successful retained testing-branch run. It installs into a separate versioned prefix without starting a
service. No public release or live-provider readiness is implied.

The [proposed post-engine roadmap](rebuild/docs/POST-ENGINE-ROADMAP.md) starts
after engine/CLI acceptance and sequences a native Mac alpha, daily mail,
daily calendar, and a dependable personal release. It is future planning;
native implementation is outside the current goal.

## Active workspace and verification

Use the independent [`rebuild/Cargo.toml`](rebuild/Cargo.toml) workspace. Its
[README](rebuild/README.md), [running guide](rebuild/docs/RUNNING.md),
[account-management guide](rebuild/docs/ACCOUNT-MANAGEMENT.md), and
[API contract](rebuild/docs/API.md) describe the current source.

```sh
cd rebuild
cargo build --locked -p nunciod -p nuncio-cli
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
```

These are basic Rust checks. The complete [offline verification](rebuild/docs/TESTING.md)
also runs test-feature builds, independent provider services, real daemon/CLI
subprocess tests, security/resource checks, and production-package checks.

## Original workspace

The root Cargo workspace and original roadmap remain preserved separately.
The July 26 assessment found the original proof of concept did not work end to
end and older documentation overstated its capabilities. Its library layout below
describes that original code, not the independent rebuild or a finished product.

Engine libraries composed by the daemon:

| Crate | Role |
| :--- | :--- |
| `crates/nuncio-core` | Domain models, event bus, IPC/API layer, cross-cutting services |
| `crates/nuncio-store` | SQLite (WAL) persistence, FTS search, ciphers, corruption recovery, keyring vault |
| `crates/nuncio-mail` | IMAP, JMAP, SMTP protocol engines and MIME handling |
| `crates/nuncio-cal` | iCalendar, CalDAV, CardDAV, recurrence, scheduling |
| `crates/nuncio-contacts` | Contacts store, CardDAV sync, vCard |
| `crates/nuncio-filter` | NSQL declarative filter language (parser, validator, engine) |
| `crates/nunciod` | The daemon binary — owns state and serves the API |
| `crates/nuncio-cli` | Reference API client + E2E driver |

The former `nuncio-tui`, `nuncio-gui`, and `nuncio-mcp` shells have been moved to
`_reference/` (out of the workspace) and will be rebuilt as separate client
repositories (roadmap Phase 5).

## Original-workspace build and test

```bash
cargo build --workspace          # build everything
cargo check-all                  # clippy, all targets, warnings-as-errors (the gate)
cargo test-all                   # run the whole test suite
cargo test -p nuncio-store       # test a single crate
```

There is no `cargo verify` alias (Cargo aliases can't chain subcommands). Run the
full local gate as three separate commands — `cargo fmt --all -- --check`,
`cargo check-all`, `cargo test-all` — which is exactly what the pre-commit hook runs.

Cargo aliases are defined in [`.cargo/config.toml`](.cargo/config.toml).

## Original architecture and planning references

- **[Roadmap — live rendered page](https://koftwentytwo.github.io/nuncio/roadmap/)** — the visual plan to a client-ready backend (GitHub Pages)
- [`docs/ROADMAP.md`](docs/ROADMAP.md) — original-workspace roadmap (source for the page above); the rebuild follows its [approved specification](docs/superpowers/specs/2026-09-10-google-first-rebuild.md)
- [`docs/STORY-WORKFLOW.md`](docs/STORY-WORKFLOW.md) — how contributors execute a roadmap story (gates, patterns, review flow)
- [`docs/HANDOFF.md`](docs/HANDOFF.md) — development handoff: current state, branches, and the M1–M7 story index
- [`docs/BACKLOG.md`](docs/BACKLOG.md) — engineering-ready Phase 0 / Phase 1 stories
- [`docs/adr/`](docs/adr/) — architecture decision records
- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — technical architecture

## License

MIT OR Apache-2.0.
