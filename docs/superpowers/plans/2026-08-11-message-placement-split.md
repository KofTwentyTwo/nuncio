# Message/Placement Identity Split — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Separate a mail message's *identity* (content-addressed, folder-independent) from its *placements* (one row per folder the message occupies), so a message moved between folders stays one message.

**Architecture:** `messages` becomes identity + immutable content, keyed by a content-addressed `message_key` derived through a four-tier precedence (`EMAILID` → `X-GM-MSGID` → `(account, Message-ID, content_hash)` → legacy surrogate). A new `placements` table holds `(account_id, folder_id, uidvalidity, uid)` as its primary key plus the per-mailbox read flag. The Rust `Email` struct **loses** its `folder_id` / `remote_id` / `uid_validity` / `read` scalars so the compiler flags every site that assumed a message lives in exactly one folder. Filter evaluation becomes per-placement, and fire-once becomes an atomic `(rule_id, message_key)` claim.

**Tech Stack:** Rust 2021 (toolchain 1.97.1), `sqlx` + SQLite/WAL, `sha2`, `tonic`/`prost`, `tokio`.

## Global Constraints

- **Branch:** `story/message-placement-split`, already created off `dev` with `story/message-id-capture` (#386) and `story/schema-version-and-token-scheme` (#387) merged in. Baseline gate is green on it (627+ tests). Do not rebase onto the #401–#403 chain.
- **Gate.** Removing `Email`'s folder scalars breaks four crates until Task 9 lands, so the gate is staged:
  - **Tasks 1–8:** `cargo fmt --all -- --check` **and** `cargo test -p <crate under change>` must pass. The workspace will not compile end-to-end during these tasks; that is expected and is not a reason to stop or to widen the task.
  - **Task 9 onward, and the PR head:** the full gate — `cargo fmt --all -- --check`, then `cargo check-all`, then `cargo test-all`.
  - Warnings are hard errors throughout; `unwrap_used` / `expect_used` / `panic` / `todo` are `deny` outside tests. Never silence a lint to get a commit through.
  - `cargo test-all` can exceed a 10-minute tool timeout on this workspace; allow ~540s.
- **Commits:** Conventional Commits, imperative, <72-char subject, **no AI attribution**. `Refs #394` in the message body is fine.
- **Comments:** explain intent, constraints, rationale. **No issue numbers, GH refs, or `Phase N` breadcrumbs in `.rs` / `.proto` comments.**
- **No live network in tests.** IMAP/JMAP/SMTP/CalDAV/CardDAV all mocked; keyring via `MockKeyring`. Ephemeral SQLite (`tempfile` / `:memory:`) per test.
- **Never key identity on a Message-ID alone.** Tier 3 requires `Message-ID` **and** `content_hash` both present; if either is missing, fall through to the surrogate tier. First-write-wins on the body.
- **`uidvalidity` is in the placement primary key.** UIDs restart at 1 after a UIDVALIDITY bump and would otherwise collide with old rows.
- **FTS plaintext-orphan rule.** Dropping *a* placement must NOT delete the `messages_fts` row; dropping the *last* placement must. Inverted, this leaks plaintext bodies in orphaned FTS rows.
- **No `.proto` schema changes in this plan.** Only a doc-comment correction on `Message.id`. Placement fields on the wire are #395's scope, and the repo allows one proto-touching story in flight at a time.

---

## File Structure

| File | Responsibility | Change |
| --- | --- | --- |
| `crates/nuncio-core/src/model.rs` | `Email` (identity + content), new `Placement`, new `MessageIdentity` / `IdentitySource`, `derive_message_key` | Modify |
| `crates/nuncio-store/src/db.rs` | `placements` + `filter_fired` schema, `IDENTITY_SCHEMA_VERSION` bump, all message reads/writes/deletes | Modify |
| `crates/nuncio-store/src/search.rs` | FTS hit mapping onto `message_key` | Modify |
| `crates/nuncio-filter/src/engine.rs` | per-placement condition evaluation | Modify |
| `crates/nuncio-filter/src/validator.rs` | `Folder` / `Account` field validation unchanged, comments corrected | Modify |
| `crates/nuncio-mail/src/imap.rs` | emit identity + placement instead of a surrogate-keyed `Email` | Modify |
| `crates/nuncio-mail/src/jmap.rs` | same | Modify |
| `crates/nuncio-mail/src/mock.rs` | mock backend emits placements | Modify |
| `crates/nunciod/src/sync.rs` | placement-aware new-vs-seen, per-placement filter fan-out, placement removals | Modify |
| `crates/nunciod/src/grpc.rs` | present a placement-in-context on the wire | Modify |
| `crates/nuncio-proto/proto/nuncio/v1/nuncio.proto` | `Message.id` doc comment only | Modify |

---

## Task 1: Core identity and placement types

**Files:**
- Modify: `crates/nuncio-core/src/model.rs`
- Test: `crates/nuncio-core/src/model.rs` (inline `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `Email::normalize_message_id`, `Email::content_hash_of` (both already exist from #386).
- Produces:
  - `pub enum IdentitySource { EmailId, GmailMsgId, MessageIdContent, Surrogate }` with `as_str(&self) -> &'static str` and `pub fn from_str(s: &str) -> Option<Self>`
  - `pub struct MessageIdentity { pub key: String, pub source: IdentitySource }`
  - `pub struct RemoteIdentity<'a> { pub email_id: Option<&'a str>, pub gm_msgid: Option<&'a str>, pub message_id: Option<&'a str>, pub content_hash: Option<&'a str> }`
  - `pub struct Placement { pub account_id: String, pub folder_id: String, pub uid_validity: String, pub remote_id: String, pub read: bool }`
  - `Email::derive_message_key(account_id: &str, remote: RemoteIdentity<'_>, folder_id: &str, uid_validity: &str, remote_id: &str) -> MessageIdentity`
  - `Email` keeps `id`, `account_id`, `subject`, `sender`, `recipient`, `received_at`, `body_plain`, `body_html`, `attachments`, `message_id`, `content_hash`; **loses** `folder_id`, `remote_id`, `uid_validity`, `read`.

- [ ] **Step 1: Write the failing tests**

Add to the existing `mod tests` in `crates/nuncio-core/src/model.rs`:

```rust
#[test]
fn message_key_precedence_prefers_emailid_over_everything_below_it() {
    let remote = RemoteIdentity {
        email_id: Some("M00000001"),
        gm_msgid: Some("1234567890"),
        message_id: Some("a@b.example"),
        content_hash: Some("deadbeef"),
    };
    let id = Email::derive_message_key("acct-1", remote, "INBOX", "42", "5");
    assert_eq!(id.source, IdentitySource::EmailId);

    // Same EMAILID in a different folder is the SAME message.
    let elsewhere = Email::derive_message_key(
        "acct-1",
        RemoteIdentity { email_id: Some("M00000001"), gm_msgid: None, message_id: None, content_hash: None },
        "Archive",
        "99",
        "7",
    );
    assert_eq!(id.key, elsewhere.key);
}

#[test]
fn message_key_is_account_scoped_at_every_tier() {
    let remote = || RemoteIdentity {
        email_id: Some("M00000001"),
        gm_msgid: None,
        message_id: None,
        content_hash: None,
    };
    let a = Email::derive_message_key("acct-1", remote(), "INBOX", "42", "5");
    let b = Email::derive_message_key("acct-2", remote(), "INBOX", "42", "5");
    assert_ne!(a.key, b.key, "the same EMAILID in two accounts must not collide");
}

#[test]
fn a_message_id_without_a_content_hash_never_reaches_the_content_tier() {
    // The non-negotiable: a Message-ID is public and forgeable, so it must
    // never key storage on its own. Missing content hash falls through to the
    // folder-scoped surrogate rather than trusting the header alone.
    let id = Email::derive_message_key(
        "acct-1",
        RemoteIdentity { email_id: None, gm_msgid: None, message_id: Some("a@b.example"), content_hash: None },
        "INBOX",
        "42",
        "5",
    );
    assert_eq!(id.source, IdentitySource::Surrogate);

    let surrogate_only = Email::derive_message_key(
        "acct-1",
        RemoteIdentity { email_id: None, gm_msgid: None, message_id: None, content_hash: None },
        "INBOX",
        "42",
        "5",
    );
    assert_eq!(id.key, surrogate_only.key);
}

#[test]
fn the_same_message_id_with_different_content_is_two_messages() {
    let mk = |hash: &str| {
        Email::derive_message_key(
            "acct-1",
            RemoteIdentity {
                email_id: None,
                gm_msgid: None,
                message_id: Some("a@b.example"),
                content_hash: Some(hash),
            },
            "INBOX",
            "42",
            "5",
        )
    };
    let list_copy = mk("1111");
    let direct_copy = mk("2222");
    assert_eq!(list_copy.source, IdentitySource::MessageIdContent);
    assert_ne!(list_copy.key, direct_copy.key);
}

#[test]
fn tiers_are_domain_separated_so_a_shared_value_cannot_collide_across_them() {
    let as_emailid = Email::derive_message_key(
        "acct-1",
        RemoteIdentity { email_id: Some("X"), gm_msgid: None, message_id: None, content_hash: None },
        "INBOX", "42", "5",
    );
    let as_gmsgid = Email::derive_message_key(
        "acct-1",
        RemoteIdentity { email_id: None, gm_msgid: Some("X"), message_id: None, content_hash: None },
        "INBOX", "42", "5",
    );
    assert_ne!(as_emailid.key, as_gmsgid.key);
}

#[test]
fn identity_source_round_trips_through_its_stored_string() {
    for source in [
        IdentitySource::EmailId,
        IdentitySource::GmailMsgId,
        IdentitySource::MessageIdContent,
        IdentitySource::Surrogate,
    ] {
        assert_eq!(IdentitySource::from_str(source.as_str()), Some(source));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p nuncio-core model::tests`
Expected: FAIL — `cannot find type RemoteIdentity`, `no function derive_message_key`.

- [ ] **Step 3: Implement the identity types**

In `crates/nuncio-core/src/model.rs`, add above `impl Email`:

```rust
/// Which tier of the identity precedence produced a [`MessageIdentity`].
///
/// Persisted alongside the key so a later pass can tell a server-assigned
/// identity from a locally-derived fallback without recomputing it, and so a
/// store can be audited for how much of it rests on the weakest tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdentitySource {
    /// RFC 8474 OBJECTID `EMAILID` -- server-assigned and stable across folders.
    EmailId,
    /// Gmail's `X-GM-MSGID`, the same guarantee by a vendor extension.
    GmailMsgId,
    /// `Message-ID` **and** a content hash together. Never the header alone.
    MessageIdContent,
    /// Folder-scoped last resort: the message is only identified by where it
    /// currently sits, so the same mail in another folder is a second message.
    Surrogate,
}

impl IdentitySource {
    /// Stable token persisted in `messages.identity_source`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::EmailId => "emailid",
            Self::GmailMsgId => "gmsgid",
            Self::MessageIdContent => "msgid_content",
            Self::Surrogate => "surrogate",
        }
    }

    /// Parse a token written by [`Self::as_str`]. Returns `None` for anything
    /// else so an unrecognised value fails loudly rather than defaulting to a
    /// tier that would overstate how trustworthy the key is.
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "emailid" => Some(Self::EmailId),
            "gmsgid" => Some(Self::GmailMsgId),
            "msgid_content" => Some(Self::MessageIdContent),
            "surrogate" => Some(Self::Surrogate),
            _ => None,
        }
    }
}

/// A content-addressed message key together with the tier that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageIdentity {
    /// Hex SHA-256 key identifying the message within its account.
    pub key: String,
    /// Which precedence tier produced `key`.
    pub source: IdentitySource,
}

/// Server-supplied identity hints for one message, in precedence order.
///
/// Borrowed rather than owned because every caller already holds these as
/// fields on a parsed response and only needs them for the duration of the
/// key derivation.
#[derive(Debug, Clone, Copy, Default)]
pub struct RemoteIdentity<'a> {
    /// RFC 8474 `EMAILID` from a `FETCH ... (EMAILID)`, if the server has OBJECTID.
    pub email_id: Option<&'a str>,
    /// Gmail `X-GM-MSGID`, if the server advertises `X-GM-EXT-1`.
    pub gm_msgid: Option<&'a str>,
    /// Normalized `Message-ID` (see [`Email::normalize_message_id`]).
    pub message_id: Option<&'a str>,
    /// Hex SHA-256 over the full RFC822 octets (see [`Email::content_hash_of`]).
    pub content_hash: Option<&'a str>,
}

/// One occupancy of a message in one mailbox.
///
/// A message can sit in several folders at once (an IMAP `COPY`, a Gmail label,
/// a message that is both in a thread's folder and in `\All`). The addressing
/// coordinates and the read flag are properties of the *occupancy*, not of the
/// message: IMAP `\Seen` is per-mailbox, and a UID is only meaningful inside one
/// `uid_validity` scope of one folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    /// Account owning the mailbox.
    pub account_id: String,
    /// Mailbox folder identifier (e.g. "inbox").
    pub folder_id: String,
    /// The IMAP UIDVALIDITY the `remote_id` was captured under, as a decimal
    /// string; a stable sentinel for protocols without one (JMAP). In the
    /// primary key because UIDs restart at 1 after a bump and would otherwise
    /// collide with rows written before it.
    pub uid_validity: String,
    /// Protocol-native id addressing the message in this mailbox: the IMAP UID
    /// as a decimal string, or the JMAP Email object id.
    pub remote_id: String,
    /// Read/unread state **in this mailbox**.
    pub read: bool,
}
```

- [ ] **Step 4: Implement `derive_message_key` and reshape `Email`**

Add to `impl Email`:

```rust
    /// Domain-separated, length-prefixed digest over an identity tier's inputs.
    ///
    /// The tier label participates in the hash so that a value appearing in two
    /// different tiers (a server whose `EMAILID` happens to equal another's
    /// `X-GM-MSGID`) cannot produce one key. Length prefixes stop two distinct
    /// field tuples serializing to the same byte stream, exactly as in
    /// [`Email::surrogate_id`].
    fn identity_digest(tier: &str, account_id: &str, parts: &[&str]) -> String {
        let mut hasher = Sha256::new();
        for field in [tier, account_id].into_iter().chain(parts.iter().copied()) {
            hasher.update((field.len() as u64).to_le_bytes());
            hasher.update(field.as_bytes());
        }
        hex::encode(hasher.finalize())
    }

    /// Derive the account-scoped message key by identity precedence.
    ///
    /// `EMAILID` → `X-GM-MSGID` → (`Message-ID` + content hash) → surrogate.
    /// Every tier is account-scoped: identity is only ever claimed within the
    /// account that fetched it, so one account's server cannot mint a key that
    /// addresses another account's mail.
    ///
    /// The third tier requires **both** a `Message-ID` and a content hash. A
    /// `Message-ID` is public -- it is echoed in the `References` of every reply
    /// -- and forgeable, so keying on it alone would let a crafted message
    /// address, and with an upsert overwrite, a message the user already trusts,
    /// and would evade filtering by never being classified as new. Pairing it
    /// with the content hash also gets the ordinary cases right: a mailing-list
    /// copy and a direct copy share a `Message-ID` but differ in content, and so
    /// are correctly two messages.
    ///
    /// The surrogate tier is folder-scoped and therefore *not* stable across a
    /// move; it is the honest answer when the server offered nothing better.
    pub fn derive_message_key(
        account_id: &str,
        remote: RemoteIdentity<'_>,
        folder_id: &str,
        uid_validity: &str,
        remote_id: &str,
    ) -> MessageIdentity {
        if let Some(email_id) = remote.email_id.filter(|v| !v.is_empty()) {
            return MessageIdentity {
                key: Self::identity_digest("emailid", account_id, &[email_id]),
                source: IdentitySource::EmailId,
            };
        }
        if let Some(gm_msgid) = remote.gm_msgid.filter(|v| !v.is_empty()) {
            return MessageIdentity {
                key: Self::identity_digest("gmsgid", account_id, &[gm_msgid]),
                source: IdentitySource::GmailMsgId,
            };
        }
        if let (Some(message_id), Some(content_hash)) = (
            remote.message_id.filter(|v| !v.is_empty()),
            remote.content_hash.filter(|v| !v.is_empty()),
        ) {
            return MessageIdentity {
                key: Self::identity_digest("msgid_content", account_id, &[message_id, content_hash]),
                source: IdentitySource::MessageIdContent,
            };
        }
        MessageIdentity {
            key: Self::identity_digest("surrogate", account_id, &[folder_id, uid_validity, remote_id]),
            source: IdentitySource::Surrogate,
        }
    }
```

Then delete the `folder_id`, `remote_id`, `uid_validity`, and `read` fields from `struct Email`, and replace the doc comment on `Email::id` with:

```rust
    /// Opaque, account-scoped identity for the message itself.
    ///
    /// Derived by [`Email::derive_message_key`], so it is independent of which
    /// folder the message currently occupies: moving a message between folders
    /// does not change it. Where the message actually sits, and its read state
    /// there, live in [`Placement`] rows instead. Callers must treat this as an
    /// opaque token -- it carries no decodable structure.
    pub id: String,
```

Keep `Email::surrogate_id` in place — the surrogate tier still uses the same
construction and existing tests cover it.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p nuncio-core model::tests`
Expected: PASS. Other crates will not compile yet; that is expected and is fixed in later tasks.

- [ ] **Step 6: Commit**

```bash
git add crates/nuncio-core/src/model.rs
git commit -m "feat(core): separate message identity from mailbox placement"
```

---

## Task 2: Store schema — placements, fire-once claims, and the version bump

**Files:**
- Modify: `crates/nuncio-store/src/db.rs` (`migrate`, `reset_cached_message_store`, `IDENTITY_SCHEMA_VERSION`)
- Test: `crates/nuncio-store/src/db.rs` inline tests

**Interfaces:**
- Consumes: `DatabaseEngine::reconcile_schema_version` and `reset_cached_message_store` (both from #387).
- Produces: `placements` and `filter_fired` tables; `messages` reshaped to `message_key` + identity columns; `IDENTITY_SCHEMA_VERSION == 2`; and three store methods —
  - `claim_filter_fire(&self, rule_id: &str, message_key: &str) -> Result<bool, DatabaseError>`
  - `has_filter_fired(&self, rule_id: &str, message_key: &str) -> Result<bool, DatabaseError>`
  - `filter_fire_count(&self) -> Result<i64, DatabaseError>`

  The last two are `pub` (not test-only) because `nunciod` asserts on the ledger from another crate and cannot reach the pool.

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn placements_primary_key_includes_uidvalidity() {
    let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();

    // Same folder, same UID, different UIDVALIDITY: two distinct placements.
    // After a UIDVALIDITY bump the server restarts UIDs at 1, so without
    // uidvalidity in the key the new mail would overwrite the old.
    for validity in ["42", "43"] {
        sqlx::query(
            "INSERT INTO placements (account_id, folder_id, uidvalidity, uid, message_key, read_flag)
             VALUES (?, ?, ?, ?, ?, 0)",
        )
        .bind("acct-1")
        .bind("INBOX")
        .bind(validity)
        .bind("1")
        .bind(format!("key-{validity}"))
        .execute(engine.pool())
        .await
        .expect("both placements must insert");
    }

    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM placements")
        .fetch_one(engine.pool())
        .await
        .unwrap();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn a_filter_fired_claim_is_won_exactly_once() {
    let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
    assert!(engine.claim_filter_fire("rule-1", "key-1").await.unwrap());
    assert!(
        !engine.claim_filter_fire("rule-1", "key-1").await.unwrap(),
        "a second claim on the same (rule, message) must lose"
    );
    assert!(
        engine.claim_filter_fire("rule-2", "key-1").await.unwrap(),
        "a different rule still gets its own claim"
    );
}

#[tokio::test]
async fn bumping_the_identity_schema_rebuilds_placements_and_claims_too() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("nuncio.db");
    let secrets = crate::vault::MockKeyring::new();

    {
        let engine = DatabaseEngine::connect_file(&db_path, &secrets).await.unwrap();
        sqlx::query(
            "INSERT INTO placements (account_id, folder_id, uidvalidity, uid, message_key, read_flag)
             VALUES ('a', 'INBOX', '1', '1', 'k', 0)",
        )
        .execute(engine.pool())
        .await
        .unwrap();
        engine.claim_filter_fire("rule-1", "k").await.unwrap();
        // Pretend this file was written by the previous identity generation.
        sqlx::query("PRAGMA user_version = 1").execute(engine.pool()).await.unwrap();
        engine.close().await;
    }

    let engine = DatabaseEngine::connect_file(&db_path, &secrets).await.unwrap();
    let (placements,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM placements")
        .fetch_one(engine.pool())
        .await
        .unwrap();
    let (claims,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM filter_fired")
        .fetch_one(engine.pool())
        .await
        .unwrap();
    assert_eq!(placements, 0, "placements are derived from the server and must be rebuilt");
    assert_eq!(claims, 0, "fire-once claims name message keys that no longer resolve");
    engine.close().await;
}
```

If `DatabaseEngine` has no `pool()` accessor, add one gated to the crate:

```rust
    /// Test-only access to the underlying pool for schema-level assertions.
    #[cfg(test)]
    pub(crate) fn pool(&self) -> &sqlx::SqlitePool {
        &self.pool
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p nuncio-store db::tests::placements_primary_key_includes_uidvalidity db::tests::a_filter_fired_claim_is_won_exactly_once`
Expected: FAIL — `no such table: placements`, `no method claim_filter_fire`.

- [ ] **Step 3: Add the schema**

In `DatabaseEngine::migrate`, replace the `CREATE TABLE IF NOT EXISTS messages (...)` block with:

```sql
            -- Identity and immutable content, one row per message. Where the
            -- message sits lives in `placements`; nothing here is folder-scoped.
            CREATE TABLE IF NOT EXISTS messages (
                message_key TEXT PRIMARY KEY NOT NULL,
                account_id TEXT NOT NULL,
                subject TEXT NOT NULL,
                sender TEXT NOT NULL,
                recipient TEXT NOT NULL,
                received_at INTEGER NOT NULL,
                body_plain TEXT,
                body_html TEXT,
                message_id TEXT,
                content_hash TEXT,
                identity_source TEXT NOT NULL DEFAULT 'surrogate'
            );

            -- One row per (mailbox, message) occupancy. `uidvalidity` is in the
            -- primary key because UIDs restart at 1 after a bump: without it the
            -- first message of the new generation would overwrite the row of
            -- whichever old message shared its UID.
            CREATE TABLE IF NOT EXISTS placements (
                account_id TEXT NOT NULL,
                folder_id TEXT NOT NULL,
                uidvalidity TEXT NOT NULL,
                uid TEXT NOT NULL,
                message_key TEXT NOT NULL,
                read_flag INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (account_id, folder_id, uidvalidity, uid)
            );

            -- Fire-once ledger for filter actions. Keyed on the message
            -- *identity*, not a placement, so a message arriving in a second
            -- folder does not re-fire a rule that already acted on it -- which
            -- matters most for FORWARD and CALL WEBHOOK, whose effects land on
            -- third parties and cannot be undone by convergence.
            CREATE TABLE IF NOT EXISTS filter_fired (
                rule_id TEXT NOT NULL,
                message_key TEXT NOT NULL,
                fired_at INTEGER NOT NULL,
                PRIMARY KEY (rule_id, message_key)
            );
```

Add alongside the existing indexes:

```sql
            CREATE INDEX IF NOT EXISTS idx_placements_message ON placements(message_key);
            CREATE INDEX IF NOT EXISTS idx_placements_folder ON placements(account_id, folder_id);
```

The `messages_ad` trigger stays exactly as it is. Update only its comment to
record why it must remain on `messages`:

```sql
            -- Subject/sender are never encrypted in `messages`, so a delete-only trigger is
            -- sufficient here: it just keeps the FTS index free of orphaned rows. Insertion and
            -- update of `messages_fts` content happens explicitly in `upsert_message`, never via an
            -- AFTER INSERT/UPDATE trigger, because such a trigger would only ever see the
            -- ciphertext body column.
            --
            -- This trigger MUST stay on `messages` and never move to `placements`. Dropping one
            -- placement of a message that still sits in another folder must leave the body
            -- searchable; only losing the last placement removes the `messages` row, and that is
            -- what reaps the FTS row here. Firing on `placements` instead would delete the index
            -- entry while the body remained -- and, inverted, leaving it off `messages` entirely
            -- would strand plaintext bodies in `messages_fts` after the message itself was gone.
            CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN
                DELETE FROM messages_fts WHERE message_key = old.message_key;
            END;
```

Change the FTS virtual table's key column to match:

```sql
            CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
                message_key UNINDEXED,
                subject,
                sender,
                body_plain,
                tokenize = 'trigram'
            );
```

- [ ] **Step 4: Bump the version and extend the reset**

```rust
    pub const IDENTITY_SCHEMA_VERSION: i64 = 2;
```

Extend the statement list inside `reset_cached_message_store` to:

```rust
        for statement in [
            "DELETE FROM messages",
            "DELETE FROM messages_fts",
            "DELETE FROM placements",
            "DELETE FROM filter_fired",
            "DELETE FROM folder_sync_state",
            "DELETE FROM filter_execution_logs",
        ] {
```

and add to its doc comment, in the "Cleared" sentence, `mailbox placements` and
`the filter fire-once ledger`.

Apply the same additions to the `for statement in [...]` list in the
existing `clear_cached_message_data`-style helper at `db.rs:596` if it is a
separate function.

- [ ] **Step 5: Add `claim_filter_fire`**

```rust
    /// Atomically claim the right to run `rule_id`'s actions against
    /// `message_key`, returning `true` only for the claim that won.
    ///
    /// The claim is the fire-once guard for side-effecting actions. It is keyed
    /// on message identity rather than on a placement because the same message
    /// legitimately arrives in several folders -- and, under the old
    /// folder-scoped identity, each arrival looked like a different message and
    /// fired the rule again. `INSERT ... ON CONFLICT DO NOTHING` makes winning
    /// the claim a single atomic statement, so two passes racing over the same
    /// message cannot both act.
    pub async fn claim_filter_fire(
        &self,
        rule_id: &str,
        message_key: &str,
    ) -> Result<bool, DatabaseError> {
        let result = sqlx::query(
            "INSERT INTO filter_fired (rule_id, message_key, fired_at) VALUES (?, ?, ?)
             ON CONFLICT (rule_id, message_key) DO NOTHING",
        )
        .bind(rule_id)
        .bind(message_key)
        .bind(chrono::Utc::now().timestamp())
        .execute(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        Ok(result.rows_affected() == 1)
    }

    /// Whether `rule_id` has already acted on `message_key`.
    ///
    /// Public rather than test-only: the claim ledger answers a question
    /// operators and the outbox both have a real reason to ask -- "did this rule
    /// already act on this message" -- and callers in other crates cannot reach
    /// the pool directly.
    pub async fn has_filter_fired(
        &self,
        rule_id: &str,
        message_key: &str,
    ) -> Result<bool, DatabaseError> {
        let found: Option<(i64,)> = sqlx::query_as(
            "SELECT 1 FROM filter_fired WHERE rule_id = ? AND message_key = ?",
        )
        .bind(rule_id)
        .bind(message_key)
        .fetch_optional(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        Ok(found.is_some())
    }

    /// How many distinct (rule, message) fires the ledger records.
    ///
    /// Counterpart to [`Self::has_filter_fired`] for callers that need to assert
    /// on the ledger as a whole rather than one entry.
    pub async fn filter_fire_count(&self) -> Result<i64, DatabaseError> {
        let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM filter_fired")
            .fetch_one(&self.pool)
            .await
            .map_err(DatabaseError::Query)?;
        Ok(count)
    }
```

Use whichever clock helper `db.rs` already uses for `created_at`/`matched_at`
rather than introducing a second one.

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p nuncio-store db::tests::placements_primary_key_includes_uidvalidity db::tests::a_filter_fired_claim_is_won_exactly_once db::tests::bumping_the_identity_schema_rebuilds`
Expected: PASS. The rest of `nuncio-store` will not compile until Task 3.

- [ ] **Step 7: Commit**

```bash
git add crates/nuncio-store/src/db.rs
git commit -m "feat(store): add placements, fire-once claims, and bump identity schema"
```

---

## Task 3: Store writes — upsert message, upsert placement, first-write-wins

**Files:**
- Modify: `crates/nuncio-store/src/db.rs` (`save_email` → `upsert_message` + `upsert_placement`, `backfill_message_fts`)
- Test: `crates/nuncio-store/src/db.rs` inline tests

**Interfaces:**
- Consumes: `Email` and `Placement` from Task 1; `placements` schema from Task 2.
- Produces:
  - `DatabaseEngine::upsert_message(&self, email: &Email, source: IdentitySource) -> Result<bool, DatabaseError>` — `true` when the row was newly inserted
  - `DatabaseEngine::upsert_placement(&self, message_key: &str, placement: &Placement) -> Result<bool, DatabaseError>` — `true` when the placement was newly inserted
  - `DatabaseEngine::save_email_at(&self, email: &Email, source: IdentitySource, placement: &Placement) -> Result<SaveOutcome, DatabaseError>` where `pub struct SaveOutcome { pub message_is_new: bool, pub placement_is_new: bool }`

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn a_second_placement_does_not_overwrite_the_stored_body() {
    let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
    let email = nuncio_core::model::Email {
        id: "key-1".into(),
        account_id: "acct-1".into(),
        subject: "Original".into(),
        sender: "a@nuncio.mx".into(),
        recipient: "b@nuncio.mx".into(),
        received_at: 1_000,
        body_plain: Some("the trusted body".into()),
        body_html: None,
        attachments: Vec::new(),
        message_id: Some("a@b.example".into()),
        content_hash: Some("1111".into()),
    };
    let inbox = nuncio_core::model::Placement {
        account_id: "acct-1".into(),
        folder_id: "INBOX".into(),
        uid_validity: "42".into(),
        remote_id: "5".into(),
        read: false,
    };
    let outcome = engine
        .save_email_at(&email, nuncio_core::model::IdentitySource::MessageIdContent, &inbox)
        .await
        .unwrap();
    assert!(outcome.message_is_new && outcome.placement_is_new);

    // The same key arriving from another folder, carrying a different body.
    let impostor = nuncio_core::model::Email { subject: "Replaced".into(), body_plain: Some("attacker body".into()), ..email.clone() };
    let archive = nuncio_core::model::Placement { folder_id: "Archive".into(), remote_id: "9".into(), ..inbox.clone() };
    let outcome = engine
        .save_email_at(&impostor, nuncio_core::model::IdentitySource::MessageIdContent, &archive)
        .await
        .unwrap();
    assert!(!outcome.message_is_new, "identity already existed");
    assert!(outcome.placement_is_new, "but the Archive placement is new");

    let stored = engine.get_message("key-1").await.unwrap();
    assert_eq!(stored.subject, "Original", "first write must win");
    assert_eq!(stored.body_plain.as_deref(), Some("the trusted body"));
}

#[tokio::test]
async fn re_placing_the_same_message_leaves_exactly_one_fts_row() {
    let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
    let (email, inbox) = sample_message_and_placement("key-1", "INBOX", "5");
    engine.save_email_at(&email, nuncio_core::model::IdentitySource::EmailId, &inbox).await.unwrap();
    let archive = nuncio_core::model::Placement { folder_id: "Archive".into(), remote_id: "9".into(), ..inbox.clone() };
    engine.save_email_at(&email, nuncio_core::model::IdentitySource::EmailId, &archive).await.unwrap();

    let (rows,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM messages_fts WHERE message_key = 'key-1'")
        .fetch_one(engine.pool())
        .await
        .unwrap();
    assert_eq!(rows, 1, "the FTS index holds one row per message, not per placement");
}

#[tokio::test]
async fn read_state_is_tracked_per_placement() {
    let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
    let (email, inbox) = sample_message_and_placement("key-1", "INBOX", "5");
    let archive = nuncio_core::model::Placement { folder_id: "Archive".into(), remote_id: "9".into(), read: true, ..inbox.clone() };
    engine.save_email_at(&email, nuncio_core::model::IdentitySource::EmailId, &inbox).await.unwrap();
    engine.save_email_at(&email, nuncio_core::model::IdentitySource::EmailId, &archive).await.unwrap();

    let placements = engine.placements_of("key-1").await.unwrap();
    let inbox_read = placements.iter().find(|p| p.folder_id == "INBOX").unwrap().read;
    let archive_read = placements.iter().find(|p| p.folder_id == "Archive").unwrap().read;
    assert!(!inbox_read);
    assert!(archive_read, "IMAP \\Seen is per-mailbox, so the flags differ");
}
```

Add this test helper next to the tests:

```rust
    fn sample_message_and_placement(
        key: &str,
        folder: &str,
        uid: &str,
    ) -> (nuncio_core::model::Email, nuncio_core::model::Placement) {
        (
            nuncio_core::model::Email {
                id: key.into(),
                account_id: "acct-1".into(),
                subject: "Subject".into(),
                sender: "a@nuncio.mx".into(),
                recipient: "b@nuncio.mx".into(),
                received_at: 1_000,
                body_plain: Some("body".into()),
                body_html: None,
                attachments: Vec::new(),
                message_id: None,
                content_hash: None,
            },
            nuncio_core::model::Placement {
                account_id: "acct-1".into(),
                folder_id: folder.into(),
                uid_validity: "42".into(),
                remote_id: uid.into(),
                read: false,
            },
        )
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p nuncio-store db::tests::a_second_placement_does_not_overwrite`
Expected: FAIL — `no method save_email_at`.

- [ ] **Step 3: Implement the write path**

Replace `save_email` with:

```rust
/// What a [`DatabaseEngine::save_email_at`] call actually changed.
///
/// The two flags answer different questions and a caller usually needs both:
/// `message_is_new` gates work that should happen once per message (indexing,
/// side-effecting filter actions), `placement_is_new` gates work that should
/// happen once per mailbox occupancy (per-folder counters, per-placement
/// evaluation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaveOutcome {
    /// The message identity had not been stored before this call.
    pub message_is_new: bool,
    /// This mailbox occupancy had not been stored before this call.
    pub placement_is_new: bool,
}

impl DatabaseEngine {
    /// Persist a message's identity and content, and record that it occupies
    /// `placement`, in one transaction.
    ///
    /// The message row is **first-write-wins**: a second sighting of the same
    /// key -- which is the normal case for a message that also exists in another
    /// folder -- leaves the stored subject and body untouched. That is what
    /// keeps a message whose identity rests on `Message-ID` + content hash from
    /// being rewritten by a later fetch claiming the same key, and it is why the
    /// FTS row is written only on the insert that actually created the message.
    ///
    /// The placement row is upserted rather than ignored, because its read flag
    /// is genuinely mutable: `\Seen` changes in the mailbox and the store must
    /// follow it.
    pub async fn save_email_at(
        &self,
        email: &nuncio_core::model::Email,
        source: nuncio_core::model::IdentitySource,
        placement: &nuncio_core::model::Placement,
    ) -> Result<SaveOutcome, DatabaseError> {
        // Encryption failures MUST surface before any SQL runs: a swallowed error here would
        // otherwise leave a row with an empty/placeholder body column, indistinguishable from
        // a genuinely empty body on read-back.
        let enc_plain = email
            .body_plain
            .as_ref()
            .map(|p| crate::cipher::PayloadCipher::encrypt_text_at_rest(&self.storage_key, p))
            .transpose()?;
        let enc_html = email
            .body_html
            .as_ref()
            .map(|h| crate::cipher::PayloadCipher::encrypt_text_at_rest(&self.storage_key, h))
            .transpose()?;

        let mut tx = self.pool.begin().await.map_err(DatabaseError::Query)?;

        // Ask before writing. SQLite reports one row affected for both arms of
        // an upsert, so after the fact there is no way to tell an inserted
        // placement from an updated one -- and the caller needs that distinction
        // to decide whether this is a first arrival worth filtering.
        let (already_placed,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM placements
             WHERE account_id = ? AND folder_id = ? AND uidvalidity = ? AND uid = ?",
        )
        .bind(&placement.account_id)
        .bind(&placement.folder_id)
        .bind(&placement.uid_validity)
        .bind(&placement.remote_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(DatabaseError::Query)?;
        let placement_is_new = already_placed == 0;

        let inserted = sqlx::query(
            r#"
            INSERT INTO messages
            (message_key, account_id, subject, sender, recipient, received_at,
             body_plain, body_html, message_id, content_hash, identity_source)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT (message_key) DO NOTHING
            "#,
        )
        .bind(&email.id)
        .bind(&email.account_id)
        .bind(&email.subject)
        .bind(&email.sender)
        .bind(&email.recipient)
        .bind(email.received_at)
        .bind(&enc_plain)
        .bind(&enc_html)
        .bind(&email.message_id)
        .bind(&email.content_hash)
        .bind(source.as_str())
        .execute(&mut *tx)
        .await
        .map_err(DatabaseError::Query)?;

        let message_is_new = inserted.rows_affected() == 1;

        // Index only on the insert that created the message. Re-indexing on
        // every placement would either duplicate the row (FTS5 standalone tables
        // have no upsert) or rewrite it with a body the first-write-wins rule
        // just rejected.
        if message_is_new {
            sqlx::query(
                "INSERT INTO messages_fts (message_key, subject, sender, body_plain) VALUES (?, ?, ?, ?)",
            )
            .bind(&email.id)
            .bind(&email.subject)
            .bind(&email.sender)
            .bind(email.body_plain.as_deref().unwrap_or(""))
            .execute(&mut *tx)
            .await
            .map_err(DatabaseError::Query)?;
        }

        sqlx::query(
            r#"
            INSERT INTO placements
            (account_id, folder_id, uidvalidity, uid, message_key, read_flag)
            VALUES (?, ?, ?, ?, ?, ?)
            ON CONFLICT (account_id, folder_id, uidvalidity, uid)
            DO UPDATE SET message_key = excluded.message_key, read_flag = excluded.read_flag
            "#,
        )
        .bind(&placement.account_id)
        .bind(&placement.folder_id)
        .bind(&placement.uid_validity)
        .bind(&placement.remote_id)
        .bind(&email.id)
        .bind(if placement.read { 1i64 } else { 0i64 })
        .execute(&mut *tx)
        .await
        .map_err(DatabaseError::Query)?;

        tx.commit().await.map_err(DatabaseError::Query)?;

        Ok(SaveOutcome { message_is_new, placement_is_new })
    }
}
```

Add the placement reader used by the tests:

```rust
    /// Every mailbox this message currently occupies.
    pub async fn placements_of(
        &self,
        message_key: &str,
    ) -> Result<Vec<nuncio_core::model::Placement>, DatabaseError> {
        let rows: Vec<(String, String, String, String, i64)> = sqlx::query_as(
            "SELECT account_id, folder_id, uidvalidity, uid, read_flag
             FROM placements WHERE message_key = ? ORDER BY folder_id ASC",
        )
        .bind(message_key)
        .fetch_all(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        Ok(rows
            .into_iter()
            .map(|(account_id, folder_id, uid_validity, remote_id, read_flag)| {
                nuncio_core::model::Placement {
                    account_id,
                    folder_id,
                    uid_validity,
                    remote_id,
                    read: read_flag != 0,
                }
            })
            .collect())
    }
```

Update `backfill_message_fts` to select `message_key` instead of `id` and to
join nothing — it stays a `messages`-only backfill.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p nuncio-store db::tests`
Expected: the three new tests PASS. Pre-existing `save_email` tests fail to compile — fix them in Task 4 as part of the read-path migration.

- [ ] **Step 5: Commit**

```bash
git add crates/nuncio-store/src/db.rs
git commit -m "feat(store): write message identity and placement separately"
```

---

## Task 4: Store reads — placement-aware queries and folder derivation

**Files:**
- Modify: `crates/nuncio-store/src/db.rs` (`get_message`, `list_messages`, `list_messages_page`, `message_ids_in_folder`, `set_message_read`, `list_folders`, `list_folders_page`, unread count at `db.rs:471`)
- Modify: `crates/nuncio-store/src/search.rs`
- Test: `crates/nuncio-store/src/db.rs` inline tests

**Interfaces:**
- Consumes: `placements_of`, `SaveOutcome` from Task 3.
- Produces:
  - `get_message(&self, message_key: &str) -> Result<Email, DatabaseError>` (identity only, no folder scalars)
  - `list_messages(&self, account_id: &str, folder_id: &str, limit: usize) -> Result<Vec<(Email, Placement)>, DatabaseError>`
  - `existing_placements(&self, keys: &[PlacementKey]) -> Result<HashSet<PlacementKey>, DatabaseError>` with:

    ```rust
    /// The four coordinates that address one mailbox occupancy.
    ///
    /// `Ord` is derived because the sync path sorts and dedups batches of these
    /// before deleting; `Hash`/`Eq` because it is also the key of the
    /// already-present set used to classify a fetched chunk.
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct PlacementKey {
        pub account_id: String,
        pub folder_id: String,
        pub uid_validity: String,
        pub remote_id: String,
    }
    ```
- Test helper: the tests below reuse `sample_message_and_placement`, added to `db.rs`'s test module in Task 3 Step 1. It returns an `(Email, Placement)` pair for `account_id = "acct-1"`, `uid_validity = "42"`.
  - `set_placement_read(&self, key: &PlacementKey, read: bool) -> Result<(), DatabaseError>`
  - `list_folders(&self, account_id: &str) -> Result<Vec<Folder>, DatabaseError>` — now account-scoped

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn folders_are_derived_from_placements_and_scoped_per_account() {
    let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
    for (account, folder, uid) in [("acct-1", "INBOX", "1"), ("acct-1", "Archive", "2"), ("acct-2", "INBOX", "1")] {
        let (mut email, mut placement) = sample_message_and_placement(&format!("key-{account}-{folder}"), folder, uid);
        email.account_id = account.into();
        placement.account_id = account.into();
        engine.save_email_at(&email, nuncio_core::model::IdentitySource::EmailId, &placement).await.unwrap();
    }

    let folders = engine.list_folders("acct-1").await.unwrap();
    let names: Vec<&str> = folders.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(names, vec!["Archive", "INBOX"]);
    assert!(
        !folders.iter().any(|f| f.total_messages > 1),
        "acct-2's INBOX must not be folded into acct-1's"
    );
}

#[tokio::test]
async fn one_message_in_two_folders_counts_once_in_each() {
    let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
    let (email, inbox) = sample_message_and_placement("key-1", "INBOX", "5");
    let archive = nuncio_core::model::Placement { folder_id: "Archive".into(), remote_id: "9".into(), ..inbox.clone() };
    engine.save_email_at(&email, nuncio_core::model::IdentitySource::EmailId, &inbox).await.unwrap();
    engine.save_email_at(&email, nuncio_core::model::IdentitySource::EmailId, &archive).await.unwrap();

    let folders = engine.list_folders("acct-1").await.unwrap();
    for folder in &folders {
        assert_eq!(folder.total_messages, 1, "{} should hold the message once", folder.id);
    }
    let (messages,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM messages").fetch_one(engine.pool()).await.unwrap();
    assert_eq!(messages, 1, "but it is still one message");
}

#[tokio::test]
async fn existing_placements_reports_the_occupancy_not_the_message() {
    let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
    let (email, inbox) = sample_message_and_placement("key-1", "INBOX", "5");
    engine.save_email_at(&email, nuncio_core::model::IdentitySource::EmailId, &inbox).await.unwrap();

    let in_inbox = PlacementKey { account_id: "acct-1".into(), folder_id: "INBOX".into(), uid_validity: "42".into(), remote_id: "5".into() };
    let in_archive = PlacementKey { folder_id: "Archive".into(), remote_id: "9".into(), ..in_inbox.clone() };

    let found = engine.existing_placements(&[in_inbox.clone(), in_archive.clone()]).await.unwrap();
    assert!(found.contains(&in_inbox));
    assert!(
        !found.contains(&in_archive),
        "the same message arriving in a new folder is a new placement and must be seen as such"
    );
}

#[tokio::test]
async fn marking_read_in_one_folder_leaves_the_other_unread() {
    let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
    let (email, inbox) = sample_message_and_placement("key-1", "INBOX", "5");
    let archive = nuncio_core::model::Placement { folder_id: "Archive".into(), remote_id: "9".into(), ..inbox.clone() };
    engine.save_email_at(&email, nuncio_core::model::IdentitySource::EmailId, &inbox).await.unwrap();
    engine.save_email_at(&email, nuncio_core::model::IdentitySource::EmailId, &archive).await.unwrap();

    let inbox_key = PlacementKey { account_id: "acct-1".into(), folder_id: "INBOX".into(), uid_validity: "42".into(), remote_id: "5".into() };
    engine.set_placement_read(&inbox_key, true).await.unwrap();

    let placements = engine.placements_of("key-1").await.unwrap();
    assert!(placements.iter().find(|p| p.folder_id == "INBOX").unwrap().read);
    assert!(!placements.iter().find(|p| p.folder_id == "Archive").unwrap().read);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p nuncio-store db::tests::folders_are_derived_from_placements`
Expected: FAIL — `list_folders` takes no arguments; `PlacementKey` undefined.

- [ ] **Step 3: Implement the read path**

`get_message` — drop the folder scalars from both the `SELECT` and the
constructed `Email`:

```rust
    /// Retrieve a message's identity and content by its key.
    ///
    /// Returns the message alone. Where it sits, and whether it is read there,
    /// are per-mailbox facts -- ask [`Self::placements_of`] for those.
    pub async fn get_message(
        &self,
        message_key: &str,
    ) -> Result<nuncio_core::model::Email, DatabaseError> {
        let row: (String, String, String, String, String, i64, Option<String>, Option<String>, Option<String>, Option<String>) =
            sqlx::query_as(
                r#"
                SELECT message_key, account_id, subject, sender, recipient, received_at,
                       body_plain, body_html, message_id, content_hash
                FROM messages
                WHERE message_key = ?
                "#,
            )
            .bind(message_key)
            .fetch_one(&self.pool)
            .await
            .map_err(DatabaseError::Query)?;

        let dec_plain = row
            .6
            .map(|p| crate::cipher::PayloadCipher::decrypt_text_at_rest(&self.storage_key, &p))
            .transpose()
            .map_err(|e| DatabaseError::Decryption(e.to_string()))?;
        let dec_html = row
            .7
            .map(|h| crate::cipher::PayloadCipher::decrypt_text_at_rest(&self.storage_key, &h))
            .transpose()
            .map_err(|e| DatabaseError::Decryption(e.to_string()))?;

        Ok(nuncio_core::model::Email {
            id: row.0,
            account_id: row.1,
            subject: row.2,
            sender: row.3,
            recipient: row.4,
            received_at: row.5,
            body_plain: dec_plain,
            body_html: dec_html,
            attachments: Vec::new(),
            message_id: row.8,
            content_hash: row.9,
        })
    }
```

`list_messages` and `list_messages_page` join through `placements` and return
pairs. Both gain an `account_id` parameter for the same reason
`message_ids_in_folder` already has one — folder ids are not globally unique:

```rust
    /// Messages occupying one folder of one account, newest first, paired with
    /// the placement they were found through.
    ///
    /// The placement travels with the message because the caller almost always
    /// needs the per-mailbox facts -- the read flag and the addressing UID --
    /// and re-deriving them would mean a second query per row.
    #[allow(clippy::type_complexity)]
    pub async fn list_messages(
        &self,
        account_id: &str,
        folder_id: &str,
        limit: usize,
    ) -> Result<Vec<(nuncio_core::model::Email, nuncio_core::model::Placement)>, DatabaseError> {
        let rows: Vec<(
            String,
            String,
            String,
            String,
            String,
            i64,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            String,
            String,
            i64,
        )> = sqlx::query_as(
            r#"
            SELECT m.message_key, m.account_id, m.subject, m.sender, m.recipient,
                   m.received_at, m.body_plain, m.body_html, m.message_id, m.content_hash,
                   p.uidvalidity, p.uid, p.read_flag
            FROM messages m
            JOIN placements p ON p.message_key = m.message_key
            WHERE p.account_id = ? AND p.folder_id = ?
            ORDER BY m.received_at DESC
            LIMIT ?
            "#,
        )
        .bind(account_id)
        .bind(folder_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let dec_plain = row
                .6
                .map(|p| crate::cipher::PayloadCipher::decrypt_text_at_rest(&self.storage_key, &p))
                .transpose()
                .map_err(|e| DatabaseError::Decryption(e.to_string()))?;
            let dec_html = row
                .7
                .map(|h| crate::cipher::PayloadCipher::decrypt_text_at_rest(&self.storage_key, &h))
                .transpose()
                .map_err(|e| DatabaseError::Decryption(e.to_string()))?;

            out.push((
                nuncio_core::model::Email {
                    id: row.0,
                    account_id: row.1,
                    subject: row.2,
                    sender: row.3,
                    recipient: row.4,
                    received_at: row.5,
                    body_plain: dec_plain,
                    body_html: dec_html,
                    attachments: Vec::new(),
                    message_id: row.8,
                    content_hash: row.9,
                },
                nuncio_core::model::Placement {
                    account_id: account_id.to_string(),
                    folder_id: folder_id.to_string(),
                    uid_validity: row.10,
                    remote_id: row.11,
                    read: row.12 != 0,
                },
            ));
        }
        Ok(out)
    }
```

`list_messages_page` takes the same join and adds the existing keyset cursor
over `m.received_at`, fetching `page_size + 1` rows to detect a following page
exactly as it does today.

`message_ids_in_folder` selects from `placements` and returns placement keys, since
that is what a UID-set diff subtracts:

```rust
    /// Every placement currently stored for one folder of one account.
    ///
    /// The local half of a UID-set diff. This returns placements rather than
    /// message keys because what a server stops reporting is an occupancy, not a
    /// message: the same mail may still exist in another folder and must survive
    /// there.
    pub async fn placements_in_folder(
        &self,
        account_id: &str,
        folder_id: &str,
    ) -> Result<Vec<PlacementKey>, DatabaseError> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT uidvalidity, uid FROM placements WHERE account_id = ? AND folder_id = ?",
        )
        .bind(account_id)
        .bind(folder_id)
        .fetch_all(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        Ok(rows
            .into_iter()
            .map(|(uid_validity, remote_id)| PlacementKey {
                account_id: account_id.to_string(),
                folder_id: folder_id.to_string(),
                uid_validity,
                remote_id,
            })
            .collect())
    }
```

`existing_placements` mirrors `existing_message_ids`, batching with a tuple
`IN` clause:

```rust
    /// The subset of `keys` already stored, in one round trip.
    ///
    /// SQLite supports row-value `IN` lists, so the whole batch is one query
    /// rather than one existence check per placement. An empty slice
    /// short-circuits, since `IN ()` is not valid SQL.
    pub async fn existing_placements(
        &self,
        keys: &[PlacementKey],
    ) -> Result<std::collections::HashSet<PlacementKey>, DatabaseError> {
        if keys.is_empty() {
            return Ok(std::collections::HashSet::new());
        }

        let mut builder: sqlx::QueryBuilder<'_, sqlx::Sqlite> = sqlx::QueryBuilder::new(
            "SELECT account_id, folder_id, uidvalidity, uid FROM placements
             WHERE (account_id, folder_id, uidvalidity, uid) IN (",
        );
        let mut separated = builder.separated(", ");
        for key in keys {
            separated.push("(");
            separated.push_bind_unseparated(key.account_id.clone());
            separated.push_unseparated(", ");
            separated.push_bind_unseparated(key.folder_id.clone());
            separated.push_unseparated(", ");
            separated.push_bind_unseparated(key.uid_validity.clone());
            separated.push_unseparated(", ");
            separated.push_bind_unseparated(key.remote_id.clone());
            separated.push_unseparated(")");
        }
        builder.push(")");

        let rows: Vec<(String, String, String, String)> = builder
            .build_query_as()
            .fetch_all(&self.pool)
            .await
            .map_err(DatabaseError::Query)?;

        Ok(rows
            .into_iter()
            .map(|(account_id, folder_id, uid_validity, remote_id)| PlacementKey {
                account_id,
                folder_id,
                uid_validity,
                remote_id,
            })
            .collect())
    }
```

> **Implementer note:** if `QueryBuilder`'s separated API makes the row-value
> list awkward, build the `IN` list with a plain loop over
> `builder.push("(").push_bind(..)` calls instead — the shape of the SQL is what
> matters, not the builder idiom. Verify the generated SQL against the test
> before moving on.

`set_message_read` becomes `set_placement_read`, updating `placements` by its
full primary key and keeping the `RowNotFound` contract:

```rust
    /// Update one placement's read flag in place.
    ///
    /// Scoped to a single mailbox occupancy because IMAP `\Seen` is per-mailbox:
    /// a message read in `INBOX` is not thereby read in `Archive`. Returns
    /// `DatabaseError::Query(sqlx::Error::RowNotFound)` when no such placement
    /// exists, so callers can still distinguish "flag flipped" from "never
    /// there" rather than silently succeeding on a no-op update.
    pub async fn set_placement_read(
        &self,
        key: &PlacementKey,
        read: bool,
    ) -> Result<(), DatabaseError> {
        let result = sqlx::query(
            "UPDATE placements SET read_flag = ?
             WHERE account_id = ? AND folder_id = ? AND uidvalidity = ? AND uid = ?",
        )
        .bind(if read { 1i64 } else { 0i64 })
        .bind(&key.account_id)
        .bind(&key.folder_id)
        .bind(&key.uid_validity)
        .bind(&key.remote_id)
        .execute(&self.pool)
        .await
        .map_err(DatabaseError::Query)?;

        if result.rows_affected() == 0 {
            return Err(DatabaseError::Query(sqlx::Error::RowNotFound));
        }
        Ok(())
    }
```

`list_folders` / `list_folders_page` group over `placements` and take an
`account_id`:

```sql
            SELECT folder_id, COUNT(*) as total,
                   SUM(CASE WHEN read_flag = 0 THEN 1 ELSE 0 END) as unread
            FROM placements
            WHERE account_id = ?
            GROUP BY folder_id
            ORDER BY folder_id ASC
```

Add to `list_folders`'s doc comment:

```rust
    /// Folders are derived from the placements that reference them rather than
    /// stored in their own table, so a folder exists exactly as long as it holds
    /// mail. Scoping by account is required, not cosmetic: folder ids are not
    /// globally unique -- every account has an `INBOX` -- and grouping across
    /// accounts would report one merged folder holding both accounts' mail.
```

Fix the unread count at `db.rs:471` to read
`SELECT COUNT(*) FROM placements WHERE read_flag = 0`.

In `search.rs`, change the FTS hit column from `id` to `message_key` and have
hits resolve through `get_message`; a hit names a message, not a placement.

- [ ] **Step 4: Migrate the existing tests in `db.rs`**

Every pre-existing test constructing an `Email` with `folder_id` / `remote_id` /
`uid_validity` / `read` must move those into a `Placement` and call
`save_email_at`. Work through the compiler errors; there is no behaviour
decision to make in this step. Two that need real thought rather than
mechanical translation:

- the corrupted-ciphertext test around `db.rs:3276` — it must still fail closed
  on `get_message`, unchanged in intent
- `existing_message_ids_returns_only_the_ids_already_persisted` around
  `db.rs:4649` — rewrite as the `existing_placements` test already added above,
  and delete the original

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p nuncio-store`
Expected: PASS, whole crate.

- [ ] **Step 6: Commit**

```bash
git add crates/nuncio-store/src/db.rs crates/nuncio-store/src/search.rs
git commit -m "feat(store): read messages through their placements"
```

---

## Task 5: Store deletes — the FTS plaintext-orphan rule

**Files:**
- Modify: `crates/nuncio-store/src/db.rs` (`delete_messages` → `delete_placements`)
- Test: `crates/nuncio-store/src/db.rs` inline tests

**Interfaces:**
- Consumes: `PlacementKey` from Task 4; `save_email_at` / `placements_of` from Task 3.
- Produces: `delete_placements(&self, keys: &[PlacementKey]) -> Result<DeleteOutcome, DatabaseError>` with `pub struct DeleteOutcome { pub placements_removed: u64, pub messages_reaped: u64 }`
- Test helper: `sample_message_and_placement`, added to `db.rs`'s test module in Task 3 Step 1 (returns an `(Email, Placement)` pair for `account_id = "acct-1"`, `uid_validity = "42"`).

This task carries the non-negotiable that is easiest to get backwards, so its
tests are written to fail loudly in **both** directions.

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn dropping_one_placement_keeps_the_body_searchable_from_the_other() {
    let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
    let (email, inbox) = sample_message_and_placement("key-1", "INBOX", "5");
    let archive = nuncio_core::model::Placement { folder_id: "Archive".into(), remote_id: "9".into(), ..inbox.clone() };
    engine.save_email_at(&email, nuncio_core::model::IdentitySource::EmailId, &inbox).await.unwrap();
    engine.save_email_at(&email, nuncio_core::model::IdentitySource::EmailId, &archive).await.unwrap();

    let inbox_key = PlacementKey { account_id: "acct-1".into(), folder_id: "INBOX".into(), uid_validity: "42".into(), remote_id: "5".into() };
    let outcome = engine.delete_placements(&[inbox_key]).await.unwrap();
    assert_eq!(outcome.placements_removed, 1);
    assert_eq!(outcome.messages_reaped, 0, "the message still lives in Archive");

    engine.get_message("key-1").await.expect("the message must survive");
    let (fts,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM messages_fts WHERE message_key = 'key-1'")
        .fetch_one(engine.pool())
        .await
        .unwrap();
    assert_eq!(fts, 1, "search must still find a message that still exists");
}

#[tokio::test]
async fn dropping_the_last_placement_reaps_the_message_and_its_plaintext_index() {
    let (engine, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
    let (email, inbox) = sample_message_and_placement("key-1", "INBOX", "5");
    engine.save_email_at(&email, nuncio_core::model::IdentitySource::EmailId, &inbox).await.unwrap();

    let inbox_key = PlacementKey { account_id: "acct-1".into(), folder_id: "INBOX".into(), uid_validity: "42".into(), remote_id: "5".into() };
    let outcome = engine.delete_placements(&[inbox_key]).await.unwrap();
    assert_eq!(outcome.messages_reaped, 1);

    engine.get_message("key-1").await.expect_err("the message is gone");
    let (fts,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM messages_fts WHERE message_key = 'key-1'")
        .fetch_one(engine.pool())
        .await
        .unwrap();
    assert_eq!(
        fts, 0,
        "an orphaned FTS row would leave the plaintext body readable to anyone \
         with filesystem access after the message itself was deleted"
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p nuncio-store db::tests::dropping_one_placement db::tests::dropping_the_last_placement`
Expected: FAIL — `no method delete_placements`.

- [ ] **Step 3: Implement the delete path**

```rust
/// What a [`DatabaseEngine::delete_placements`] call removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeleteOutcome {
    /// Mailbox occupancies removed.
    pub placements_removed: u64,
    /// Messages that lost their last placement and were removed with it.
    pub messages_reaped: u64,
}

impl DatabaseEngine {
    /// Remove mailbox occupancies, reaping any message left with none.
    ///
    /// A server that stops reporting a UID has told us the message left *that
    /// mailbox*, not that it ceased to exist -- a move reports exactly this in
    /// the source folder while the message continues in the destination. So the
    /// placement goes unconditionally and the message goes only when its last
    /// placement does.
    ///
    /// The reap is what keeps the plaintext FTS index honest. `messages_fts`
    /// holds decrypted bodies keyed on `message_key`, reaped by the
    /// `messages_ad` trigger on `messages`. Deleting the message row is
    /// therefore the only thing that clears the index: skip the reap and every
    /// fully-deleted message leaves its body readable to anyone with filesystem
    /// access to the database.
    pub async fn delete_placements(
        &self,
        keys: &[PlacementKey],
    ) -> Result<DeleteOutcome, DatabaseError> {
        if keys.is_empty() {
            return Ok(DeleteOutcome { placements_removed: 0, messages_reaped: 0 });
        }

        let mut tx = self.pool.begin().await.map_err(DatabaseError::Query)?;
        let mut placements_removed = 0u64;
        let mut touched: Vec<String> = Vec::with_capacity(keys.len());

        for key in keys {
            // Remember which message each placement pointed at before removing
            // it -- afterwards there is nothing left to join through.
            let owner: Option<(String,)> = sqlx::query_as(
                "SELECT message_key FROM placements
                 WHERE account_id = ? AND folder_id = ? AND uidvalidity = ? AND uid = ?",
            )
            .bind(&key.account_id)
            .bind(&key.folder_id)
            .bind(&key.uid_validity)
            .bind(&key.remote_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(DatabaseError::Query)?;

            let Some((message_key,)) = owner else { continue };

            let result = sqlx::query(
                "DELETE FROM placements
                 WHERE account_id = ? AND folder_id = ? AND uidvalidity = ? AND uid = ?",
            )
            .bind(&key.account_id)
            .bind(&key.folder_id)
            .bind(&key.uid_validity)
            .bind(&key.remote_id)
            .execute(&mut *tx)
            .await
            .map_err(DatabaseError::Query)?;

            placements_removed += result.rows_affected();
            touched.push(message_key);
        }

        touched.sort_unstable();
        touched.dedup();

        let mut messages_reaped = 0u64;
        for message_key in touched {
            // Reap only where nothing is left pointing at the message. The
            // `messages_ad` trigger clears the FTS row as a consequence.
            let result = sqlx::query(
                "DELETE FROM messages WHERE message_key = ?
                 AND NOT EXISTS (SELECT 1 FROM placements WHERE message_key = ?)",
            )
            .bind(&message_key)
            .bind(&message_key)
            .execute(&mut *tx)
            .await
            .map_err(DatabaseError::Query)?;
            messages_reaped += result.rows_affected();
        }

        tx.commit().await.map_err(DatabaseError::Query)?;
        Ok(DeleteOutcome { placements_removed, messages_reaped })
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p nuncio-store`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/nuncio-store/src/db.rs
git commit -m "feat(store): reap a message only when its last placement goes"
```

---

## Task 6: Filter evaluation becomes per-placement

**Files:**
- Modify: `crates/nuncio-filter/src/engine.rs`
- Modify: `crates/nuncio-filter/src/validator.rs` (comments only)
- Test: `crates/nuncio-filter/src/engine.rs` inline tests

**Interfaces:**
- Consumes: `Email`, `Placement` from Task 1.
- Produces:
  - `pub struct PlacedEmail<'a> { pub email: &'a Email, pub placement: &'a Placement }`
  - `CompiledFilter::evaluate_condition(&self, placed: PlacedEmail<'_>) -> bool`
  - `FilterEngine::evaluate(&self, placed: PlacedEmail<'_>) -> Vec<(FilterRule, Vec<RuleAction>)>`
  - `FilterEngine::evaluate_with_timeout(&self, placed: PlacedEmail<'_>, timeout: Duration) -> Vec<(FilterRule, Vec<RuleAction>)>`

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_folder_condition_matches_only_the_placement_it_is_evaluated_against() {
    let engine = FilterEngine::new(vec![rule_where_folder_is("INBOX")]).unwrap();
    let email = sample_email("key-1");
    let inbox = sample_placement("INBOX", "5");
    let archive = sample_placement("Archive", "9");

    assert_eq!(
        engine.evaluate(PlacedEmail { email: &email, placement: &inbox }).len(),
        1,
        "the INBOX placement matches"
    );
    assert_eq!(
        engine.evaluate(PlacedEmail { email: &email, placement: &archive }).len(),
        0,
        "the same message's Archive placement does not"
    );
}

#[test]
fn an_account_condition_reads_the_placement_account() {
    let engine = FilterEngine::new(vec![rule_where_account_is("acct-1")]).unwrap();
    let email = sample_email("key-1");
    let mine = sample_placement("INBOX", "5");
    let theirs = Placement { account_id: "acct-2".into(), ..sample_placement("INBOX", "5") };

    assert_eq!(engine.evaluate(PlacedEmail { email: &email, placement: &mine }).len(), 1);
    assert_eq!(engine.evaluate(PlacedEmail { email: &email, placement: &theirs }).len(), 0);
}
```

Add the test helpers `sample_email`, `sample_placement`,
`rule_where_folder_is`, `rule_where_account_is` next to the existing engine
test helpers, mirroring the rule-construction style already used at
`engine.rs:650`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p nuncio-filter engine::tests::a_folder_condition_matches_only`
Expected: FAIL — `cannot find type PlacedEmail`.

- [ ] **Step 3: Implement per-placement evaluation**

Add near the top of `engine.rs`:

```rust
/// A message together with the one mailbox occupancy being evaluated.
///
/// `WHERE FOLDER = 'INBOX'` has no answer for a message that sits in three
/// folders at once, so evaluation is defined against a single placement and the
/// caller fans out over however many the message has. Borrowing both halves
/// keeps that fan-out free of clones.
#[derive(Debug, Clone, Copy)]
pub struct PlacedEmail<'a> {
    /// Identity and content -- the same for every placement.
    pub email: &'a Email,
    /// The occupancy this evaluation is about.
    pub placement: &'a Placement,
}
```

Thread `PlacedEmail` through `evaluate_condition`, `eval_node`, and
`eval_leaf`, replacing the `email: &Email` parameter. In `eval_leaf`, the
content fields read `placed.email`, and the two placement-scoped fields become:

```rust
            FilterField::Folder => Self::eval_string_op(
                &placed.placement.folder_id,
                &leaf.operator,
                &leaf.value,
                regexes,
            ),
            FilterField::Account => Self::eval_string_op(
                &placed.placement.account_id,
                &leaf.operator,
                &leaf.value,
                regexes,
            ),
```

In `evaluate` and `evaluate_with_timeout`, the account-scope gate reads the
placement:

```rust
            if filter.rule.matches_account(&placed.placement.account_id) && filter.evaluate_condition(placed) {
```

`evaluate_with_timeout` moves owned values into `spawn_blocking`, so it needs
owned clones inside the loop:

```rust
            let filter_for_task = filter.clone();
            let email_for_task = placed.email.clone();
            let placement_for_task = placed.placement.clone();
            let eval_task = tokio::task::spawn_blocking(move || {
                filter_for_task.evaluate_condition(PlacedEmail {
                    email: &email_for_task,
                    placement: &placement_for_task,
                })
            });
```

Update `evaluate`'s doc comment to record the new contract:

```rust
    /// Evaluate one placement of a message, returning the matching rules' actions.
    ///
    /// Defined per placement, not per message: `FOLDER` and `ACCOUNT` are
    /// properties of where the message sits, and a message can sit in several
    /// places at once. Callers holding a multi-placement message evaluate each
    /// placement and are responsible for not acting twice on one message -- see
    /// the fire-once claim in the store.
    ///
    /// A rule only ever fires for the account it was created against
    /// (`FilterRule::target_account`, set via `ON ACCOUNT` or `*` for all
    /// accounts) — condition matching alone is not account-scoped, so this
    /// check must happen before `evaluate_condition` runs.
```

In `validator.rs`, correct the comment above the `Folder | Account` arm at
line ~99 to say these validate against a placement's coordinates.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p nuncio-filter`
Expected: PASS. Existing engine tests need their `evaluate(&email)` calls
rewritten as `evaluate(PlacedEmail { .. })`; that is mechanical.

- [ ] **Step 5: Commit**

```bash
git add crates/nuncio-filter/src/engine.rs crates/nuncio-filter/src/validator.rs
git commit -m "feat(filter): evaluate rules against a placement, not a message"
```

---

## Task 7: Mail backends emit identity and placement

**Files:**
- Modify: `crates/nuncio-mail/src/imap.rs` (~lines 1186, 1208, 1244–1310)
- Modify: `crates/nuncio-mail/src/jmap.rs` (~line 401)
- Modify: `crates/nuncio-mail/src/mock.rs`
- Modify: `crates/nuncio-mail/src/backend.rs` (`FolderChanges`)
- Test: `crates/nuncio-mail/tests/wiremock_jmap_test.rs`, inline `imap.rs` tests

**Interfaces:**
- Consumes: `derive_message_key`, `RemoteIdentity`, `Placement`, `MessageIdentity`.
- Produces: `pub struct PlacedMessage { pub email: Email, pub source: IdentitySource, pub placement: Placement }`; `FolderChanges.upserts: Vec<PlacedMessage>`; `FolderChanges.removals: Vec<PlacementKey>`; `FolderChanges.present: Option<Vec<PlacementKey>>`.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn a_message_fetched_from_two_folders_keeps_one_identity() {
    // Both folders return the same Message-ID and the same octets, so the
    // content tier must produce one key from two different UIDs.
    let raw = b"Message-ID: <a@b.example>\r\nSubject: Hi\r\n\r\nbody";
    let hash = Email::content_hash_of(raw);
    let inbox = Email::derive_message_key(
        "acct-1",
        RemoteIdentity { email_id: None, gm_msgid: None, message_id: Some("a@b.example"), content_hash: Some(&hash) },
        "INBOX", "42", "5",
    );
    let archive = Email::derive_message_key(
        "acct-1",
        RemoteIdentity { email_id: None, gm_msgid: None, message_id: Some("a@b.example"), content_hash: Some(&hash) },
        "Archive", "77", "9",
    );
    assert_eq!(inbox.key, archive.key);
    assert_eq!(inbox.source, IdentitySource::MessageIdContent);
}
```

Place this in `crates/nuncio-mail/src/imap.rs`'s test module so it guards the
backend's own use of the precedence, not just the core function.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p nuncio-mail a_message_fetched_from_two_folders`
Expected: FAIL to compile until `nuncio-mail` is migrated.

- [ ] **Step 3: Migrate the IMAP backend**

At each of `imap.rs:1186`, `1208`, and `1278`, the call currently reads
`Email::surrogate_id(&self.account_id, folder_id, &uid_validity, &remote_id)`.
Replace with a helper on the backend:

```rust
    /// Build the identity and placement for one fetched message.
    ///
    /// `EMAILID` and `X-GM-MSGID` are only present when the server advertised
    /// `OBJECTID` / `X-GM-EXT-1` and the FETCH asked for them; when neither is
    /// available the pair of `Message-ID` and content hash carries the identity,
    /// and only if both are missing does this fall back to the folder-scoped
    /// surrogate -- which is the one tier that does not survive a move.
    fn place(
        &self,
        folder_id: &str,
        uid_validity: &str,
        remote_id: &str,
        email_id: Option<&str>,
        gm_msgid: Option<&str>,
        message_id: Option<&str>,
        content_hash: Option<&str>,
        read: bool,
    ) -> (MessageIdentity, Placement) {
        let identity = Email::derive_message_key(
            &self.account_id,
            RemoteIdentity { email_id, gm_msgid, message_id, content_hash },
            folder_id,
            uid_validity,
            remote_id,
        );
        let placement = Placement {
            account_id: self.account_id.clone(),
            folder_id: folder_id.to_string(),
            uid_validity: uid_validity.to_string(),
            remote_id: remote_id.to_string(),
            read,
        };
        (identity, placement)
    }
```

`EMAILID` and `X-GM-MSGID` are not fetched today. Pass `None` for both and
leave a comment saying so — wiring the FETCH items is #395/#396 territory and
adding it here would widen this change without a test server that serves them:

```rust
        // EMAILID (RFC 8474) and X-GM-MSGID are not requested by the current
        // FETCH, so identity rests on the Message-ID/content pair where the
        // message carried a Message-ID and on the folder-scoped surrogate
        // otherwise. Adding those FETCH items lifts existing rows to a stronger
        // tier on the next resync at no cost to this code path.
```

Change `FolderChanges` in `backend.rs`:

```rust
/// One message as a backend surfaced it: identity, content, and the mailbox
/// occupancy it was found in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedMessage {
    /// Identity and content. `email.id` is the derived message key.
    pub email: Email,
    /// Which precedence tier produced `email.id`.
    pub source: IdentitySource,
    /// The occupancy this fetch found it in.
    pub placement: Placement,
}
```

and retype `upserts`, `removals`, and `present` as noted in the Interfaces
block above. `removals` and `present` become placement keys because what a
folder stops reporting is an occupancy.

- [ ] **Step 4: Migrate the JMAP backend and the mock**

At `jmap.rs:401`, JMAP has no UIDVALIDITY, so it keeps its existing sentinel as
`uid_validity` and uses the JMAP object id as `remote_id`. JMAP's object id is
server-assigned and account-stable, so pass it as `email_id` — that is exactly
what the top tier is for:

```rust
        // A JMAP Email id is server-assigned and stable across mailboxes within
        // the account, which is the same guarantee RFC 8474 EMAILID gives, so it
        // enters at the top tier rather than through the surrogate.
```

In `mock.rs`, have `MockMailBackend` emit `PlacedMessage` values so daemon
tests can construct multi-placement scenarios.

Leave the JMAP `present: None` gap exactly as it is — JMAP still accumulates
ghosts until `Email/changes` lands, and pretending otherwise here would delete
mail.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p nuncio-mail`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/nuncio-mail/
git commit -m "feat(mail): derive message identity and emit placements"
```

---

## Task 8: Sync wiring — placement-aware fan-out and the fire-once claim

**Files:**
- Modify: `crates/nunciod/src/sync.rs` (`fetch_and_persist`, `apply_filter_actions`)
- Test: `crates/nunciod/src/sync.rs` inline tests, `crates/nunciod/tests/filters_triage_e2e_test.rs`

**Interfaces:**
- Consumes: `save_email_at`, `existing_placements`, `delete_placements`, `claim_filter_fire`, `PlacedEmail`, `PlacedMessage`.
- Produces: no new public API; `fetch_and_persist` keeps its signature and return type.

This task carries the bug the issue calls out by name: under a message-scoped
"is new" test, a message arriving in a second folder looks like it was already
seen and **silently never fires any filter**.

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn a_message_arriving_in_a_second_folder_still_gets_evaluated() {
    // The regression this guards: with identity now folder-independent, a
    // message already stored from INBOX has a known message_key, so a
    // message-scoped "already present" test would classify its arrival in
    // Archive as old and skip filtering it entirely.
    let (db, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
    let backend = MockMailBackend::with_same_message_in(&["INBOX", "Archive"]);
    let engine = FilterEngine::new(vec![rule_matching_everything()]).unwrap();

    let processed = fetch_and_persist(&db, &EventBus::new(), &backend, &engine, Some("acct-1"))
        .await
        .unwrap();

    assert_eq!(processed, 2, "both placements are processed");
    let placements = db.placements_of(&expected_key()).await.unwrap();
    assert_eq!(placements.len(), 2);
}

#[tokio::test]
async fn a_rule_fires_once_for_a_message_that_lands_in_two_folders() {
    // MOVE and FLAG converge under repetition; FORWARD and CALL WEBHOOK do not.
    // Two placements of one message must not page an on-call engineer twice.
    let (db, _dir) = DatabaseEngine::connect_ephemeral().await.unwrap();
    let backend = MockMailBackend::with_same_message_in(&["INBOX", "Archive"]);
    let engine = FilterEngine::new(vec![rule_matching_everything()]).unwrap();

    fetch_and_persist(&db, &EventBus::new(), &backend, &engine, Some("acct-1"))
        .await
        .unwrap();

    assert_eq!(
        db.filter_fire_count().await.unwrap(),
        1,
        "one claim, therefore one set of actions"
    );
    assert!(db.has_filter_fired(&rule_matching_everything().id, &expected_key()).await.unwrap());
}
```

Add `MockMailBackend::with_same_message_in(folders: &[&str])` to `mock.rs`,
returning the same `email` (same derived key) under a different
`folder_id`/`remote_id` per folder.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p nunciod sync::tests::a_message_arriving_in_a_second_folder`
Expected: FAIL to compile until the sync path is migrated.

- [ ] **Step 3: Rewrite the classification and fan-out**

In `fetch_and_persist`, replace the chunk-classification block:

```rust
        // One round trip classifies the whole chunk. This asks about
        // *placements*, not messages: identity is folder-independent now, so a
        // message already stored from another folder is still a first arrival
        // here, and testing the message key instead would classify it as seen
        // and skip filtering it entirely.
        let chunk_keys: Vec<PlacementKey> = placed
            .iter()
            .map(|p| PlacementKey {
                account_id: p.placement.account_id.clone(),
                folder_id: p.placement.folder_id.clone(),
                uid_validity: p.placement.uid_validity.clone(),
                remote_id: p.placement.remote_id.clone(),
            })
            .collect();
        let already_present = db.existing_placements(&chunk_keys).await?;
        let mut seen_this_pass = std::collections::HashSet::new();

        for message in placed {
            let key = PlacementKey {
                account_id: message.placement.account_id.clone(),
                folder_id: message.placement.folder_id.clone(),
                uid_validity: message.placement.uid_validity.clone(),
                remote_id: message.placement.remote_id.clone(),
            };
            let is_new = !already_present.contains(&key) && seen_this_pass.insert(key);
            db.save_email_at(&message.email, message.source, &message.placement).await?;
            if is_new {
                apply_filter_actions(db, filter_engine, &message.email, &message.placement).await;
            }
            synced += 1;
        }
```

Change `apply_filter_actions` to take the placement, evaluate against it, and
gate every action behind the claim:

```rust
async fn apply_filter_actions(
    db: &nuncio_store::db::DatabaseEngine,
    filter_engine: &FilterEngine,
    email: &nuncio_core::model::Email,
    placement: &nuncio_core::model::Placement,
) {
    let placed = nuncio_filter::PlacedEmail { email, placement };
    for (rule, actions) in filter_engine.evaluate(placed) {
        // Claim before acting. The claim is keyed on message identity, so the
        // same mail arriving in a second folder -- an ordinary MOVE, or a Gmail
        // label -- does not run the actions again. That is load-bearing for
        // FORWARD and CALL WEBHOOK, which land on third parties and have no
        // shared state to converge against; MOVE and FLAG would survive the
        // repetition, but there is no reason to pay for it.
        match db.claim_filter_fire(&rule.id, &email.id).await {
            Ok(true) => {}
            Ok(false) => {
                tracing::debug!(
                    rule_id = %rule.id,
                    message_key = %email.id,
                    folder_id = %placement.folder_id,
                    "rule already fired for this message; skipping actions for this placement"
                );
                continue;
            }
            Err(e) => {
                // Fail closed. An unverifiable claim must not become a licence
                // to act: a lost claim check on a FORWARD is a duplicate mail
                // the recipient cannot un-receive.
                tracing::error!(
                    rule_id = %rule.id,
                    message_key = %email.id,
                    error = %e,
                    "could not claim the filter fire; skipping actions"
                );
                continue;
            }
        }

        // ... existing per-action dispatch, unchanged apart from taking
        // `placement` where it previously read `email.folder_id` /
        // `email.remote_id` / `email.uid_validity`
    }
}
```

Every `RemoteMutationSpec` built here now takes its `folder_id`, `remote_id`,
and `uid_validity` from `placement`, and its `message_id` from `email.id`.
`MARK READ` names the placement it was evaluated against — which is what makes
it well-defined at all, since `\Seen` is per-mailbox.

Update the removals block to work in placements:

```rust
            if let Some(present) = changes.present {
                let present: std::collections::HashSet<PlacementKey> = present.into_iter().collect();
                let stored = db.placements_in_folder(acct, &folder.id).await?;
                gone.extend(stored.into_iter().filter(|key| !present.contains(key)));
            }

            gone.sort_unstable();
            gone.dedup();
            if !gone.is_empty() {
                let outcome = db.delete_placements(&gone).await?;
                tracing::info!(
                    account_id = %acct,
                    folder_id = %folder.id,
                    placements_removed = outcome.placements_removed,
                    messages_reaped = outcome.messages_reaped,
                    "removed placements that are no longer on the server"
                );
            }
```

`PlacementKey` needs `Ord` for the `sort_unstable`/`dedup` pair; derive
`PartialOrd, Ord` on it in Task 4.

Rewrite the doc comment on `fetch_and_persist` so it describes placement
classification rather than message classification — the current text at
`sync.rs:200–228` explains the old model in detail and would be actively
misleading left as-is.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p nunciod`
Expected: PASS, including `filters_triage_e2e_test`.

- [ ] **Step 5: Commit**

```bash
git add crates/nunciod/src/sync.rs crates/nuncio-mail/src/mock.rs
git commit -m "feat(sync): classify arrivals per placement and claim fires once"
```

---

## Task 9: Daemon surface — gRPC, CLI, and the proto comment

**Files:**
- Modify: `crates/nunciod/src/grpc.rs`
- Modify: `crates/nuncio-proto/proto/nuncio/v1/nuncio.proto` (comment only)
- Modify: `crates/nuncio-cli/src/runner.rs` (only where it fails to compile)
- Test: `crates/nunciod/tests/spine_e2e_test.rs`, `crates/nunciod/tests/typed_errors_e2e_test.rs`

**Interfaces:**
- Consumes: everything above.
- Produces: no wire-schema change. `Message.folder_id` and `Message.read` are populated from the placement the message was reached through.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn list_messages_reports_the_folder_it_was_asked_about() {
    // A message in two folders appears in both listings, each time carrying the
    // placement that listing is about -- not an arbitrary one of the two.
    let harness = SpineHarness::start().await;
    harness.seed_message_in(&["INBOX", "Archive"]).await;

    for folder in ["INBOX", "Archive"] {
        let listed = harness
            .client()
            .list_messages(ListMessagesRequest { folder_id: folder.into(), ..Default::default() })
            .await
            .unwrap()
            .into_inner();
        assert_eq!(listed.messages.len(), 1);
        assert_eq!(listed.messages[0].folder_id, folder);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p nunciod --test spine_e2e_test list_messages_reports_the_folder`
Expected: FAIL to compile until `grpc.rs` is migrated.

- [ ] **Step 3: Migrate the gRPC layer**

**Account scoping at the boundary.** The store methods now take an
`account_id`, but `ListFoldersRequest` and `ListMessagesRequest` carry none and
this plan does not change the wire schema. Bridge it by iterating the accounts
and merging, which reproduces exactly today's behaviour — no wire change, no
regression:

```rust
        // The request carries no account, so this reproduces the pre-existing
        // account-blind view: every account's folders, merged by folder id. The
        // store below it is account-scoped and correct; only this presentation
        // is ambiguous, and it is ambiguous in precisely the way it already was.
        // An account-scoped request field is the real fix and belongs with the
        // rest of the placement surface.
        let mut merged: std::collections::BTreeMap<String, nuncio_core::model::Folder> =
            std::collections::BTreeMap::new();
        for account in db.list_accounts().await? {
            for folder in db.list_folders(&account.id).await? {
                let entry = merged.entry(folder.id.clone()).or_insert_with(|| {
                    nuncio_core::model::Folder {
                        id: folder.id.clone(),
                        name: folder.name.clone(),
                        total_messages: 0,
                        unread_messages: 0,
                    }
                });
                entry.total_messages += folder.total_messages;
                entry.unread_messages += folder.unread_messages;
            }
        }
```

Apply the same account loop to `ListMessages`, concatenating each account's
matches for the requested folder and then applying the existing ordering and
page size to the merged result. Use whatever account-listing method `db`
already exposes rather than adding one.

`ListMessages` already carries a folder, so map each `(Email, Placement)` pair
straight onto the proto `Message`, taking `folder_id` and `read` from the
placement.

`GetMessage` receives only an id, so it must choose. Resolve through
`placements_of` and take the first — `placements_of` orders by `folder_id ASC`,
so the choice is deterministic rather than arbitrary:

```rust
        // A message key can name several placements and this RPC's request
        // carries no folder to disambiguate them. Report the first by folder id
        // so the answer is at least stable between calls, and mark the response
        // honestly: the full placement list needs a wire field this contract
        // does not have yet.
        let placements = db.placements_of(&request.message_id).await?;
        let Some(primary) = placements.first() else {
            return Err(tonic::Status::not_found(
                "message has no placement; it is no longer in any folder",
            ));
        };
```

Add the same note to the `.proto` beside `Message.folder_id`:

```proto
  // Mailbox folder identifier (e.g. "inbox").
  //
  // A message can occupy several folders at once. This names the one the
  // message was reached through: the requested folder for a listing, and the
  // lowest-sorting folder id for a lookup by id. A field carrying the complete
  // placement set is not yet part of this contract.
  string folder_id = 3;
```

and correct the `Message.id` comment, whose current text describes the old
folder-scoped derivation:

```proto
  // Unique message identifier. OPAQUE and server-assigned: content-addressed
  // from the message's strongest available identity, so it is independent of
  // which folder the message currently occupies and survives a move. Never a
  // raw upstream protocol id (e.g. an IMAP UID, which is only stable within a
  // single IMAP session and is NOT a durable cross-session identifier).
  // Clients MUST treat this as an opaque token -- pass it back verbatim in
  // later requests, but never parse, construct, or assume any structure in
  // its value.
  string id = 1;
```

- [ ] **Step 4: Reconcile the descriptor golden**

Run: `cargo test -p nuncio-proto descriptor_matches_committed_golden`

If it passes, comments are not in the descriptor and nothing more is needed.
If it fails, the test prints the exact path of the freshly generated
descriptor; copy it over `crates/nuncio-proto/proto/nuncio/v1/descriptor.bin`
exactly as the failure message instructs, then re-run to confirm.

- [ ] **Step 5: Fix the CLI**

Work through `cargo check-all` errors in `crates/nuncio-cli/src/runner.rs`.
The CLI consumes the proto types, which have not changed shape, so the fixes
should be confined to test fixtures constructing `Message` values.

- [ ] **Step 6: Run the full gate**

```bash
cargo fmt --all -- --check
cargo check-all
cargo test-all
```

Expected: all three pass. Give the test command ~540s; `cargo test-all` can
exceed a 10-minute tool timeout on this workspace.

- [ ] **Step 7: Commit**

```bash
git add crates/nunciod/src/grpc.rs crates/nuncio-cli/src/runner.rs crates/nuncio-proto/
git commit -m "feat(daemon): present the placement a message was reached through"
```

---

## Task 10: Documentation and PR

**Files:**
- Modify: `docs/ROADMAP.md` and `docs/roadmap/index.html` (both, in one commit)
- Modify: `docs/HANDOFF.md`

- [ ] **Step 1: Update the roadmap in both formats**

Mark the message/placement split done in `docs/ROADMAP.md`, and make the
matching edit to `docs/roadmap/index.html`. The rendered page is published at
<https://koftwentytwo.github.io/nuncio/roadmap/> and must never drift from the
markdown; the repo requires both to change in the same commit.

- [ ] **Step 2: Commit**

```bash
git add docs/
git commit -m "docs(roadmap): mark the message/placement split complete"
```

- [ ] **Step 3: Push and open the PR**

SSH has no key in this environment; push over HTTPS with the `gh` credential
helper and leave the remote as SSH:

```bash
git -c credential.helper='!gh auth git-credential' \
  push https://github.com/KofTwentyTwo/nuncio.git story/message-placement-split
```

Open the PR against `dev`. The body must carry the reasoning, not just the
what, and must state explicitly:

- the branch merges #398 (#386) and #400 (#387) as its declared ADR
  dependencies, so **those two PRs must land first**
- `IDENTITY_SCHEMA_VERSION` goes 1 → 2, so **every existing local store resets
  its cached mail on next open**; accounts, credentials, and rules survive
- `EMAILID` / `X-GM-MSGID` are not yet fetched, so identity currently rests on
  the Message-ID/content tier and the surrogate fallback; adding those FETCH
  items lifts rows to a stronger tier on the next resync
- JMAP still reports `present: None` and therefore still accumulates ghost
  messages — unchanged by this PR, and needs `Email/changes`
- the wire contract is unchanged; `Message.folder_id` now names the placement
  the message was reached through, and the full placement set awaits #395
- expect conflicts in `db.rs`, `imap.rs`, and `sync.rs` against the
  #401–#403 chain

---

## Deferred, deliberately

Named here so they are not mistaken for oversights.

- **`EMAILID` / `X-GM-MSGID` are not requested.** Wiring the FETCH items needs a
  test server that serves them; the precedence handles their absence correctly
  and picks them up for free once they arrive.
- **The full placement set is not on the wire.** `Message` gains it in #395,
  which owns the proto surface.
- **`GetMessage` picks one placement.** Deterministic (lowest folder id) and
  documented, but genuinely lossy until the contract can express the set.
- **JMAP ghost messages persist.** `present: None` is unchanged; the fix is
  `Email/changes` and is not filed as an issue yet.
