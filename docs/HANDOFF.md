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

## Current state of the code (2026-08-07)
- **Phases 0–2 complete.** The daemon boots gRPC-only on loopback
  `127.0.0.1:9420` behind a keyring bearer token, serving eight services (System,
  Accounts, Mail, Filters, Calendar, Contacts, Export, Audit), each behind the
  auth interceptor. The
  `IMAP → store → read → SMTP send` spine works end-to-end over gRPC via the
  `nuncio-cli` reference client, tested offline. `nuncio.v1` is published with a
  contract-stability golden.
- **The feature surface (Phase 3 through M5) is now COMPLETE — M1–M5 all merged
  to `dev`** (M1–M4 already promoted to `main`; M5 promotes next).
  Filter correctness (3.A), Calendar (M1: CalDAV + RRULE recurrence, API vertical),
  Contacts (M2: real CardDAV engine + API vertical), JMAP (M3: real client wired
  into Mail sync), Filters made live (M4: engine wired into sync with a
  fire-once guard, the `account`-field bug fixed, edit/export/import/logs and
  `Filters.Triage` bulk rescan on the gRPC service), and Mail lifecycle +
  incremental sync (M5: account edit/delete/test-connection with persisted
  `imap_tls_mode`/`smtp_tls_mode` driving the real IMAP/SMTP connection, a real
  per-folder `{uidvalidity}:{uidnext}` incremental-sync checkpoint with a safe
  full re-fetch on UIDVALIDITY change and a per-item FETCH timeout, explicit
  `SendMessage` account selection, and keyring rollback on failed `AddAccount`)
  are all done.
