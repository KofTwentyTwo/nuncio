# ADR 0002 — Convergent sync model for multiple concurrent engines

- **Status:** Accepted
- **Date:** 2026-08-10
- **Deciders:** James Maes
- **Extends:** [ADR 0001](0001-engine-first-grpc-architecture.md). Nothing here
  changes the engine-first, contract-published architecture; this decides how a
  daemon behaves as one of several clients of the same mail account.

## Context

ADR 0001 establishes one daemon owning all state. It does not say what happens
when **several** daemons own state derived from the **same** upstream account —
which is the normal case for a local-first product the moment a user runs it on
a laptop and a desktop, and is already the case today whenever the user also has
a phone mail app or Gmail open in a browser.

The engine currently implements a **single-writer, append-only** model:

- Mail sync is a forward-only per-folder high-water mark (`{uidnext}:*`), and
  `fetch_and_persist` only ever calls `save_email`. Nothing observes deletion.
- CalDAV/CardDAV sync re-queries a full window every time; there is no
  sync-token, no CTag, no ETag use, and no write-back at all.
- Local identity is `surrogate_id(account, folder, uidvalidity, uid)`, so the
  same message in a different folder is a different message.
- Mutations are fire-and-forget: `apply_mutation` returns `Result<()>` and
  `Ok(())` is recorded as `completed` with no check that anything happened.

The consequences are not hypothetical. A message moved or deleted by any other
client becomes a permanent local ghost. A mutation that lost a race against
another client is reported as success, because a UID command against a UID that
no longer exists returns `OK` having done nothing (RFC 3501 §6.4.8). Two engines
diverge silently; ten diverge identically and confidently.

A design was drafted, then reviewed independently for protocol correctness,
codebase fit, and adversarially. The reviews falsified four load-bearing claims
in the draft. This ADR records the corrected decision; the evidence is in
[`docs/reviews/2026-08-10-multi-engine-design-review.md`](../reviews/2026-08-10-multi-engine-design-review.md).

## Decision

### 1. The servers are the coordinator. We do not build a distributed system.

No engine-to-engine messaging, no leader election, no shared database, no vector
clocks or CRDTs, no broker, no custom sync protocol. IMAP and WebDAV are designed
for many concurrent clients against one collection and ship the primitives for
it. If each engine is a **correct** client, N engines is **O(N) server load and
O(1) design complexity**.

The work is therefore not "add multi-engine support." It is "finish the sync
model"; multi-engine falls out of it.

### 2. There is one bounded exception: side-effecting actions.

The servers coordinate *state*, not *actions*. `MOVE`/`COPY`/`FLAG` converge
under at-least-once execution — ten moves produce one net effect. **`FORWARD`
and `CALL WEBHOOK` do not.** They land on third parties with no shared resource
to compare-and-set against, and every engine independently classifies the same
message as new. Ten engines page an on-call engineer ten times per alert.

- A per-account `filters_enabled` setting, **default off**, gives exactly one
  engine ownership of filter execution. This converts a silent duplication bug
  into an explicit, documented limitation, and requires no protocol work.
- Where CONDSTORE and custom keywords are available, an engine may instead claim
  a message with `UID STORE <uid> +FLAGS.SILENT (UNCHANGEDSINCE <modseq>)
  ($NuncioFiltered)`; a `MODIFIED` response means another engine won the claim.
  This is the same compare-and-set primitive as §4, so it adds no new machinery.
- Rule identity becomes a content hash of normalized NSQL rather than random
  bytes, and side-effecting actions carry `Idempotency-Key:
  hash(rule_key, message_key)`, so receivers can dedup what we cannot make
  once-only.

### 3. Change enumeration replaces the high-water mark — four rungs, per mailbox.

`CollectionSync::enumerate_changes(collection, since) -> {upserts, removals,
next_token}`. The `removals` set is the entire fix for ghost messages, and it
fixes the single-engine case too.

