# Logging & redaction policy

Nuncio's telemetry (`tracing` spans and events, plus anything that reaches a log
sink through a `Debug` / `Display` / `serde` rendering) is a plaintext side
channel. This document is the authoritative statement of what may and may not
appear in it. It is enforced mechanically by two things:

- **`nuncio_core::redact::Redacted<T>`** — a newtype whose `Debug`, `Display`,
  and `serde` all render the fixed placeholder `<redacted>`, so a secret field
  wrapped in it cannot leak even through a *derived* `Debug` on the containing
  struct. The inner value is reachable only via an explicit, greppable
  `expose_secret()` / `into_inner()` call. The module doc comment restates this
  policy next to the type.
- **the no-secrets canary** — `crates/nunciod/tests/no_secrets_canary_test.rs`,
  a workspace test that drives a representative slice of the instrumented code
  paths with known sentinel secrets and asserts none of them appear in captured
  telemetry. It is the regression net: if a future change starts logging a
  secret, this test fails.

## MUST NEVER be logged (in any encoding)

- Message bodies, subjects, and snippets.
- Sender / recipient email addresses and any other message correspondents.
- Account credentials: passwords and app-specific passwords/tokens.
- gRPC bearer tokens.
- Keyring secret **values** (the material stored in the OS vault) and any
  derived key material (encryption keys, HMAC signing keys).
- vCard / iCal personal data: contact names, phone numbers, addresses; event
  summaries, locations, attendees, and descriptions.

## MAY be logged (identifying but non-sensitive)

Operating the daemon is impossible without correlation identifiers, and none of
the following reveals content or grants access:

- Opaque ids: account id, message id, rule id, folder id — and UIDs.
- Counts, sizes, and durations.
- Operation / rule / transport / action **kinds** (the discriminant only, never
  its payload).
- Folder names, hostnames, ports, and the per-request `request_id`.
- The keyring **lookup key** (e.g. `nuncio/<account-id>`) — an identifier that
  names a vault entry but is not itself the stored secret.

## Guidance for contributors

- Prefer logging an **id or a kind** over the value it points at. If you need to
  correlate a log line with a message/event/contact, log its id/UID, never its
  content.
- When a struct must hold a credential/token/key, make the field type
  `Redacted<T>` rather than relying on a hand-written `Debug`. Reach the value
  only at the point of use, via `expose_secret()`.
- Never interpolate a whole config/request struct into a log or error with
  `{:?}` unless every sensitive field it holds is `Redacted<T>` (or otherwise
  self-redacting).
- If you add an instrumented path that could plausibly touch any of the
  "MUST NEVER" values, extend the canary to drive it with a sentinel.
