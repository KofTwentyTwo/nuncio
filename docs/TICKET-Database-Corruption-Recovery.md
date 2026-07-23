# Ticket Specification: Database Corruption Detection, Recovery & Self-Healing Engine

**Epic ID**: `EPIC-DATABASE-CORRUPTION-RECOVERY`  
**Tracking Issues**: `#286` through `#300`  
**Target Release**: Nuncio v1.0.0 (Core Storage Resilience Milestone)  
**Specification Document**: [`wiki/Database-Corruption-and-Recovery-Specification.md`](file:///R:/Git.Local/KofTwentyTwo/nuncio/wiki/Database-Corruption-and-Recovery-Specification.md)  
**Status**: `READY_FOR_IMPLEMENTATION`  

---

## 1. Executive Summary & Objective

Implement a **5-Stage Automated Database Self-Healing & Recovery Pipeline** in `crates/nuncio-store` and `crates/nunciod`. 

Unexpected power loss, OS crashes mid-write, disk bit rot, or WAL index mismatches must **never** cause Nuncio to crash silently or lose unrecoverable account credentials and rule configurations. When corruption occurs, Nuncio will automatically probe database integrity on startup, isolate damaged files into forensic backups, salvage valid tables, initiate clean remote server resyncs (using intact OS Keyring secrets), and broadcast recovery status alerts across all 4 presentation shells.

---

## 2. 5-Stage System Pipeline

```
 [ Startup / Query Error ]
             │
             ▼
┌────────────────────────────────────────────────────────┐
│ Stage 1: Pre-Flight Integrity Probe                    │
│ Executes `PRAGMA quick_check(10);` in DatabaseEngine   │
└──────────────────────────┬─────────────────────────────┘
                           │
                           ▼
┌────────────────────────────────────────────────────────┐
│ Stage 2: Forensic Backup Isolation                     │
│ Copies .db to `~/.nuncio/corrupted_backups/`          │
└──────────────────────────┬─────────────────────────────┘
                           │
                           ▼
┌────────────────────────────────────────────────────────┐
│ Stage 3: `.recover` Stream Salvage Engine              │
│ Extracts valid tables (accounts, rules) to fresh DB   │
└──────────────────────────┬─────────────────────────────┘
                           │
                           ▼
┌────────────────────────────────────────────────────────┐
│ Stage 4: Remote Server Self-Healing Resync             │
│ Triggers background IMAP/JMAP inbox rebuilding         │
└──────────────────────────┬─────────────────────────────┘
                           │
                           ▼
┌────────────────────────────────────────────────────────┐
│ Stage 5: 4-Shell IPC Recovery Notifications            │
│ Broadcasts `CoreEvent::DatabaseRecovered` across UIs   │
└────────────────────────────────────────────────────────┘
```

---

## 3. Work Breakdown & Tracking Issues (#286 – #300)

### Phase 1: Integrity Probing & Error Interception (`crates/nuncio-store`)
- [ ] `#286`: Implement `DatabaseEngine::check_integrity()` executing `PRAGMA quick_check(10);` on database open.
- [ ] `#287`: Implement SQLite error code trap catching `SQLITE_CORRUPT` (11), `SQLITE_NOTADB` (26), and `SQLITE_CANTOPEN` (14) during runtime query execution.

### Phase 2: Backup & Salvage Recovery Engine (`crates/nuncio-store`)
- [ ] `#288`: Implement `CorruptedBackupManager` isolating damaged `.db`, `.db-wal`, and `.db-shm` files to `~/.nuncio/corrupted_backups/nuncio_corrupted_<timestamp>.db`.
- [ ] `#289`: Implement `SqliteRecoveryEngine` executing stream salvage to extract valid `accounts`, `filter_rules`, `filter_conditions`, and `filter_actions` records into a fresh SQLite file (`nuncio_main.db`).
- [ ] `#290`: Implement cryptographic hash-chain audit ledger verification (`verify_chain_integrity()`) to detect log tampering or corrupted `filter_execution_logs`.

### Phase 3: Daemon Self-Healing Resync (`crates/nunciod`)
- [ ] `#291`: Implement `SelfHealingSyncOrchestrator` in `nunciod` triggering clean background IMAP/JMAP resync when local email caches are reset.
- [ ] `#292`: Connect OS Keyring vault credentials to re-initialize authenticated protocol connections seamlessly post-recovery.
- [ ] `#293`: Implement `CoreEvent::DatabaseRecovered` event payload and broadcast it over IPC sockets.

### Phase 4: Multi-Shell Presentation Alerts (100% Parity)
- [ ] `#294`: **POSIX CLI (`nuncio-cli`)**: Display recovery notice when `DatabaseRecovered` event or status flag is detected.
- [ ] `#295`: **Terminal TUI (`nuncio-tui`)**: Render top menu recovery banner and status indicator.
- [ ] `#296`: **Desktop GUI (`nuncio-gui`)**: Render toast notification and resync status badge in React frontend.
- [ ] `#297`: **Native MCP (`nuncio-mcp`)**: Expose database health and recovery diagnostic status in `nuncio://system/status`.

### Phase 5: Testing & Simulation Suite
- [ ] `#298`: Create unit test simulating database header corruption and verifying Stage 1 detection.
- [ ] `#299`: Create integration test verifying Stage 2 backup creation and Stage 3 table salvage.
- [ ] `#300`: Verify zero warnings (`cargo check --workspace`), zero clippy lints (`cargo clippy --workspace -- -D warnings`), and 100% passing tests.

---

## 4. Acceptance Criteria & Quality Gates

1. **Zero Data Loss for Secrets**: OS Keyring secrets (passwords, OAuth tokens) remain 100% intact post-recovery.
2. **Automated Probe**: Startup `PRAGMA quick_check(10);` completes in $<15\text{ms}$.
3. **Backup Isolation**: Damaged `.db` files are copied to `~/.nuncio/corrupted_backups/` before repair.
4. **Zero Warnings**: `cargo check --workspace` yields 0 warnings.
5. **Zero Clippy Lints**: `cargo clippy --workspace -- -D warnings` yields 0 lints.
6. **100% Passing Tests**: All workspace tests pass.