| Rung | Mechanism | Providers |
| --- | --- | --- |
| 1 | QRESYNC: `ENABLE QRESYNC` + `SELECT (QRESYNC …)` → `VANISHED` | Fastmail/Cyrus, Dovecot |
| 2 | CONDSTORE: `UID FETCH 1:* (FLAGS) (CHANGEDSINCE n)` + periodic UID-set diff | **Gmail** |
| 3 | `STATUS`/`LIST-STATUS` change gate, then ESEARCH UID-set diff | **Exchange**, Yahoo |
| 4 | Full resync | last resort |

**Gmail advertises CONDSTORE but not QRESYNC; Exchange advertises neither.**
Detection is per *mailbox*, not per account, because `NOMODSEQ` is a mailbox
property. A two-rung ladder would put the largest provider on the most expensive
path.

CalDAV and CardDAV both use RFC 6578 `sync-collection` (identical shape), with
`sync-level: infinite`, 507 truncation handling, loop-to-convergence, and
`calendar-multiget`/`addressbook-multiget` to fetch bodies for changed hrefs.
CTag is a vendor extension, not a peer mechanism, and is a fallback only.

Sync state is structured, not a string: `(uidvalidity, highestmodseq,
known-uid-set, seq-match-sample)`. QRESYNC **never signals modseq expiry** — a
too-old modseq produces a large `VANISHED`, not an error — so a full resync is
always a client-side derivation from a UIDVALIDITY mismatch, `NOMODSEQ`, or a
HIGHESTMODSEQ regression. A periodic full UID-set reconciliation is retained
even on rung 1 as a safety net.

**Sync tokens carry a scheme tag** (`v2:qresync:…`). The existing checkpoint is
`{uidvalidity}{DELIM}{uid}` — the same shape as `uidvalidity:highestmodseq` with
a different meaning. Untagged reuse would hand a UIDNEXT to a server as a
MODSEQ; because UIDNEXT is typically the larger number, the server would answer
"nothing changed" and everything below it would never be enumerated again.
Unknown schemes force a full resync.

### 4. Mutations carry a precondition and must prove their effect. Three outcomes.

```
MutationOutcome = Applied { token: Option<Token> } | Conflict { observed } | Unknown
```

`Conflict` requires **positive evidence**: `[MODIFIED …]` in the tagged
response, or HTTP 412. Anything ambiguous is `Unknown`, which triggers
re-enumeration of the affected collections rather than a verdict.

This matters because the obvious proofs are unreliable:

- `COPYUID` is only *SHOULD* for MOVE (RFC 6851 §4.3), is advised in an
  **untagged** OK, has no empty form, and is legitimately omitted for
  UIDNOTSTICKY destinations. Present ⇒ applied; absent ⇒ **unknown**.
- Under QRESYNC, `UID EXPUNGE` returns `VANISHED`, not `EXPUNGE` (RFC 6851 §4.4),
  so counting `EXPUNGE` responses reports failure on every successful delete.
- `+FLAGS.SILENT` suppresses the untagged FETCH (RFC 3501 §6.4.6) and is the
  normal form for conditional stores, so "no FETCH ⇒ conflict" fires on the most
  common flag operation.
- A DAV server that rewrites a submitted object MUST NOT return an ETag
  (RFC 4791 §5.3.4), and normalisation is the norm; `Applied` must tolerate a
  missing token and schedule a re-read.

Flags use per-flag last-writer-wins with benign merge. RFC 7162 §3.1.3 only
*SHOULD*s avoiding spurious `MODIFIED` on single-modseq servers, so a blanket
hard-conflict policy would make routine concurrent flag activity a constant
conflict stream.

### 5. Conflicts are durable and queryable, not a notification.

Mutations model **desired end-state on an identity**, not an imperative
operation on a placement, so re-enumerate-and-retarget preserves user intent;
re-targets are capped, then a hard conflict is raised. Conflicts persist in a
table with RPCs to list and resolve them. Events become an optimization over a
queryable source of truth — necessary because `Subscribe` is a broadcast stream
that silently drops events on lag, so no event-driven guarantee is currently
sound without a sequence number and resumption.

