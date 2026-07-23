# Nuncio Multi-Shell Parity & V1-V3 Strategic Execution Plan

> **Master Engineering Execution Strategy & Standing Rule Enforcement**  
> **Author**: Master Orchestrator Agent (`agy`)  
> **Repository**: [KofTwentyTwo/nuncio](https://github.com/KofTwentyTwo/nuncio)

---

## 1. Standing Rule: 100% Multi-Shell Feature Parity Equivalence

It is a binding architectural constraint that **all four presentation shells MUST maintain 100% feature parity with each other**:

1. **`nuncio-cli` (POSIX CLI)**: Every domain action MUST have a pure `<Noun> <Verb>` CLI subcommand outputting deterministic JSON envelopes (`--json`) for scriptability.
2. **`nuncio-tui` (Terminal TUI)**: Every domain action MUST have a single-key / leader-key Vim shortcut (`keybindings.rs`) and a Ratatui interactive modal/panel.
3. **`nuncio-gui` (Tauri v2 Desktop GUI)**: Every domain action MUST have a visual React component (`App.tsx`, `Sidebar`, `MessageList`, `Reader`) and command palette entry (`Cmd+K`).
4. **`nuncio-mcp` (Native LLM Agent UI)**: Every domain action MUST have an exposed MCP JSON-RPC 2.0 tool (`tools.rs`), resource URI resolver (`resources.rs`), and prompt template (`prompts.rs`).

```
┌────────────────────────────────────────────────────────────────────────────────────────┐
│                        100% MULTI-SHELL FEATURE PARITY RULE                            │
│                                                                                        │
│        New Feature Request / Domain Capability (Mail, Calendar, Security, AI)          │
│                                           │                                            │
│       ┌───────────────────┬───────────────┴───────────────┬───────────────────┐        │
│       ▼                   ▼                               ▼                   ▼        │
│ ┌───────────┐       ┌───────────┐                   ┌───────────┐       ┌───────────┐  │
│ │nuncio-cli │       │nuncio-tui │                   │nuncio-gui │       │nuncio-mcp │  │
│ │(Noun+Verb)│       │ (Ratatui) │                   │(Tauri v2) │       │ (MCP Stdio│  │
│ └─────┬─────┘       └─────┬─────┘                   └─────┬─────┘       └─────┬─────┘  │
│       │                   │                               │                   │        │
│       └───────────────────┴───────────────┬───────────────┴───────────────────┘        │
│                                           │                                            │
│                                           ▼                                            │
│                                IpcClient / IpcDaemonServer                             │
│                                    (Central nunciod)                                   │
└────────────────────────────────────────────────────────────────────────────────────────┘
```

---

## 2. Phase Execution Matrix (V1 $\rightarrow$ V2 $\rightarrow$ V3)

### Phase 1: V1 GA Launch & Polish (Immediate Execution)
- **Goal**: Complete 100% feature parity across all 4 UIs for core Mail & Calendar operations (`sync`, `list`, `read`, `search`, `send`, `event_create`, `account_manage`).
- **Milestones**:
  - `v0.8.0` (Hybrid Daemon Integration): Complete IPC client wiring for TUI and GUI shells over `nunciod`.
  - `v0.9.0` (UX & Performance Polish): Implement TUI async `crossterm::event::EventStream` with `tokio::select!` for sub-16ms frame target. Implement Vim leader chord state machine (`gg`, `gi`, `gs`).
  - `v1.0.0` (GA Release): Cross-platform installer packaging (`.msi`, `.dmg`, `.AppImage`), zero warnings, and 100% unit test coverage gate.

### Phase 2: V2 Power User & Enterprise Capabilities
- **Goal**: JMAP transport, OAuth2 PKCE token rotation, CardDAV contact sync, OpenPGP / S/MIME, multi-timezone calendar overlays, 1-on-1 scheduling links, meeting link auto-detection.
- **UI Parity Rollout**:
  - `nuncio-cli`: `nuncio jmap sync`, `nuncio auth oauth`, `nuncio contact list`, `nuncio cal schedule-link`.
  - `nuncio-tui`: Contactpicker overlay, multi-timezone column toggle (`t`), GPG key manager modal (`K`).
  - `nuncio-gui`: React CardDAV contact sidebar, timezone timeline resizer, video meeting join button.
  - `nuncio-mcp`: `nuncio_jmap_sync`, `nuncio_contact_search`, `nuncio_cal_create_schedule_link`.

### Phase 3: V3 Platform & AI Automation
- **Goal**: Local LLM agent auto-categorization & triage, automated meeting summaries, Lua/WASM plugin runtime, webhook workflow engine, age encrypted cloud sync relay.
- **UI Parity Rollout**:
  - `nuncio-cli`: `nuncio ai categorize`, `nuncio ai summarize --id msg_123`, `nuncio plugin load --path plugin.wasm`.
  - `nuncio-tui`: AI Triage pane (`A`), Meeting summary drawer (`M`), Plugin status monitor (`P`).
  - `nuncio-gui`: React AI smart categories (`Important`, `Paper Trail`, `Feed`), WASM plugin settings modal.
  - `nuncio-mcp`: `nuncio_ai_triage_inbox`, `nuncio_ai_summarize_thread`, `nuncio_plugin_execute`.

---

## 3. Subagent Execution Assignment Table

| Subagent Role | Primary Crate Assignments | Key Responsibilities |
| :--- | :--- | :--- |
| **Master Orchestrator (`agy`)** | Workspace root, `docs/` | High-level roadmap tracking, atomic milestone assignment, GitHub Project Board synchronization, quality gate checks. |
| **Protocol & Core Engineer (`claude`)** | `nuncio-core`, `nuncio-mail`, `nuncio-cal`, `nuncio-store` | JMAP HTTP RFC 8620/8621, IMAP/SMTP transport, CalDAV/CardDAV engine, SQLite FTS5 trigrams, `age` ciphers, 100% test coverage. |
| **CLI & Automation Engineer (`codex`)** | `nuncio-cli` | Noun + Verb CLI command parsing, `--json` envelope schemas, Unix pipe streaming, scripting automation. |
| **UI & Presentation Engineer (`agy-worker`)** | `nuncio-tui`, `nuncio-gui`, `nuncio-mcp` | Ratatui TUI event loop (<16ms target), Vim motion state machine, Tauri v2 React frontend, MCP JSON-RPC stdio server. |
| **Security Engineer (`security_auditor`)** | `nuncio-store`, `nunciod` | Credential memory hygiene (`ZeroizeOnDrop`), OS Keyring integration, socket peer authorization (`SO_PEERCRED`), HTML iframe CSP sandboxing, MCP HITL safety prompts. |

---

## 4. Continuous Quality Gate Enforcement

1. **Compiler Gate**: `cargo check --workspace` MUST return **0 warnings** (`-D warnings`).
2. **Clippy Gate**: `cargo clippy --workspace -- -D warnings` MUST return **0 lints**.
3. **Test Suite Gate**: `cargo test --workspace` MUST pass **100% of tests** across all 9 crates.
4. **Safety Gate**: Zero `.unwrap()` or `.expect()` calls allowed in production non-test code.
5. **Parity Gate**: Every PR adding a feature must update all 4 presentation crates (`cli`, `tui`, `gui`, `mcp`).
