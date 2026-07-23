# PLAN: Nuncio Multi-Agent Implementation Strategy

## Goal
Build Nuncio ([nuncio.mx](https://nuncio.mx)), a high performance cross-platform mail and calendar application in Rust, using a library-first architecture with skinny shells for CLI, TUI, and GUI across Windows, macOS, and Linux.

## Multi-Agent Execution Strategy

Antigravity (`agy`) operates as the **Master Orchestrator Agent**, managing high-level task decomposition, roadmap tracking, multi-agent dispatching, quality gate validation (`cargo verify`, `cargo cov`), git branch workflows, and wiki documentation.

Subagents are assigned specialized domain responsibilities based on their model strengths:

```
┌────────────────────────────────────────────────────────────────────────┐
│                   Antigravity Master Agent (agy)                       │
│    Roadmap Management, Subagent Dispatching, Quality Verification     │
└────────┬──────────────────────┬───────────────────────┬────────────────┘
         │                      │                       │
         ▼                      ▼                       ▼
┌──────────────────┐  ┌──────────────────┐    ┌──────────────────┐
│   Claude Code    │  │   OpenAI Codex   │    │Antigravity Worker│
│ Engine & Domain  │  │   CLI & Automation│   │  TUI & GUI Shells│
└──────────────────┘  └──────────────────┘    └──────────────────┘
```

### Subagent Role Breakdown

1. **Claude Code Subagent (`claude`)**:
   - **Assigned Domain**: Headless engine crates (`crates/nuncio-core`, `crates/nuncio-mail`, `crates/nuncio-cal`, `crates/nuncio-store`).
   - **Key Tasks**: `sqlx` SQLite WAL migrations, FTS5 trigram triggers, `mail-parser` zero-copy MIME parsing, `jmap-client` WebSocket streams, `async-imap` dual-socket manager, `calcard` JSCalendar conversions, `rrule` recurrence algorithms, `age` blob encryption, and 100% unit test line coverage enforcement.

2. **OpenAI Codex Subagent (`codex`)**:
   - **Assigned Domain**: CLI tool & pipeline automation (`crates/nuncio-cli`).
   - **Key Tasks**: `clap` v4 subcommand derive parsers (`nuncio mail list|read|send`, `nuncio cal list|add`), JSON output formatters (`--json`), shell autocomplete generation, and automated data generator scripts.

3. **Antigravity Worker Subagent (`agy`)**:
   - **Assigned Domain**: Presentation shells & CI/CD packaging (`crates/nuncio-tui`, `crates/nuncio-gui`).
   - **Key Tasks**: `ratatui` + `crossterm` TUI screen layouts, `html2text` hyperlink text formatting, `Tauri v2` desktop GUI shell wrapper, native OS webview sandboxing (`nuncio-mail://` scheme), and GitHub Actions cross-platform matrix build pipelines.

---

## High Level Roadmap Phases & Assigned Subagent

### Phase 1: Engine Foundation & Storage
- `nuncio-core` workspace engine setup & Tokio event loop -> **Claude Code Subagent**
- `nuncio-store` SQLite WAL schema & FTS5 trigram indexes -> **Claude Code Subagent**
- `keyring` OS vault integration & `age` payload encryption -> **Claude Code Subagent**
- Unit test suite with 100% line coverage gate -> **Claude Code Subagent**

### Phase 2: Mail & Calendar Protocol Engine Libraries
- `nuncio-mail` `mail-parser`, `jmap-client`, `async-imap`, `lettre` -> **Claude Code Subagent**
- `nuncio-cal` `calcard`, `rrule`, `reqwest` CalDAV REPORT queries -> **Claude Code Subagent**
- Protocol mocks (`wiremock`, `MockMailBackend`, `MockKeyring`) -> **Claude Code Subagent**

### Phase 3: Command Line & Terminal User Interfaces
- `nuncio-cli` subcommands (`mail`, `cal`, `sync`, `status`) & JSON pipes -> **OpenAI Codex Subagent**
- `nuncio-tui` Ratatui layout, composer, and `html2text` renderer -> **Antigravity Worker Subagent**

### Phase 4: Desktop GUI Shell & Cross-Platform Packaging
- `nuncio-gui` Tauri v2 wrapper calling `nuncio-core` IPC -> **Antigravity Worker Subagent**
- Untrusted HTML email iframe sandboxing & CSP headers -> **Antigravity Worker Subagent**
- GitHub Actions cross-platform release pipeline -> **Antigravity Master Agent (agy)**