### 6. Identity is content-addressed and account-scoped.

Precedence: `EMAILID` (RFC 8474 OBJECTID) → `X-GM-MSGID` → `(account_id,
normalized Message-ID, content_hash)` → `(account, folder, uidvalidity, uid)`.

Bodies are keyed by a content hash of the full RFC822 octets, first-write-wins.
**A Message-ID hash alone must not be the key**: Message-IDs are public, echoed
in every `References:` header, and forgeable, so with `INSERT OR REPLACE` a
message reusing a known Message-ID would silently overwrite the body of a
message the user already trusts and would evade filters by never being seen as
new. That hole does not exist today and must not be introduced. The same
Message-ID with different content is two messages, which also covers
list-copy-vs-direct and drafts.

A message is separated from its **placements** — `PRIMARY KEY(account_id,
folder_id, uidvalidity, uid)`, with `uidvalidity` in the key so UIDs restarting
after a bump cannot collide with old rows. DAV gets the same split: href =
placement, iCalendar/vCard UID = identity, ETag = version.

### 7. Load is bounded by bandwidth, not connection count.

Gmail publishes both a 15-simultaneous-connection and a 2,500 MB/day download
cap **per account**. On rungs 2 and 3, ten engines over five accounts reach the
byte cap long before the connection cap. Per-engine budget derives from a
`fleet_size` setting, since cross-engine rate limiting is impossible without the
coordination layer we decline to build. `Retry-After` is honoured *with* jitter
proportional to `fleet_size`, or ten engines retry in the same instant.

One mutex-guarded connection per (engine, account) with mutation priority; sync
work is chunked per folder so it yields to user actions.

## Consequences

- **This gates the M7 API freeze.** Mutation RPCs do not exist (`Mail` has no
  Move/Delete/Flag); `Conflict` must be a typed error, not a response field, so
  old clients take the error path; `Message` needs identity and placements.
  Most individual additions are wire-additive, but redefining `Message.id`
  changes the *value* of every existing id while staying wire-compatible —
  invisible to the byte-exact descriptor golden, and that token appears at ten
  other wire positions. A model that cannot express deletion or conflict must
  not be frozen.
- **Shared OAuth accounts across engines are out of scope.** Gmail and
  Microsoft 365 mandate OAuth; providers that rotate refresh tokens on use would
  have ten engines invalidating each other. A token broker is exactly the
  coordination layer this ADR declines to build. Documented limitation.
- **Migration is forward-only.** `message_key` cannot be derived for existing
  rows: Message-ID is never parsed, never stored, and raw bytes are discarded
  after parse. Ingest must capture identity first and let it accumulate. Legacy
  rows keep their surrogate as `message_key` with one placement each and receive
  no retroactive dedup.
- **The audit ledger gains a namespace boundary.**
  `filter_execution_logs.message_id` is an input to an HMAC hash chain verified
  by a live `Audit.VerifyChain`, so historical ids cannot be rewritten.
- **A new class of test is required.** Convergence cannot be shown by one engine
  against one mock. Verification needs ≥3 engines against one stateful server,
  plus side-effect-counting SMTP and webhook mocks — final-state placement
  convergence is blind to duplicate forwards, so the largest risk in this ADR
  would pass a naive convergence test.
- **`async-imap` 0.11.3 cannot express any of this**: no `ENABLE`, its `SELECT`
  parser drops `VANISHED`, and `check_status_ok` discards the tagged response
  code where `COPYUID` and `[MODIFIED …]` live. A fork-versus-hand-rolled
  decision gates the protocol work.

## Alternatives considered

- **A coordination layer** (broker, leader election, shared state). Rejected: it
  would be the largest and least reliable component in the repo, and it solves a
  problem the servers already solve for everything except side-effecting actions.
- **CRDTs / vector clocks.** Rejected: the server holds authoritative state, so
  there is no merge to compute — only a state to observe and preconditions to
  respect.
- **Freezing `nuncio.v1` first and adding conflict semantics later.** Rejected:
  not expressible additively.
