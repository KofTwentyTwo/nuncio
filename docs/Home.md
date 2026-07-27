# Nuncio Wiki

**A local-first mail, calendar, and contacts engine in Rust.**

> **Status: pre-alpha, under active reconstruction (2026-07-26).** Nuncio began as
> an AI-generated proof of concept that a forensic assessment found to be largely
> non-functional, with substantially fabricated documentation. It is being rebuilt
> engine-first. Older wiki pages claiming completed features, benchmarks, or
> "100% parity" were removed. **Nothing here should be read as a shipped feature
> unless the roadmap says so.**

## Start here

- **[Roadmap](ROADMAP)** — the authoritative plan and target architecture.
- **[Backlog](BACKLOG)** — engineering-ready Phase 0 / Phase 1 stories.
- **[Architecture](ARCHITECTURE)** — technical architecture (target + current state).
- **[ADR 0001 — Engine-first gRPC architecture](adr/0001-engine-first-grpc-architecture)** — the governing decision.

## Reference (real subsystems, with status caveats)

- **[NSQL Filter Language](NSQL-Filter-Language-Specification)** — the declarative
  filter language (parser/validator/engine are real; actions and scoping are being
  finished in Phase 3).
- **[Database Self-Healing](Database-Corruption-Self-Healing)** — corruption
  detection and recovery (real, with two defects being fixed in Phase 1).

## The short version

The daemon (`nunciod`) is the product: it owns all state, credentials, and
protocol logic, and publishes a versioned gRPC API over loopback. User interfaces
are separate native clients that consume that API. Parity across clients is a
property of the published contract, not a matter of discipline.
