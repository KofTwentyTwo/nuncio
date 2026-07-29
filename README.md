# Nuncio

**A local-first mail, calendar, and contacts engine written in Rust.**

> **Status: pre-alpha, under active reconstruction (2026-07-26).**
> Nuncio began as an AI-generated proof of concept. A forensic assessment found
> that while several library-layer components are genuinely well-built, the
> end-to-end product did not work and much of the prior documentation was
> inaccurate. The project is being rebuilt engine-first. **It is not usable yet,
> and published pre-1.0 releases are non-functional — do not install them.**
> See the **[live roadmap](https://koftwentytwo.github.io/nuncio/roadmap/)**
> (source: [`docs/ROADMAP.md`](docs/ROADMAP.md)) for the plan and
> [`docs/adr/0001-engine-first-grpc-architecture.md`](docs/adr/0001-engine-first-grpc-architecture.md)
> for the architecture decision.

## What Nuncio is

A background daemon (`nunciod`) owns all state, credentials, and protocol logic
(IMAP/SMTP, then JMAP, CalDAV, CardDAV), and **publishes a versioned gRPC API over
loopback**. User interfaces are separate native client projects that consume that
API — they hold no business logic and no data of their own. This makes feature
parity across clients a property of the published contract rather than a matter of
discipline.

The only client in this repository is `nuncio-cli`, which serves as the reference
API consumer and the driver for end-to-end tests. Native GUIs (macOS/Windows), a
TUI, and an MCP bridge will live in separate repositories once the engine and its
API are solid.

## Workspace layout

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

## Build & test

```bash
cargo build --workspace          # build everything
cargo check-all                  # clippy, all targets, warnings-as-errors (the gate)
cargo test-all                   # run the whole test suite
cargo verify                     # fmt --check + check-all + test-all
cargo test -p nuncio-store       # test a single crate
```

Cargo aliases are defined in [`.cargo/config.toml`](.cargo/config.toml).

## Documentation

- **[Roadmap — live rendered page](https://koftwentytwo.github.io/nuncio/roadmap/)** — the visual plan to a client-ready backend (GitHub Pages)
- [`docs/ROADMAP.md`](docs/ROADMAP.md) — authoritative roadmap and target architecture (source for the page above)
- [`docs/STORY-WORKFLOW.md`](docs/STORY-WORKFLOW.md) — how contributors execute a roadmap story (gates, patterns, review flow)
- [`docs/HANDOFF.md`](docs/HANDOFF.md) — development handoff: current state, branches, and the M1–M7 story index
- [`docs/BACKLOG.md`](docs/BACKLOG.md) — engineering-ready Phase 0 / Phase 1 stories
- [`docs/adr/`](docs/adr/) — architecture decision records
- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — technical architecture

## License

MIT OR Apache-2.0.
