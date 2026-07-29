# Executing a Nuncio story (guide for LLM contributors)

You have been assigned **one GitHub issue** ("story") from the Nuncio roadmap and
are working on your own machine. This is the canonical protocol for doing it
successfully and getting it merged. **Every story issue links here — read this
first, then the issue.**

Product-owner / reviewer: the coordinating session (referred to as "the PO"). You
implement; the PO reviews and merges. Do not merge your own work.

## 1. Orient (read before coding)
- [`CLAUDE.md`](../CLAUDE.md) — what the repo is, build/test/gates, conventions.
- [`docs/ROADMAP.md`](ROADMAP.md) — where your story fits (milestones M1–M7).
- [`docs/ARCHITECTURE.md`](ARCHITECTURE.md) and
  [`docs/adr/0001-engine-first-grpc-architecture.md`](adr/0001-engine-first-grpc-architecture.md)
  — the engine-first, daemon-owns-everything, gRPC-contract model.
- [`docs/DEVELOPING-IN-RUSTROVER.md`](DEVELOPING-IN-RUSTROVER.md) /
  [`docs/RUNNING.md`](RUNNING.md) — running the daemon + CLI locally.

## 2. The definition of done (non-negotiable)
A story is done only when **all** of these hold:
1. **engine + proto (if the API surface changes) + CLI command + offline E2E test
   exist together** and prove the capability end to end.
2. The full local gate is green — that is `cargo fmt --all -- --check`,
   `cargo check-all` (clippy `--all-targets --workspace -D warnings`, **zero**
   warnings), and `cargo test-all` (`cargo test --workspace`), run as three
   separate commands (there is no combined `cargo verify` alias — Cargo aliases
   can't chain subcommands). **0 ignored tests.**
3. **No fabricated success.** A path that cannot do the real thing returns an
   honest `Unimplemented`/error — never canned data, never a fake "sent"/"ok".
   (Anti-pattern to never imitate: a client that logs "fetching…" and returns a
   hardcoded result; a test that asserts on fabricated output.)
4. All external protocols are **mocked** (`wiremock` / `Mock*Backend`); **live
   network calls in tests are forbidden.** Never connect to a real server.
5. The proto **contract-stability** test passes (see §5).

## 3. Hard constraints
- Rust edition 2021. **No `unsafe`.** No `unwrap`/`expect`/`panic`/`todo` in
  non-test code (tests are exempt via each crate's `#![cfg_attr(test, allow(...))]`).
  Use `thiserror` error types.
- **Comment convention:** code comments (`.rs`/`.proto`) state the code's
  intent/constraints/rationale ONLY. **No** backlog/story numbers, **no** GitHub
  issue refs (`#NNN`, `GH-`, `Refs #`), **no** "Phase N"/"story"/"backlog"
  breadcrumbs in comments. Keep genuine technical rationale (RFC citations,
  algorithm notes, why-not-X). That traceability lives in git history + PRs.
- Toolchain is pinned (`rust-toolchain.toml` = 1.97.1). Green locally ==
  green in CI. The pre-commit hook runs the full gate.

## 4. The security invariant (do not violate)
**Every gRPC service MUST be mounted behind its own `BearerAuthInterceptor`.**
See the doc comment in `nuncio.proto` and the mounting code in
`crates/nunciod/src/grpc.rs` (`serve_on_listener_with_overrides`). If you add a
service, wrap it exactly like `Mail`/`Filters`. Your E2E must assert that an
**unauthenticated** call to your service is rejected (mirror
`crates/nunciod/tests/grpc_auth_test.rs`).

## 5. Patterns to mirror (copy these, don't invent)
- **A full API vertical** (proto → daemon → CLI → E2E): the **Mail** vertical is
  the reference. `service Mail` in `crates/nuncio-proto/proto/nuncio/v1/nuncio.proto`;
  `MailGrpcService` + `MailEngineOverrides` + `serve_on_listener_with_overrides`
  in `crates/nunciod/src/grpc.rs`; `connect_mail` in
  `crates/nuncio-proto/src/client.rs`; `handle_*`/`connect_mail_client` in
  `crates/nuncio-cli/src/runner.rs`; the capstone `crates/nunciod/tests/spine_e2e_test.rs`.
- **An engine backend + transport** (real protocol client + mock + trait): the
  **Calendar** engine (`crates/nuncio-cal/src/{backend.rs,caldav.rs,parser.rs}`)
  mirrors `nuncio_mail::MailBackend`; its wiremock test is
  `crates/nuncio-cal/tests/wiremock_caldav_test.rs`. The JMAP wiremock test
  `crates/nuncio-mail/tests/wiremock_jmap_test.rs` is the protocol-level template.
- **Store CRUD**: mirror `DatabaseEngine::save_email` and the calendar CRUD in
  `crates/nuncio-store/src/db.rs`.
- **Changing the proto → regenerate the golden.** Any change to `nuncio.proto`
  will (by design) fail `nuncio-proto`'s `contract_stability::descriptor_matches_committed_golden`
  test. The failure message in `crates/nuncio-proto/src/lib.rs` prints the exact
  recipe: rebuild `-p nuncio-proto`, copy the freshly generated
  `nuncio_v1_descriptor.bin` from this build's `OUT_DIR` over
  `crates/nuncio-proto/proto/nuncio/v1/descriptor.bin`, re-test, and commit the
  new golden with your change. Keep additions **additive** within `nuncio.v1`.

## 6. Proto coordination (important for parallel work)
Several stories add to the **same** `nuncio.proto` + its golden. To avoid
conflicts, **only one proto-touching story is "in flight" at a time.** Before
starting a story that changes `nuncio.proto`, confirm with the PO that no other
proto story is open. Non-proto stories (engine-only, CLI-only) can run in parallel.

## 7. Branch, PR, and review flow
1. Branch from `dev`: `story/<milestone>-<slug>` (e.g. `story/m1-calendar-api`,
   `story/m6-zeroize-keys`). **Never commit to `dev` or `main` directly.**
2. TDD where practical. Keep commits Conventional (`feat(scope): …`,
   `fix(scope): …`), imperative, <72-char subject, **no AI attribution**, no
   issue refs in code (a `Refs #NNN` in the commit message is fine).
3. Before pushing, make sure `cargo fmt --all -- --check`, `cargo check-all`, and
   `cargo test-all` are all green. The pre-commit hook enforces this.
4. Open a **PR to `dev`** (never to `main`). In the PR body: link the issue,
   summarize what you built, **paste the gate output** (fmt/clippy/test results +
   test counts) as evidence, and list any decisions or follow-ups. If you found a
   real bug outside your scope, **file a new issue** — do not silently fix or
   ignore it.
5. The PO reviews spec compliance + code quality (and, for auth/crypto/transport,
   an adversarial pass). Address Critical/Important findings. **The issue closes
   only after the review passes** — never on a green build alone.

## 8. If you get stuck
Report honestly: what's blocking, the exact error, and what you tried. Do not
weaken the auth invariant, skip the hook (`--no-verify`), fabricate data, or hit
a live server to make something pass. An honest BLOCKED beats a green lie.
