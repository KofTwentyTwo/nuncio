# Design review — multi-engine sync model (2026-08-10)

Evidence behind [ADR 0002](../adr/0002-multi-engine-sync-model.md). A draft
design was reviewed independently along three axes, each instructed to find
errors rather than confirm the draft, and to say plainly when an attack failed.
All three found load-bearing defects. Reviewed against `dev` @ `a997220`.

Findings are recorded here because several are non-obvious, several cost real
effort to establish, and at least two would otherwise be rediscovered the
expensive way.

---

## A. Protocol correctness

### A1. The provider capability matrix in the draft was wrong

The draft claimed Gmail, Fastmail, Dovecot, and Exchange all support CONDSTORE
and QRESYNC. Verified against live capability strings:

| Provider | CONDSTORE | QRESYNC |
| --- | --- | --- |
| Fastmail (Cyrus) | yes | yes (also RFC 8474 OBJECTID) |
| Dovecot | yes | yes, but `NOMODSEQ` per mailbox where persistent modseqs were never enabled |
| **Gmail** | yes | **no** — not planned |
| **Exchange / Exchange Online** | **no** | **no** |

Two of four named providers cannot use the draft's headline mechanism. This is
what forced the ladder from two rungs to four, and capability detection from
per-account to per-mailbox (RFC 7162 §3.1.2.2 makes `NOMODSEQ` a mailbox
property).

