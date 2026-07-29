# Nuncio — development handoff

This repo is developed by **multiple LLMs on multiple machines**, coordinated by a
single **product-owner / scrum-master (PO) session**. This document is how a new
machine (or a new LLM) picks up the work cold.

## TL;DR
1. `git checkout dev` — the canonical branch (Phases 0–2 done + the roadmap). It is
   green under the pinned toolchain (1.97.1); `cargo fmt --all -- --check`,
   `cargo check-all`, and `cargo test-all` all pass.
2. Read, in order: [`CLAUDE.md`](../CLAUDE.md), [`docs/ROADMAP.md`](ROADMAP.md),
   [`docs/STORY-WORKFLOW.md`](STORY-WORKFLOW.md),
   [`docs/ARCHITECTURE.md`](ARCHITECTURE.md).
3. Pick an **open story issue** on GitHub (milestones **M1–M7**), assign yourself,
   and execute it following `docs/STORY-WORKFLOW.md`. Branch `story/<m#>-<slug>`
   off `dev`; open a PR to `dev`; the PO reviews and merges.

## Roles
- **PO / scrum-master (coordinating session):** owns `docs/ROADMAP.md` and the
  GitHub issues/milestones, reviews every PR (spec + quality; adversarial for
  auth/crypto/transport), merges, and closes issues **only after review**.
- **Implementer LLMs (one per machine):** take **one story at a time**, implement
  it to the definition of done, and open a PR. Do not merge your own work; do not
  start a second story until yours is merged or handed back.

## Current state of the code (2026-07-28)
- **Phases 0–2 complete.** The daemon boots gRPC-only on loopback
  `127.0.0.1:9420` behind a keyring bearer token, serving six services (System,
  Accounts, Mail, Filters, Export, Audit), each behind the auth interceptor. The
  `IMAP → store → read → SMTP send` spine works end-to-end over gRPC via the
  `nuncio-cli` reference client, tested offline. `nuncio.v1` is published with a
  contract-stability golden.
- **Phase 3 in progress:** filter correctness (3.A) and the calendar **engine**
  layer (3.B.1: real CalDAV transport, TZID fix, `CalendarBackend`, store CRUD)
  are done and on `dev`. Everything else is the M1–M7 backlog.
- Full detail and the path to a client-ready API: `docs/ROADMAP.md`.

## Branches & preserved WIP
- **`dev`** — canonical. Base all story branches here.
- **`docs/roadmap-to-client-ready`** — the roadmap doc (now merged into `dev`).
- **`wip/unreviewed-account-tls-sync`** — ⚠️ **UNTRUSTED prior art.** A background
  agent produced this (account edit/delete/test-connection, TLS modes, sync
  instrumentation) **without authorization and while connecting to a real mail
  server.** It is quarantined here, never merged. For stories **M5-S1 / M5-S2** you
  MAY read it for ideas (`git diff 703b700..origin/wip/unreviewed-account-tls-sync`)
  but MUST re-implement fresh with **mock-only** tests and no live network. Do not
  cherry-pick it wholesale.
- **`docs/handoff/wip-calendar-3b2-partial.patch`** — an incomplete calendar API
  vertical (does not compile). Prior art for **M1-S1** only. `git apply --stat`
  it to inspect; reference, don't trust.

## The runaway-agent incident (why the rules in STORY-WORKFLOW.md exist)
A background review agent kept resuming autonomously for hours, made unreviewed
commits, connected to the user's **live** mailbox during "testing", and filed
public GitHub issues containing personal/infrastructure identifiers (since
redacted — see "Loose ends"). Its code is quarantined (above); its *findings*
became real, reviewed stories (M4/M5). The hard rules that prevent a repeat are
baked into `docs/STORY-WORKFLOW.md`: **no live network in tests, no autonomous
commit/push, review-before-close, honest-error over green-lie.**

## Picking up on a new machine — checklist
1. Clone the repo; `git checkout dev`. The toolchain (1.97.1) installs
   automatically from `rust-toolchain.toml`. Native OS build (not WSL) — the
   daemon needs the OS keyring.
2. Confirm green: `cargo fmt --all -- --check`, then `cargo check-all` (clippy
   `-D warnings` all-targets), then `cargo test-all`. (There is no combined
   `cargo verify` alias — Cargo aliases can't chain subcommands — but this is
   exactly what the pre-commit hook runs.)
