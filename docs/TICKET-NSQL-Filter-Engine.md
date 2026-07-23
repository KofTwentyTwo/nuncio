# Ticket Specification: NSQL Server-Side Email Filter & Automation Engine

**Epic ID**: `EPIC-NSQL-FILTER-ENGINE`  
**Tracking Issues**: `#261` through `#285`  
**Target Release**: Nuncio v1.0.0 (V3 Automation & Platform Milestone)  
**Specification Document**: [`wiki/NSQL-Filter-Language-Specification.md`](file:///R:/Git.Local/KofTwentyTwo/nuncio/wiki/NSQL-Filter-Language-Specification.md)  
**Status**: `READY_FOR_IMPLEMENTATION`  

---

## 1. Executive Summary & Objective

Implement **Nuncio SQL Filter Language (NSQL)**—a declarative, type-safe, server-side email automation and thread triage engine powered by `sqlparser = "0.54"`. 

The engine must execute **server-side inside `nunciod`** during protocol synchronization (IMAP IDLE / JMAP Push) **before** messages are written to local storage, and support 1M-email batch triage with fixed memory ceilings ($<10\text{MB}$ RAM). Under Nuncio's **100% Multi-Shell Parity Mandate**, all filter rule actions (creation, editing, visual building, dry-run testing, deletion, export, and log auditing) must be 100% accessible with 1:1 functional equivalence across `nuncio-cli`, `nuncio-tui`, `nuncio-gui`, and `nuncio-mcp`.

---

## 2. Core Architecture & System Specifications

```
                                  NSQL ENGINE PIPELINE
                                 
  [ Remote Email Server ] (IMAP IDLE / JMAP Push)
             │
             ▼
  ┌────────────────────────────────────────────────────────┐
  │ 1. Pre-Ingestion Async Pipeline                        │
  │    Stage 1: Async MIME Parsing                         │
  │    Stage 2: Lock-Free ArcSwap Rule Eval (<250µs)       │
  │    Stage 3: In-Memory Action Tagging                   │
  │    Stage 4: Transactional SQLite WAL Save              │
  └──────────────────────────┬─────────────────────────────┘
                             │
                             ▼
  ┌────────────────────────────────────────────────────────┐
  │ 2. Dual-Write Consistency Engine                       │
  │    Transactional Outbox + Exponential Backoff Retry    │
  └──────────────────────────┬─────────────────────────────┘
                             │
                             ▼
  ┌────────────────────────────────────────────────────────┐
  │ 3. Cryptographic Hash-Chain Execution Ledger            │
  │    HMAC-SHA256 Link Validation (filter_execution_logs) │
  └──────────────────────────┬─────────────────────────────┘
                             │
                             ▼
  ┌────────────────────────────────────────────────────────┐
  │ 4. 4-Shell IPC Event Streaming                         │
  │    Length-Prefixed JSON-RPC (CoreEvent::FilterExecuted)│
  └────────────────────────────────────────────────────────┘
```

---

## 3. Micro-Task Work Breakdown & Tracking Issues (#261 – #285)

### Phase 1: Compiler & AST Engine (`crates/nuncio-filter`)
- [ ] `#261`: Initialize `crates/nuncio-filter` workspace crate with `sqlparser = "0.54"` dependency.
- [ ] `#262`: Implement `NuncioSqlDialect` overriding `sqlparser::dialect::Dialect` for NSQL tokenization.
- [ ] `#263`: Implement `NsqlParser` transforming SQL text into `nuncio-core` domain AST (`FilterRule`, `ConditionNode`, `RuleAction`).
- [ ] `#264`: Implement 6-pass `NsqlValidator` type checker:
  - Pass 1: Field & operator compatibility check.
  - Pass 2: Mailbox target folder existence verification.
  - Pass 3: RFC 5322 email syntax validation.
  - Pass 4: Size boundary verification ($\le 500\text{MB}$).
  - Pass 5: ReDoS regex DFA memory compilation check.
  - Pass 6: Logical contradiction & dead action analysis.
- [ ] `#265`: Implement lossless round-trip code generator (`rule.to_nsql()`) and verify algebraic invariants ($\text{parse}(\text{to\_nsql}(r)) \equiv r$).

### Phase 2: Storage & Cryptographic Ledger (`crates/nuncio-store`)
- [ ] `#266`: Add SQLite migration DDL for `filter_rules`, `filter_conditions`, `filter_actions`, and `pending_remote_mutations`.
- [ ] `#267`: Add SQLite migration DDL for `filter_execution_logs` with composite priority indexes.
- [ ] `#268`: Implement HMAC-SHA256 cryptographic hash-chain ledger for `filter_execution_logs` ($H_n = \text{HMAC}(K, H_{n-1} \parallel \text{data}_n)$) with automated chain integrity verification.
- [ ] `#269`: Implement parameterized SQLite query compilation (`sqlx::QueryBuilder`) guaranteeing zero raw string interpolation (100% SQLi immunized).

### Phase 3: Daemon Execution Engine & 1M-Message Batch Processor (`crates/nunciod`)
- [ ] `#270`: Implement `ArcSwap<CompiledFilterSet>` lock-free read cache manager ($<5\text{ns}$ access latency).
- [ ] `#271`: Integrate `FilterEngine::evaluate()` into `nunciod` background sync loop for pre-ingestion filtering.
- [ ] `#272`: Implement 1M-message Keyset Chunking Engine (`WHERE id > ? LIMIT 1000`) guaranteeing $<10\text{MB}$ RAM ceiling during bulk triage.
- [ ] `#273`: Implement Transactional Outbox pattern with exponential backoff and full jitter for remote IMAP/JMAP mutations.
- [ ] `#274`: Implement Memory-Only Dry-Run Preview API (`filter.preview` IPC method) returning microsecond-resolution match traces without database mutations or network requests.
- [ ] `#275`: Implement `CoreEvent::FilterExecuted` and `CoreEvent::BatchFilterProgress` IPC length-prefixed streaming notifications over UNIX domain sockets and Windows named pipes.

### Phase 4: Security & Defensive Controls
- [ ] `#276`: Enforce AST recursion depth limit (`MAX_AST_DEPTH = 10`) to prevent stack overflow panics.
- [ ] `#277`: Implement Tokio 50ms hard execution timeout (`tokio::time::timeout`) for regular expression evaluation tasks.
- [ ] `#278`: Implement domain whitelist policy and session re-authentication for `FORWARD TO` actions.
- [ ] `#279`: Implement pre-flight DNS IP blacklisting (blocking `127.0.0.1`, `169.254.169.254`, private LAN subnets) with `max_redirects(0)` for outbound `CALL WEBHOOK` actions.
- [ ] `#280`: Implement HMAC-SHA256 request payload signatures (`X-Nuncio-Signature: t=timestamp,v1=hash`) for outbound webhooks.

### Phase 5: Multi-Shell Presentation Interfaces (100% Parity)
- [ ] `#281`: **POSIX CLI (`nuncio-cli`)**: Add `nuncio filter` subcommand suite (`list`, `create --sql`, `edit`, `delete`, `test --sql`, `export --format sql`, `import`, `logs`).
- [ ] `#282`: **Terminal TUI (`nuncio-tui`)**: Implement `AppMode::FilterRules` with NSQL code editor ↔ visual form builder toggle (`[s]`), dry-run previewer (`[t]`), priority re-ordering (`[J]`/`[K]`), and log inspector drawer (`[l]`).
- [ ] `#283`: **Desktop GUI (`nuncio-gui`)**: Build glassmorphic React `<FilterRulesModal />` component with "Visual Builder" vs "NSQL Query Editor" tabs, live syntax diagnostics, dry-run split pane, and execution log inspector tab.
- [ ] `#284`: **Native MCP (`nuncio-mcp`)**: Register `nuncio_filter_create`, `nuncio_filter_edit`, `nuncio_filter_delete`, `nuncio_filter_test`, `nuncio_filter_logs`, `nuncio_filter_export` tools and stream `nuncio://filters` resource.

### Phase 6: Integration Testing & Verification
- [ ] `#285`: Create end-to-end integration test suite verifying 1,000,000 message batch performance, zero-warning compiler build, zero-clippy lints, and 100% test pass rate across workspace crates.

---

## 4. Acceptance Criteria & Quality Gates

1. **Zero Warnings Gate**: `cargo check --workspace` yields 0 warnings.
2. **Zero Clippy Lints Gate**: `cargo clippy --workspace -- -D warnings` yields 0 lints.
3. **100% Test Pass Gate**: All unit, integration, and E2E system tests pass cleanly.
4. **Memory Constraint Gate**: Processing 1,000,000 messages in batch mode MUST NOT exceed **10 MB peak RAM**.
5. **ReDoS Timeout Gate**: Unsafe regex expressions MUST be aborted within **50 milliseconds**.
6. **SQLi Immunization Gate**: Zero raw string format interpolation into SQL query buffers.
7. **Multi-Shell Parity Gate**: 100% 1:1 capability parity verified across CLI, TUI, GUI, and MCP interfaces.