The draft also omitted the CONDSTORE-only middle rung entirely — RFC 7162 §3.2.5.1
states QRESYNC's `SELECT` is semantically equivalent to `UID FETCH … (CHANGEDSINCE
… VANISHED)`, i.e. a round-trip optimisation over CONDSTORE, not a different
capability class. Skipping it put Gmail on full flag sweeps against a published
2,500 MB/day per-account cap.

### A2. QRESYNC has no modseq-expiry error

RFC 7162 §3.2.6: a server receiving a mod-sequence older than it remembers
**MUST behave as if asked to report all expunged messages from the provided UID
set**. The failure mode is a bandwidth blowup, not a refusal. `full_resync_required`
can therefore never be *received*; it must be derived client-side from a
UIDVALIDITY mismatch (§3.2.5), `NOMODSEQ` (§3.1.2.2), or a HIGHESTMODSEQ
regression.

`uidvalidity:highestmodseq` is also insufficient state: §3.2.5.1 and §3.2.5.2
make the known-UID set and sequence-match data part of what the client supplies.
And §3.2.3 requires a tagged **BAD** if `QRESYNC` is used without `ENABLE QRESYNC`
on the connection.

### A3. "Absent COPYUID means the mutation failed" is wrong three times over

- RFC 4315 §3 ABNF has no empty `uid-set`; the states are *present* or *absent*.
- Absence is conforming on success: the client may lack SELECT rights on the
  destination, or the destination may be UIDNOTSTICKY.
- RFC 6851 §4.3 makes COPYUID only *SHOULD* for MOVE, and advises sending it in
  an **untagged** OK — so a client reading only the tagged OK misses it on every
  conforming MOVE.

Additionally, RFC 6851 §4.4: under QRESYNC the server sends `VANISHED` rather
than `EXPUNGE` for `UID EXPUNGE`, so a delete proved by counting `EXPUNGE`
responses reports conflict on every success.

Consequence: a third outcome, `Unknown`, is required. `Conflict` needs positive
evidence.

### A4. MODIFIED does not cover the case the draft cared about

RFC 7162 §3.1.3 puts `MODIFIED` in the **tagged** response, and only *SHOULD*s
that servers avoid spurious `MODIFIED` on single-modseq servers — so engine B
setting `\Seen` can fail engine A's unrelated `\Flagged` store. More
importantly, `MODIFIED` means "changed by someone else"; a UID another engine
already *moved or expunged* is simply ignored (RFC 3501 §6.4.8) and never
appears in `MODIFIED`. Presence detection needs a separate mechanism.

Relatedly, the draft's flag proof-of-effect ("untagged FETCH present") is unsound:
`.SILENT` suppresses it (RFC 3501 §6.4.6) and is the normal form for conditional
stores.

### A5. Message-ID is not a content identifier

RFC 5322 §3.6.4 makes it *SHOULD*, not MUST. One Message-ID legitimately maps to
different octets for mailing-list-plus-direct copies (Mailman preserves it while
adding `List-*` headers and a footer) and for drafts (Gmail and Exchange keep one
Message-ID across saves). The draft's "hash of raw headers" fallback is also
unstable across exactly the placements it exists to unify, because `Received:`
chains differ.

Missed primitives that solve this properly: **`EMAILID`** (RFC 8474, Fastmail/Cyrus,
Dovecot) and **`X-GM-MSGID`** (Gmail) are server-assigned identities stable across
folders.

### A6. DAV specifics the draft omitted

RFC 6578 §3.6 truncation (207 + `507` + `DAV:number-of-matches-within-limits`) —
"Clients MUST handle the 507 status"; §3.1 loop-to-convergence; `sync-level:
infinite` (§3.5.1 warns `sync-level: 1` is not reliable); discovery via
`DAV:supported-report-set`; and `calendar-multiget`/`addressbook-multiget`
(RFC 4791 §7.9, RFC 6352 §8.7) because `sync-collection` returns hrefs and ETags,
not content. RFC 4791 §5.3.4: a server that rewrites the submitted object MUST NOT
return an ETag. RFC 6638 scheduling is a write hazard — PUTting a VEVENT with
ATTENDEEs on a scheduling-aware server emails real people.

`getctag` is a Calendar Server vendor extension, not an RFC, and was
mis-presented as a peer of RFC 6578.

### A7. Other omitted primitives

`STATUS (… HIGHESTMODSEQ)` (RFC 7162 §3.1.7) and `LIST-STATUS` (RFC 5819, which
Gmail advertises) give a whole-account change probe in one round trip.
`COMPRESS=DEFLATE` (RFC 4978) directly reduces metered bytes. `NOTIFY` (RFC 5465)
watches many mailboxes on one connection, which moots the draft's
"IDLE costs a connection per mailbox" reasoning. ESEARCH `SEARCH RETURN (ALL)`
is far cheaper than `UID FETCH 1:* (FLAGS)` for a vanished-set diff.
`UIDNOTSTICKY` (RFC 4315) invalidates both COPYUID proof and the placement key
and must be detected.

---

## B. Codebase fit

### B1. `message_key` cannot be derived — the blocker is ingest, not migration

`MimeParserAdapter::parse_mime` never calls `message_id()`; `Email` has no such
field; the `messages` table has no column for it; and `build_email_from_fetch`
discards the `BODY[]` bytes after parsing. The IMAP `ENVELOPE` does carry
`message-id`, but only `subject`/`from`/`to` are read.

So in-place migration of a populated database is impossible — there is neither a
Message-ID nor raw headers to hash. Either re-fetch every message (offline-hostile,
impossible for a removed account) or set `message_key := old surrogate`, which
yields one placement per message and no retroactive dedup. Capture must ship
first and accumulate.

### B2. The audit hash chain pins historical ids permanently

`filter_execution_logs.message_id` is an input to `compute_log_hash`, verified by
`verify_execution_log_chain`, reachable from the live `Audit.VerifyChain` RPC.
Rewriting historical ids breaks verification permanently. `worm_audit_records` is
harder still — `BEFORE UPDATE`/`BEFORE DELETE` `RAISE(ABORT)` triggers. A
documented namespace boundary with a schema-version marker is the only option.

### B3. Placements break the filter engine's evaluation contract

`FilterField::Folder` and `FilterField::Account` evaluate scalar fields on
`Email`; `WHERE FOLDER = 'INBOX'` has no meaning against a multi-placement
message. `evaluate(&Email)` must become per-placement.

More seriously: **the fire-once guard is `existing_message_ids`, not the
execution log.** Under `message_key`, a genuinely new arrival that already exists
in another folder becomes "not new" and **silently never fires any filter** — a
new correctness bug the identity change introduces. The guard must become
placement-aware *and* gain a real `(rule_id, message_key)` dedup.

`set_message_read` is `UPDATE messages … WHERE id = ?`; since IMAP `\Seen` is
per-mailbox, read state becomes per-placement and `MARK READ` must name one.

### B4. FTS orphans are a confidentiality problem

`messages_fts` holds **plaintext** bodies keyed on `messages.id`, with a
delete-only trigger. Under placements, dropping *a* placement must not delete the
FTS row while others reference the body; dropping the *last* must. Inverted, this
either loses search or leaks plaintext bodies in orphaned rows — in the one table
the schema comments already document as unencrypted.

Also: there is no `folders` table. Folders are derived from `messages GROUP BY
folder_id` and are currently global across accounts, which placements will expose.

### B5. `async-imap` 0.11.3 cannot express the design

| Needed | Status in the dependency |
| --- | --- |
| `COPYUID` | `uid_copy`/`uid_mv` return `Result<()>`; `check_status_ok` discards the response code |
| `[MODIFIED …]` | no `UNCHANGEDSINCE` variant; tagged code discarded |
| `SELECT (QRESYNC …)` | no `select_qresync`; no `ENABLE` command at all |
| `VANISHED` | parsed by `imap-proto` but dropped by async-imap's `select()` parser |
| untagged FETCH / expunged UIDs | reachable; both currently drained and discarded |

`run_command`/`read_response` are public, so a hand-rolled tagged-response
collector is possible (~80–150 LOC plus per-command parsers) — but it is a
prerequisite, and it is subject to the workspace's `-D warnings` / no-`unwrap`
regime. Fork-versus-hand-roll is a gate on the protocol work.

### B6. There is no IMAP mock, and no scheduler, and no backoff

- `wiremock` is HTTP-only. IMAP had only ad-hoc inline `duplex` responders:
  single connection, no shared state. *(Addressed by PR #384.)*
- `OutboxManager::calculate_backoff_ms` is **dead code**; retries run on a fixed
  5s tick with `MAX_RETRIES = 5`, so a 25-second blip permanently fails every
  queued mutation fleet-wide.
- `sync_interval_secs` is stored and validated but **nothing polls**. There is no
  periodic sync at all; `main.rs` spawns only the outbox drain (5s) and an opt-in
  update check (24h).
- `SyncProgress` never reaches the wire — it is dropped at the gRPC boundary.
- `Mail.MarkRead` and `Contacts.CreateContact` are silently **local-only** and
  report success.
- `AccountProtocol` has no `CardDav` variant, so a CardDAV account is not
  representable — contacts sync is unwireable, not merely unwired.
- The DAV "parsers" are substring splits on `<c:calendar-data>`; a different
  namespace prefix yields zero events reported as a successful sync. DAV ids are
  positional, so re-syncs reshuffle which local row holds which remote object.

### B7. The proto claim was overstated in the draft's favour and against it

Three of five changes *are* wire-additive with free tags (mutation RPCs, sync
tokens in `GetStatus`, new `Event` oneof arms). The genuinely dangerous one is
invisible: redefining `Message.id` changes the *value* of every existing id while
staying wire-compatible — the byte-exact descriptor golden cannot see it, and the
token appears at ten other wire positions.

`STORY-WORKFLOW.md` permits one proto-touching story in flight; since four work
items touch the proto, a single "proto chunk" is not schedulable and must
dissolve into a serialized tail on each.

### B8. Sizing

The draft's M/L/M/S/M (~4–6 weeks) is roughly 3× optimistic. Measured against the
tree: **5,800–9,300 production LOC + 4,000–6,600 test LOC across ~90–120 file
touches** — about a quarter of focused work. Chunk 1 alone is ~120–160 edit sites
across ~25–30 files. This substantially *is* WS-B + WS-C + WS-E + parts of M6.5.

---

## C. Adversarial

### C1. Filter fan-out — the largest hole, absent from the draft entirely

All ten engines classify the same message as new (per-engine store), and
`pending_remote_mutations` has no uniqueness constraint. State-mutating actions
converge; `FORWARD` and `CALL WEBHOOK` do not — the on-call engineer is paged ten
times per alert. Rule IDs are `OsRng`-random per engine, so the "same" rule has
ten identities and no receiver can dedup from rule identity even in principle.

Adding a `Conflict` outcome makes the UX *worse* here, not better: the nine losing
MOVEs stop silently fabricating success and instead emit conflict events to two
clients each — eighteen spurious notifications per filtered message. "Server wins,
loudly" is calibrated for rare conflicts; filter fan-out makes conflict routine.

The draft's one sentence that sounds like it addresses this ("filters key on
`message_key`, so they stop re-firing on the destination copy") is about *self*
re-firing after a move — a different bug.

### C2. The draft's identity scheme introduces a spoofing primitive

Message-IDs are public and echoed in `References:`. With `INSERT OR REPLACE` and a
key of `hash(Message-ID)`, an attacker reusing a known Message-ID silently
overwrites the body shown under the original message's placement, and evades
filters entirely by never being classified new. **This hole does not exist today**,
because the current `surrogate_id` includes the UID. The identity change would
have created it.

### C3. Mixed-version fleets fight on the shared server

Old engines key identity on folder, so a new engine's move looks like a brand-new
message to them — they re-evaluate the rule set and re-move it. Old and new
engines trade the message back and forth. There is no `PRAGMA user_version`, no
down path, and an old binary opening a new database "works" while writing the old
model, leaving two mutually invisible truths in one file.

Worst of these: the sync-token shape collision described in ADR 0002 §3, which
fails *silently* rather than loudly.

### C4. "Server wins, loudly" strands intent and leaves clients nothing to act on

A user pressing Delete on engine 7 for a message engine 2 already archived has
their intent discarded; re-enumeration removes the placement, so from their seat
the delete appeared to work. Retry is *safe* here — `message_key` is stable and
the new location is known — and the blanket prohibition follows from modelling
mutations as imperative operations on placements rather than desired end-state.

Clients get no event payload contract, no RPC to act on a conflict, no listing, no
GC, and `Subscribe` silently drops events on lag with no sequence number or
resumption. "Loudly" reduces to a log line that sometimes reaches a UI.

### C5. Connection math measures the wrong quantity

The outbox opens a **fresh connection per mutation**: worst case 50 connect/login
cycles per 5s per engine, ×10 engines = 500 authentications per 5 seconds against
one account. Providers rate-limit *authentications* far harder than concurrent
connections, and some respond with a temporary lockout. Sync and outbox also
contend for the single pooled connection the draft specifies — share it and a
mark-read queues behind a 300-second per-account sync timeout; don't, and it is
2 per engine, which the draft's own table marks as over cap.

Jitter alone does not solve correlated wake-ups (workday start, VPN up, power
restored), and honouring `Retry-After` without jitter creates a synchronized herd.
Rate limiting across engines is impossible without coordination, so an engine must
know its `fleet_size` and derive its budget from it.

### C6. Placement schema and mutation addressing

The draft's `PRIMARY KEY(account_id, folder_id, uid)` collides after a UIDVALIDITY
bump, when UIDs restart at 1. And a mutation has no placement coordinate: a
message in both INBOX and Archive with a rule saying MOVE TO Archive has no
defined source. The outbox currently recovers addressing from the *message* row,
which ceases to exist under the split.

Drafts are unaddressed and will churn the model (APPEND-then-delete per save, new
UID each time, same or new Message-ID depending on client).

### C7. The proposed verification cannot detect C1

Final-state placement convergence is exactly blind to duplicate side effects — ten
forwards and one forward produce identical placements. N=2 also shows a single
duplicate, which reads as an off-by-one. Requires N≥3, side-effect-counting SMTP
and webhook mocks asserting "exactly one forward across the fleet", and an
interleaving harness that can hold one engine's mutation in flight while another
acts on stale state.

### C8. OAuth breaks the shared-account premise

Gmail and Microsoft 365 mandate OAuth; there is none in the tree. Providers that
rotate refresh tokens on use would have ten engines invalidating each other in a
loop, and with no backoff that becomes a lockout. A token broker is the
coordination layer the design declines to build — so this is a scope boundary, not
a TODO.

### C9. Smaller items

Clock skew across ten machines makes any wall-clock tiebreak unreliable (outbox
ordering, audit ledger). "Body stored once" is per-engine, not fleet-wide. The
append-only hash-chained execution log has no retention policy and will orphan
entries. One poison collection that always fails burns the account's 300s timeout
every cycle. And the draft never stated a consistency guarantee at all — several
of the above decisions fall out of writing one down.

---

## What the reviews agreed the draft got right

- **"Don't build a distributed system" is correct for state convergence.** For
  placements, flags, and DAV objects, the servers genuinely are the coordinator,
  and O(N) load with O(1) design complexity is the right trade. The thesis fails
  only for side-effecting actions (C1) — a bounded exception, not a repudiation.
- **`CollectionSync` with `{upserts, removals, next_token}` is the right shape**,
  and the RFC 7162 / 6578 mapping is sound.
- **"An empty effect must not be reported as success" is correct and important**;
  the objection is to the verdict drawn from it, not to detecting it.
- **The M7-freeze argument is correct and understated** — C4 and B6 strengthen it,
  since the freeze must also cover a durable conflict surface, event sequence
  numbers, and the fact that `MarkRead` will come to mean something different.
- **Change enumeration ships first and alone**, and fixes ghost messages for
  single-engine users independently of everything else. The draft was wrong that
  it depends on the identity split.