- **M6 — Security & release hardening is 13 of 16 stories closed.** Fail-closed
  encryption, zeroized key material, WORM/ledger key-length validation,
  non-loopback gRPC bind rejection plus the auth-coverage canary, the
  fail-closed updater on missing `SHA256SUMS`, and real CIDR/DNS SSRF checks for
  webhooks have all landed. Open: inbound HTML sanitization (#204), Rust static
  analysis in CodeQL (#250), assertion-free test theater (#251).
- **Work reorganized after M5 into M6.5, WS-A … WS-F, and OBS.** M6.5
  (de-fabrication, 3/11) and the six WS workstreams (22/66) are the pre-freeze
  reshape and now carry most of the remaining effort; **WS-A gates M7**. The
  **OBS observability epic is COMPLETE** (OBS-1 … OBS-9, #357–#365, merged
  without a milestone): tracing subscriber, per-RPC spans with request-id
  correlation, domain/lifecycle coverage, protocol instrumentation, a real
  `System.GetStatus`, CLI `-v`/`-vv` with typed errors, and a `Redacted<T>`
  redaction policy with a no-secrets-in-logs canary. See `docs/LOGGING.md`.
- **M7 (API freeze) has not started** — 5 stories open, 0 closed.
- **Gate verified green on 2026-08-07** under the pinned 1.97.1 toolchain:
  `cargo fmt --all -- --check` clean, `cargo check-all` clean, `cargo test-all`
  596 passed / 0 failed / 0 ignored across 37 suites. (On
  `story/monitor-and-health`: 664 passed / 40 suites.)
- **CI on `dev` is intermittently RED and has been since before 2026-08-07**,
  from three distinct flaky-test root causes — a `tracing` callsite-`Interest`
  race (#375), a port-reuse race (#347), and a timing-sensitive outbox test.
  Local green is therefore **not** CI-equivalent today, despite what the build
  section of `CLAUDE.md` implies. Judge a red PR against this baseline before
  assuming the PR caused it.
- **`dev` is a superset of `main` again.** `main` had drifted ahead by one real
  commit, `157c7c2` (rustls 0.23.42 → 0.23.43), which was never back-merged;
  back-merged 2026-08-07 and the gate re-verified green on the new lockfile.
  Watch for this: a dependency bump merged straight onto `main` leaves `dev`
  building on the older crate until someone merges back.
- Full detail and the path to a client-ready API: `docs/ROADMAP.md`.

## Branches & preserved WIP
- **`dev`** — canonical. Base all story branches here.
- **`story/monitor-and-health`** — ⏳ **COMPLETE, PUSHED, NOT MERGED, NO PR.**
  26 commits off `dev`. Adds `System.Shutdown` and `System.GetHealth` to
  `nuncio.v1` (the latter **closes #296**), `account_id` attribution on the
  outbox, and `crates/nuncio-monitor` — a Windows tray dev instrument. Gate
  green (664 passed / 40 suites). **GUI manual verification is NOT signed off**
  — the tray menu's event-loop wiring is the outstanding unknown, and nothing
  automated covers it. Full detail, design decisions, known gaps and follow-ups:
  [`docs/handoff/2026-08-08-monitor-and-health.md`](handoff/2026-08-08-monitor-and-health.md).
  **Read that before continuing this work on another machine** — the session
  notes that produced it were git-ignored and do not travel.
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
4. Claim an open story issue. M1–M5 (feature surface) and OBS (observability)
   are done; M6 is 13/16. The open work is **M6.5**, **WS-A … WS-F**, and the
   M6 remainder — start with **WS-A**, since it gates the M7 freeze. Follow
   `docs/STORY-WORKFLOW.md`.

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

**M1–M5 are DELIVERED — all stories merged to `dev`.** M6 is 13/16 closed. The
live status for any track is `gh issue list --milestone "<title>"`; the
per-story lists below are a point-in-time snapshot of M1–M7 only and do not
cover M6.5, WS-A … WS-F, or OBS (see `docs/ROADMAP.md` for those).

**M1 — Finish Calendar (CalDAV)** · milestone #23 · **DONE**
- #185 — Add Calendar API vertical (proto, daemon, CLI, E2E)  · **[PROTO]** · merged
- #186 — Wire RRULE recurrence expansion into calendar event queries · merged

**M2 — Contacts (CardDAV)** · milestone #24 · **DONE**
- #187 — Build a real Contacts (CardDAV) engine, replacing the fabricated client · merged
- #188 — Add Contacts API vertical (proto, daemon, CLI, E2E)  · **[PROTO]** · merged

**M3 — JMAP** · milestone #25 · **DONE**
- #189 — Real JMAP client wired into the existing Mail sync path · merged

**M4 — Filters made live** · milestone #26 · **DONE**
- #190 — Wire FilterEngine into live sync so matched rules actually fire  *(headline; #193 depends on its helper)* · merged
- #191 — Fix filter header-field and account-field evaluation bugs · merged
- #192 — Move filter edit/export/import/logs onto the Filters gRPC service  · **[PROTO]** · merged
- #193 — Re-expose bulk filter triage as a streaming Filters RPC  · **[PROTO]** *(depends on #190)* · merged

**M5 — Mail lifecycle + incremental sync** · milestone #27 · **DONE**
- #194 — Account lifecycle over gRPC (edit/delete/test) + persist TLS mode  · **[PROTO]** *(prior art: `wip/unreviewed-account-tls-sync`, reference-not-trust)* · merged
- #195 — Incremental IMAP sync with progress reporting and per-item FETCH timeout · merged
- #196 — SendMessage selects an explicit account instead of the first configured  · **[PROTO]** · merged
- #197 — Roll back orphaned keyring secret when AddAccount fails to persist · merged

**M6 — Security & release hardening** · milestone #28 · **13/16 closed** (open: #204, #250, #251)
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
- Follow-ups filed during M1–M5 that are open but not blocking (not scheduled
  into a milestone yet): #216 (calendar windowing/predicate refinement), #219
  (contacts notes handling), #225 (`filter edit` silently re-enables a disabled
  rule and resets `created_at`), #233 (M5 sync/SMTP robustness polish).