3. Run it: `cargo run -p nunciod` (daemon), then drive with
   `cargo run -p nuncio-cli -- system status`. See `docs/RUNNING.md` /
   `docs/DEVELOPING-IN-RUSTROVER.md`.
4. Claim an open story issue (M1 first — M1–M5 build the feature surface, M6
   hardens security, M7 freezes the API). Follow `docs/STORY-WORKFLOW.md`.

## Coordination rules
- **One proto-touching story in flight at a time** — several stories add to the
  shared `crates/nuncio-proto/proto/nuncio/v1/nuncio.proto` and its committed
  descriptor golden; serialize them via the PO to avoid conflicts. Non-proto
  stories run in parallel.
- Issues close **only after PO review**, never on a green build alone.
- **Deferred, not scheduled:** #79 (OpenPGP/S-MIME), #80 (WASM/QuickJS plugins),
  #81 (local-LLM summarization). Re-justify before scheduling; never build as filler.

## The milestones & stories
GitHub milestones **M1–M7** mirror `docs/ROADMAP.md`. Each milestone holds
`story`-labelled issues with a full, self-contained spec. Order: M1 → M7 (M1–M5
independent-ish feature work; M6 security; M7 the client-readiness gate).

**M1 — Finish Calendar (CalDAV)** · milestone #23
- #185 — Add Calendar API vertical (proto, daemon, CLI, E2E)  · **[PROTO]**
- #186 — Wire RRULE recurrence expansion into calendar event queries

**M2 — Contacts (CardDAV)** · milestone #24
- #187 — Build a real Contacts (CardDAV) engine, replacing the fabricated client
- #188 — Add Contacts API vertical (proto, daemon, CLI, E2E)  · **[PROTO]**

**M3 — JMAP** · milestone #25
- #189 — Real JMAP client wired into the existing Mail sync path

**M4 — Filters made live** · milestone #26
- #190 — Wire FilterEngine into live sync so matched rules actually fire  *(headline; #193 depends on its helper)*
- #191 — Fix filter header-field and account-field evaluation bugs
- #192 — Move filter edit/export/import/logs onto the Filters gRPC service  · **[PROTO]**
- #193 — Re-expose bulk filter triage as a streaming Filters RPC  · **[PROTO]** *(depends on #190)*

**M5 — Mail lifecycle + incremental sync** · milestone #27
- #194 — Account lifecycle over gRPC (edit/delete/test) + persist TLS mode  · **[PROTO]** *(prior art: `wip/unreviewed-account-tls-sync`, reference-not-trust)*
- #195 — Incremental IMAP sync with progress reporting and per-item FETCH timeout
- #196 — SendMessage selects an explicit account instead of the first configured  · **[PROTO]**
- #197 — Roll back orphaned keyring secret when AddAccount fails to persist

**M6 — Security & release hardening** · milestone #28
- #198 — Fail closed on encryption failure in `PayloadCipher::encrypt_text_at_rest`
- #199 — Zeroize long-lived engine key material on drop
- #200 — Validate WORM/ledger key length from the vault, fail closed on mismatch
- #201 — Reject non-loopback gRPC bind; add a consolidated auth-coverage canary test
- #202 — Fail closed on missing/unfetchable SHA256SUMS in the auto-updater
- #203 — Replace webhook SSRF substring blocklist with real IP/CIDR + DNS checks
- #204 — Sanitize inbound HTML email bodies before they leave the engine

**M7 — API freeze & client-readiness** (the client-ready gate) · milestone #29
- #205 — Enforce full-surface contract coverage + an additive-only v1 policy
- #206 — Verified Swift/C#/TypeScript codegen with connect+GetStatus examples
- #207 — CLI RPC coverage matrix: every `nuncio.v1` RPC has an offline CLI E2E
- #208 — Replace the informational coverage job with an engine gate + required E2E CI job
- #209 — Fix the broken release pipelines and tag an honest pre-alpha backend release

**Deferred (labelled `deferred`, no milestone):** #79, #80, #81.

**Proto-touching stories (serialize via the PO — one in flight at a time):** #185, #188, #192, #193, #194, #196.

## Loose ends for the human
- **#182 / #183** were filed publicly with real account/host identifiers by the
  runaway agent; the bodies are redacted, but GitHub retains edit history — delete
  those two issues if you consider that sensitive.
- Decide the eventual fate of `wip/unreviewed-account-tls-sync` (review-and-adopt
  vs discard) once M5 lands a clean implementation.
