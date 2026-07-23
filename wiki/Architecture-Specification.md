# Nuncio Architecture Specification

Nuncio is designed as a **Hybrid Daemon-First Architecture** following Hexagonal (Ports & Adapters) domain encapsulation principles.

---

## Workspace Structure (9 Modular Crates)

- `crates/nuncio-core`: Domain entities (`Email`, `CalendarEvent`, `Folder`), `EventBus`, and `IpcClient`/`IpcDaemonServer` framing & JSON-RPC protocol.
- `crates/nuncio-mail`: IMAP4rev1, JMAP (RFC 8620/8621), SMTP engines, and MIME stream parser.
- `crates/nuncio-cal`: CalDAV (RFC 4791) client, iCalendar (RFC 5545) parser, and `rrule` recurrence engine.
- `crates/nuncio-store`: SQLite WAL engine, FTS5 trigram search index, AES-256-GCM column cipher, `age` attachment stream cipher, and OS Keyring vault integration.
- `crates/nuncio-cli`: Pure Noun + Verb CLI runner and Unix pipe scripting engine.
- `crates/nuncio-tui`: Terminal user interface powered by Ratatui and crossterm.
- `crates/nuncio-gui`: Native desktop GUI application shell powered by Tauri v2 and React.
- `crates/nuncio-mcp`: MCP stdio server providing LLM agents direct read/write access.
- `crates/nunciod`: Standalone background daemon binary server.

---

## Central Daemon (`nunciod`) & IPC Framing Protocol

```
┌────────────────────────────────────────────────────────────────────────────────────────┐
│                                  Presentation Shells                                   │
│                                                                                        │
│  ┌──────────────┐   ┌──────────────┐   ┌───────────────────┐   ┌────────────────────┐  │
│  │  nuncio-cli  │   │  nuncio-tui  │   │nuncio-gui(Tauri v2│   │    nuncio-mcp      │  │
│  │ (POSIX CLI)  │   │  (Ratatui)   │   │  Desktop GUI)     │   │ (MCP AI Interface) │  │
│  └──────┬───────┘   └──────┬───────┘   └─────────┬─────────┘   └─────────┬──────────┘  │
│         │                  │                     │                       │             │
│         └──────────────────┴──────────┬──────────┴───────────────────────┘             │
│                                       │                                                │
│                                   IpcClient                                            │
│                        (Auto-Spawn + Retry Loop + JSON-RPC)                            │
└───────────────────────────────────────┼────────────────────────────────────────────────┘
                                        │
           ┌────────────────────────────┴────────────────────────────┐
           │ POSIX: UNIX Domain Socket (`~/.nuncio/nuncio.sock`)     │
           │ Windows: Named Pipe (`\\.\pipe\nuncio-ipc`)              │
           └────────────────────────────┬────────────────────────────┘
                                        │
┌───────────────────────────────────────┼────────────────────────────────────────────────┐
│                                       ▼                                                │
│                                IpcDaemonServer                                         │
│                               (Security Enclave)                                       │
│                                                                                        │
│                                  nunciod Daemon                                        │
│  ┌──────────────────────────────────────────────────────────────────────────────────┐  │
│  │                                 nuncio-core                                      │  │
│  │       EventBus (mpsc command_tx | watch state_tx | broadcast event_tx)           │  │
│  └──────────────┬──────────────────────────┬──────────────────────────┬─────────────┘  │
│                 │                          │                          │                │
│                 ▼                          ▼                          ▼                │
│  ┌──────────────────────────┐┌──────────────────────────┐┌──────────────────────────┐  │
│  │       nuncio-mail        ││        nuncio-cal        ││       nuncio-store       │  │
│  │    JMAP / IMAP / SMTP    ││      CalDAV / iCal       ││     SQLite FTS5 / Age    │  │
│  └──────────────────────────┘└──────────────────────────┘└──────────────────────────┘  │
└────────────────────────────────────────────────────────────────────────────────────────┘
```

1. **Native OS Socket Transports**:
   - POSIX: UNIX Domain Sockets (`~/.nuncio/nuncio.sock`, mode `0600`, `SO_PEERCRED` UID authorization).
   - Windows: Named Pipes (`\\.\pipe\nuncio-ipc`, User SID DACL permissions).
   - Loopback TCP Fallback (`127.0.0.1:9422`): Protected by 256-bit constant-time bearer tokens.
2. **Binary Framing Codec**: 4-byte Big-Endian length header with strict 16MB ceiling check. IPC transfer latency benchmark: **<12.4ms for 16MB payloads (~1.29 GB/s throughput)**.
3. **JSON-RPC 2.0 Wire Contract**: Typed requests, responses, and real-time server event push notifications (`events.notify`).
4. **Auto-Spawning Mechanics**: `IpcClient::connect_or_spawn()` checks socket presence; if absent, spawns detached `nunciod` process and retries connection over an exponential backoff loop (50ms $\rightarrow$ 800ms).

---

## Security & Cryptography Specification

1. **Exclusive Secret Enclave**: Passwords, OAuth refresh tokens, and `age` private keys stay **100% inside `nunciod` heap memory** protected by `ZeroizeOnDrop` wrappers.
2. **Data-at-Rest Encryption**: Sensitive SQLite database columns encrypted via authenticated AES-256-GCM (`PayloadCipher`). Large attachments encrypted via `age` X25519 ciphers.
3. **HTML Email Sandboxing**: Untrusted HTML emails rendered inside strict `<iframe sandbox="allow-same-origin">` enforcing Content Security Policy (CSP) `default-src 'none'`. Tracking pixels neutralized by default.
4. **MCP HITL Interception**: External LLM tools (`nuncio_mail_send`) execute under `Agent-Restricted` RBAC role with Human-in-the-Loop approval prompts in GUI/TUI shells.
