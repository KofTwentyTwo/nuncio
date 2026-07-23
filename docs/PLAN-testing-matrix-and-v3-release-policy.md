# Nuncio Unified V3 Release Policy & Exhaustive Testing Matrix

> **Master Release Mandate & Cross-Platform / Cross-UI Verification Specification**  
> **Author**: Master Orchestrator Agent (`agy`)  
> **Repository**: [KofTwentyTwo/nuncio](https://github.com/KofTwentyTwo/nuncio)

---

## 1. Unified V3 Public Release Mandate

> **POLICY MANDATE**: No public GA release tag (`v3.0.0`) or binary distribution (`.msi`, `.dmg`, `.AppImage`) will be cut or published until **Phase V3 (Platform & AI Automation)** is 100% feature complete, fully tested, and working across **all 4 presentation UIs** on **Windows, macOS, and Linux**.

```
┌────────────────────────────────────────────────────────────────────────────────────────┐
│                              UNIFIED V3 RELEASE MANDATE                                │
│                                                                                        │
│   Phase V1: Core Infrastructure & GA Baseline (CLI, TUI, GUI, MCP, nunciod Daemon)     │
│                                           │                                            │
│                                           ▼                                            │
│   Phase V2: Enterprise Protocols (JMAP, OAuth2, CardDAV, GPG/PGP, Scheduling Links)    │
│                                           │                                            │
│                                           ▼                                            │
│   Phase V3: Platform & AI (Local LLM Triage, Summaries, WASM Plugins, Cloud Relay)     │
│                                           │                                            │
│                                           ▼                                            │
│ ┌────────────────────────────────────────────────────────────────────────────────────┐ │
│ │                         GA PUBLIC RELEASE TAG (`v3.0.0`)                           │ │
│ │            Requires 100% Feature Parity & Verification across ALL 4 UIs           │ │
│ └────────────────────────────────────────────────────────────────────────────────────┘ │
└────────────────────────────────────────────────────────────────────────────────────────┘
```

---

## 2. CI/CD & Local Mock Protocol Testing Architecture

To guarantee offline, deterministic validation in CI/CD and local environments, external network services and OS vaults are 100% isolated via mock layers:

| Target System / Protocol | Mock Infrastructure Provider | Verification Strategy |
| :--- | :--- | :--- |
| **IMAP4rev1** | `wiremock::MockServer` + `tokio-rustls` | Simulates TLS connection handshakes, `CAPABILITY`, `SELECT`, `FETCH`, and `IDLE` push notification streams. |
| **SMTP Transport** | `wiremock::MockServer` + `lettre` | Validates `EHLO`, STARTTLS (587), Implicit TLS (465), AUTH LOGIN/PLAIN, and multipart MIME construction. |
| **CalDAV & CardDAV** | `wiremock::MockServer` | Simulates HTTP PROPFIND, REPORT queries, XML multistatus responses, and iCalendar/vCard payloads. |
| **JMAP (RFC 8620/8621)** | `wiremock::MockServer` | Simulates JMAP session discovery (`/.well-known/jmap`), `Email/get`, `Email/changes`, and batch JSON POST methods. |
| **OS Keyring Vault** | `MockKeyring` Provider | Simulates native OS credential storage without touching system keychains (`libsecret`, Keychain, Credential Manager). |
| **Database Persistence** | `DatabaseEngine::connect_ephemeral()` | Instantiates isolated SQLite WAL databases with auto-cleanup upon test drop. |

---

## 3. Full Combinations Cross-UI & Cross-OS Test Matrix

Testing enforces full combinatorial validation across **4 Presentation UIs** $\times$ **3 Target OS Platforms**:

```
                       FULL COMBINATORIAL TEST MATRIX

                Linux (Ubuntu)      macOS (Sonoma)     Windows (Win11)
              ┌──────────────────┬──────────────────┬──────────────────┐
  nuncio-cli  │  POSIX Sockets   │  POSIX Sockets   │   Named Pipes    │
              │  --json & Pipes  │  --json & Pipes  │  --json & Pipes  │
              ├──────────────────┼──────────────────┼──────────────────┤
  nuncio-tui  │ Ratatui Terminal │ Ratatui Terminal │ Ratatui Terminal │
              │ Crossterm <16ms  │ Crossterm <16ms  │ Crossterm <16ms  │
              ├──────────────────┼──────────────────┼──────────────────┤
  nuncio-gui  │ Tauri v2 WebKitGTK│ Tauri v2 WKWebView│ Tauri v2 WebView2│
              │ React Glassmorphic│ React Glassmorphic│ React Glassmorphic│
              ├──────────────────┼──────────────────┼──────────────────┤
  nuncio-mcp  │ Stdio JSON-RPC   │ Stdio JSON-RPC   │ Stdio JSON-RPC   │
              │ Local AI Agents  │ Local AI Agents  │ Local AI Agents  │
              └──────────────────┴──────────────────┴──────────────────┘
```

---

## 4. End-to-End (E2E) Test Suite Workflow Coverage

E2E integration test suites validate complete multi-shell concurrent workflows:

1. **`daemon_e2e_test.rs`**: Launches `nunciod`, connects `IpcClient` from CLI, TUI, GUI, and MCP simultaneously, and verifies state synchronization and push event broadcasting across all 4 shells.
2. **`cli_system_test.rs`**: Executes pure `<Noun> <Verb>` commands (`account add`, `mail sync`, `mail search`, `cal list`), verifying deterministic JSON output envelopes (`--json`).
3. **`tui_system_test.rs`**: Simulates crossterm keyboard events (`j`/`k`/`h`/`l`/`gg`/`gi`), verifying 3-pane split rendering and sub-16ms frame updates.
4. **`gui_system_test.rs`**: Validates Tauri v2 IPC bridge bindings, window creation, and HTML email iframe CSP sandboxing.
5. **`mcp_system_test.rs`**: Validates JSON-RPC 2.0 stdio server initialization, tool calls (`nuncio_mail_send`), resource reads (`nuncio://mail/inbox`), and HITL safety prompts.
