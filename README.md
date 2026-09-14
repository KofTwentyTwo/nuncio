# Nuncio

**A local-first mail and calendar engine written in Rust.**

> **Status: pre-alpha; active implementation is in [`rebuild/`](rebuild/README.md)
> (September 14, 2026).** Latest verified testing download: `bb70f95`. It includes
> readable CLI output, complete help, detailed startup/activity logs, guided
> account management, permanent installer paths and the Rustls security patch.
> All ten hosted rebuild jobs, seven security jobs and actual public fresh/update
> checks passed. The same source has two identical clean local archives and a
> full 35-command gate with 359 tests per workspace. Google registration and named
> live Google/MailPlus/native-keystore acceptance remain pending. See the
> [report](rebuild/docs/IMPLEMENTATION-REPORT.md) and [current state](rebuild/docs/SESSION-STATE.md).

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

The permanent commands are in `~/.local/opt/nuncio-testing/bin/`. Rerun the
installer to update them; each verified update keeps this path. Start the daemon
with `~/.local/opt/nuncio-testing/bin/nunciod --profile laptop-qa`, then add an
account using `~/.local/opt/nuncio-testing/bin/nuncio-cli --profile laptop-qa account add`.
The current source reports startup stages and running activity at **info** level
(`--log-level debug` adds detail), and readable CLI output is now the default;
scripts use `--json`. Rustls 0.23.45 addresses RUSTSEC-2026-0285 across all three
lockfiles. The downloaded package also includes bounded IMAP catch-up, granular credential/recovery timings and safe CLI typo suggestions. The laptop startup pause still needs a retest.
Public fresh/update qualification passed at bb70f95. [CLI usability audit](rebuild/docs/CLI-USABILITY-AUDIT.md)
and [logging options](rebuild/docs/RUNNING.md#running-logs).
[Guided setup](rebuild/docs/ACCOUNT-SETUP.md) uses direct prompts and hidden passwords.
Follow the [Google setup walkthrough](rebuild/docs/GOOGLE-SETUP.md) to register
the app once and connect using a private local client file. Shared registration
has not yet been bundled into testing builds.

For Apple Silicon laptop testing, the [testing installer](rebuild/docs/TESTING-INSTALL.md)
downloads a verified successful CI package without compiling locally. It
activates each build at the same testing path and retains older versions for
recovery. Actual installation and update passed at `bb70f95`. It starts no service. No public release or live-provider readiness is implied.

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
