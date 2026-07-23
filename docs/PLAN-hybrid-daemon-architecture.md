# Nuncio Hybrid Daemon-First Architecture Blueprint (`nunciod` + IPC)

> **Master Architecture & Code Refactoring Specification**
> Developed by Master Orchestrator Agent and Subagent Panel (Systems & IPC, Security, Software Architecture).

---

## Executive Overview & System Topology

Nuncio uses a **Hybrid Daemon-First Architecture**, where a single background process (`nunciod`) acts as the single source of truth for storage, network protocols (IMAP, SMTP, CalDAV), credential enclaves, and core state management. All four presentation shells (**CLI**, **TUI**, **GUI**, and **MCP LLM Interface**) communicate with `nunciod` via low-latency native OS IPC transports.

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

---

## 1. Native Socket Transport & Security Specification

### 1.1 OS Socket Transports
- **Linux & macOS**: UNIX Domain Socket (`~/.nuncio/nuncio.sock`) created with mode `0600`. Peer credentials validated at kernel level (`SO_PEERCRED` on Linux, `LOCAL_PEERCRED` on macOS) matching caller UID against `nunciod` process UID.
- **Windows**: Named Pipe (`\\.\pipe\nuncio-ipc`) with Discretionary ACLs restricting access exclusively to the active Windows User SID (`TokenUser`).
- **Loopback TCP Fallback (`127.0.0.1:9422`)**: Restricted to containerized environments, secured by a 256-bit cryptographically random bearer token (`~/.nuncio/.ipc_bearer_token`) verified via constant-time comparison (`subtle::ConstantTimeEq`).

### 1.2 Framing & Wire Protocol
- **Binary Framing Header**: 4-byte Big-Endian length prefix (`u32::to_be_bytes()`) with strict 16MB frame ceiling.
- **Protocol Format**: JSON-RPC 2.0 request, response, and server notification payloads (`system.ping`, `system.state`, `mail.sync_all`, `events.notify`).

---

## 2. Auto-Spawning & Daemon Lifecycle

1. **Absence Detection**: `IpcClient::connect_or_spawn()` attempts connecting to native IPC socket. If socket connection fails, absence is confirmed.
2. **Background Detachment**: Shell executes `nunciod` binary as a detached process (`setsid()` on POSIX, `CREATE_NO_WINDOW` on Windows) with stdio streams redirected to `NUL` or `nunciod.log`.
3. **Exponential Backoff Retry Loop**: Shell retries socket connection over 5 attempts (50ms $\rightarrow$ 100ms $\rightarrow$ 200ms $\rightarrow$ 400ms $\rightarrow$ 800ms).
4. **PID File Lock**: `nunciod` acquires an advisory lock on `~/.nuncio/nunciod.pid`. Graceful shutdown via `SIGTERM` flushes SQLite WAL buffers and cleans up PID/socket files.

---

## 3. Shell Refactoring Architecture

- **`nuncio-cli`**: Routes commands via `IpcClient`. If daemon is offline, falls back to direct SQLite `DatabaseEngine` reads.
- **`nuncio-tui`**: Connects `Ratatui` event loop to `IpcClient::subscribe_events()`, rendering real-time push updates alongside crossterm input events.
- **`nuncio-gui`**: Tauri v2 `IpcBridge` handles window commands via `IpcClient` and emits `core-event` payloads to the React frontend.
- **`nuncio-mcp`**: Tools execute queries directly against FTS5 search index and route actions (`nuncio_mail_send`) to `IpcClient` with Human-in-the-Loop (HITL) prompt enforcement.

---

## 4. Credential Enclave & Security Policy

1. **Enclave Isolation**: OS Keyring passwords, OAuth refresh tokens, and `age` private keys stay **100% inside `nunciod` heap memory**. Presentation shells never handle plaintext secrets.
2. **Heap Memory Hygiene**: Secret structures implement `ZeroizeOnDrop` with OS memory page locking (`libc::mlock` / `VirtualLock`).
3. **MCP Tool Safety & HITL Interception**: External LLM tools (`nuncio_mail_send`) operate under `Agent-Restricted` RBAC role. Outbound emails trigger a Human-in-the-Loop approval prompt in GUI/TUI before SMTP transmission. Append-only audit logging is maintained at `~/.nuncio/audit.log`.
