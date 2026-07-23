# Database Corruption Detection, Recovery & Self-Healing Specification

> **Authoritative Wiki Master Specification**  
> Maintained in `wiki/Database-Corruption-and-Recovery-Specification.md` and synchronized with [KofTwentyTwo/nuncio](https://github.com/KofTwentyTwo/nuncio).

---

## Executive Overview

Handling database corruption is a critical requirement for a local-first desktop, CLI, and daemon application. Unexpected power loss, OS crashes mid-write, disk bit rot, or WAL index mismatches must **never** cause Nuncio to crash silently or lose unrecoverable account credentials and rule configurations.

Nuncio implements a **5-Stage Self-Healing & Recovery Engine** in `nuncio-store` and `nunciod`.

---

## 1. The 5-Stage Automated Recovery Pipeline

```
               [ nunciod / CLI / TUI / GUI Startup ]
                                 │
                                 ▼
       ┌──────────────────────────────────────────────────┐
       │ Stage 1: Pre-Flight Integrity Probe              │
       │ Executes `PRAGMA quick_check;` on startup        │
       └────────────────────────┬─────────────────────────┘
                                │
                   ┌────────────┴────────────┐
                   ▼                         ▼
            [ Status: OK ]          [ Status: CORRUPT ]
                   │                         │
                   ▼                         ▼
            Proceed Normally       ┌──────────────────────────────────┐
                                   │ Stage 2: Automatic Backup        │
                                   │ Copy `.db` to `corrupted_backup/`│
                                   └────────────────┬─────────────────┘
                                                    │
                                                    ▼
                                   ┌──────────────────────────────────┐
                                   │ Stage 3: `.recover` Salvage      │
                                   │ Dump valid tables to fresh DB    │
                                   └────────────────┬─────────────────┘
                                                    │
                                                    ▼
                                   ┌──────────────────────────────────┐
                                   │ Stage 4: Remote Server Resync    │
                                   │ Re-sync inbox/cal from IMAP/JMAP │
                                   └────────────────┬─────────────────┘
                                                    │
                                                    ▼
                                   ┌──────────────────────────────────┐
                                   │ Stage 5: 4-Shell Status Alert    │
                                   │ Broadcast IPC `DatabaseRecovered`│
                                   └──────────────────────────────────┘
```

---

## 2. Stage Breakdown & Technical Implementation

### Stage 1: Pre-Flight Integrity Probe & Runtime Interception
Before opening the SQLite pool in `DatabaseEngine::open()`, Nuncio executes a low-overhead integrity probe:

```rust
pub async fn check_integrity(pool: &SqlitePool) -> Result<bool, StoreError> {
    let result: (String,) = sqlx::query_as("PRAGMA quick_check(10);")
        .fetch_one(pool)
        .await?;
    
    Ok(result.0.to_lowercase() == "ok")
}
```

#### Runtime Catching (`SQLITE_CORRUPT`):
If a `SqliteError` with error code `SQLITE_CORRUPT` (code 11) or `SQLITE_NOTADB` (code 26) occurs during normal queries, the engine traps the exception, closes active pool connections, and transitions immediately to Stage 2.

---

### Stage 2: Forensic Backup Preservation
Before attempting repair, Nuncio preserves the raw corrupted database file so data can never be accidentally destroyed during recovery.

- **Backup Path**: `~/.nuncio/corrupted_backups/nuncio_corrupted_<timestamp>.db`
- Includes companion WAL (`.db-wal`) and Shared Memory (`.db-shm`) files.

---

### Stage 3: SQLite Recovery Engine (`sqlite3_db_recover` / Salvage)
Nuncio attempts to salvage readable data (account configurations, NSQL rules, local drafts) using SQLite's stream recovery algorithm:

1. Opens the corrupted file in read-only salvage mode.
2. Extracts valid B-Tree pages for critical metadata tables (`accounts`, `filter_rules`, `filter_conditions`, `filter_actions`).
3. Writes salvaged schema and records to a brand-new SQLite WAL file (`nuncio_main.db`).

---

### Stage 4: Remote Protocol Self-Healing Resync
Because Nuncio is an email and calendar client connected to remote IMAP, JMAP, CalDAV, and CardDAV servers:

> **Local SQLite is a high-speed offline cache of truth, while remote mail servers hold the permanent remote record.**

If local email or event tables are damaged beyond salvage:
1. **Preserved Secrets**: Account credentials (passwords, OAuth tokens, private keys) are stored in the OS Keyring vault (`keyring` crate) and remain **100% intact**.
2. **Preserved Rules**: NSQL rules salvaged in Stage 3 are re-applied.
3. **Automated Resync**: `nunciod` initiates a clean background resync from the remote IMAP/JMAP server, rebuilding the local SQLite inbox cache seamlessly.

---

### Stage 5: 4-Shell Notification & Log Transparency
When database self-healing completes, `nunciod` broadcasts a `CoreEvent::DatabaseRecovered` payload across IPC sockets:

```json
{
  "jsonrpc": "2.0",
  "method": "events.notify",
  "params": {
    "event": "DatabaseRecovered",
    "payload": {
      "backup_path": "~/.nuncio/corrupted_backups/nuncio_corrupted_1700000000.db",
      "salvaged_rules_count": 12,
      "resync_triggered": true
    }
  }
}
```

#### Multi-Shell User Experience:
- **POSIX CLI**: Displays `[NOTICE] Database integrity issue was automatically repaired. Resynchronizing inbox...`
- **Terminal TUI**: Displays a pop-up status banner in the top menu.
- **Desktop GUI**: Renders a non-intrusive toast notification: `"Database auto-healed. Resyncing mail..."`
- **Native MCP**: Returns diagnostic status in `nuncio://system/status`.
