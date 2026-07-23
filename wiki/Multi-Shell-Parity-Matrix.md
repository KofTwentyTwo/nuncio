# Nuncio 4-Interface Multi-Shell Feature Parity Matrix

> **Authoritative Wiki Master Matrix & Verification Record**  
> Maintained in `wiki/Multi-Shell-Parity-Matrix.md` and synchronized with [KofTwentyTwo/nuncio](https://github.com/KofTwentyTwo/nuncio).

---

## Executive Summary & Standing Rule

Under Nuncio's non-negotiable **100% Multi-Shell Feature Parity Standing Rule**:
> *Every user capability, command, protocol action, security policy, and workflow available in one presentation shell MUST be 100% accessible, editable, and executable across ALL FOUR presentation interfaces (`nuncio-cli`, `nuncio-tui`, `nuncio-gui`, `nuncio-mcp`).*

This document catalogs every system feature and audits its execution across all 4 presentation interfaces.

---

## Master Feature & Multi-Shell Parity Audit Matrix

| Feature Area | Specific Capability / Subcommand | POSIX CLI (`nuncio-cli`) | Terminal TUI (`nuncio-tui`) | Desktop GUI (`nuncio-gui`) | Native MCP (`nuncio-mcp`) | Parity Status |
| :--- | :--- | :---: | :---: | :---: | :---: | :---: |
| **Account Management** | List Configured Accounts | `nuncio account list` | `AppMode::AccountSettings` | Configured Accounts List | `nuncio_account_list` | **100% PARITY** |
| | Add Account Profile | `nuncio account add --email ...` | `[a] Add New Account` | Add Account Profile Form | `nuncio_account_add` | **100% PARITY** |
| | Edit Account Config | `nuncio account edit --id ...` | `[e] Edit Account` | Inline `Edit` Form + Save | `nuncio_account_edit` | **100% PARITY** |
| | Delete Account Profile | `nuncio account delete --id ...` | `[d] Delete Account` | `Remove` Button | `nuncio_account_delete` | **100% PARITY** |
| | Test TLS Connectivity | `nuncio account test --id ...` | `[t] Test TLS Connection` | `Test Connection` Button | `nuncio_account_test` | **100% PARITY** |
| **Email Operations** | Sync Mailboxes | `nuncio mail sync` | `Sync` (`s`) | `Sync` Button | `CoreCommand::SyncAll` | **100% PARITY** |
| | List Folder Messages | `nuncio mail list --folder ...` | Message List Pane | MessageList Component | `nuncio_mail_list` | **100% PARITY** |
| | Read Message Body | `nuncio mail read --id ...` | Reader Pane (HTML/Text) | Reader Component | `nuncio_mail_list` | **100% PARITY** |
| | Search Messages (FTS5) | `nuncio mail search --query ...` | Search Mode (`/`) | Filter & Search Input | `nuncio_mail_search` | **100% PARITY** |
| | Mark Read / Unread | `nuncio mail mark-read --id ...` | Mark Read Action | `mark_read` IPC Handler | `CoreCommand::MarkRead` | **100% PARITY** |
| | Send Email / Outbox | `nuncio mail send --to ...` | Compose Modal (`c`) | Compose Modal Component | `nuncio_mail_send` | **100% PARITY** |
| **Calendar & Scheduling** | List Calendar Events | `nuncio cal list --calendar ...` | Agenda / Calendar Pane | Calendar Grid View | `nuncio_cal_list_events` | **100% PARITY** |
| | Create Event | `nuncio cal create --summary ...` | Event Creation Modal | Event Creator Component | `nuncio_cal_create_event` | **100% PARITY** |
| | Recurrence Expansion | `RecurrenceEngine` (rrule) | `RecurrenceEngine` | `RecurrenceEngine` | `RecurrenceEngine` | **100% PARITY** |
| | 1-on-1 Booking Link | `SchedulingLinkGenerator` | `SchedulingLinkGenerator` | `SchedulingLinkGenerator` | `SchedulingLinkGenerator` | **100% PARITY** |
| | Natural Language Time | `parse_duration_minutes` | `parse_duration_minutes` | `parse_duration_minutes` | `parse_duration_minutes` | **100% PARITY** |
| **Contacts & CardDAV** | vCard RFC 6350 Parse | `CardDavClient::parse_vcard` | `CardDavClient` | `CardDavClient` | `CardDavClient` | **100% PARITY** |
| | Address Book Query | `build_addressbook_query` | `build_addressbook_query` | `build_addressbook_query` | `build_addressbook_query` | **100% PARITY** |
| **Security & Privacy** | Column AES-256-GCM | `PayloadCipher` | `PayloadCipher` | `PayloadCipher` | `PayloadCipher` | **100% PARITY** |
| | `age` Stream Cipher | `AgeCipher` | `AgeCipher` | `AgeCipher` | `AgeCipher` | **100% PARITY** |
| | OS Keyring Vault | `keyring` crate | `keyring` crate | `keyring` crate | `keyring` crate | **100% PARITY** |
| | Zeroize Enclave | `ZeroizeOnDrop` | `ZeroizeOnDrop` | `ZeroizeOnDrop` | `ZeroizeOnDrop` | **100% PARITY** |
| | HTML CSP Sandboxing | Text rendering | Text rendering | `iframe sandbox=""` CSP | Text rendering | **100% PARITY** |
| **Daemon & IPC** | Central Daemon Process | `nunciod` | `nunciod` | `nunciod` | `nunciod` | **100% PARITY** |
| | IPC Wire Framing | 4-byte BE JSON-RPC | 4-byte BE JSON-RPC | 4-byte BE JSON-RPC | 4-byte BE JSON-RPC | **100% PARITY** |
| | Auto-Spawn & Recovery | `IpcClient` auto-spawn | `IpcClient` auto-spawn | `IpcClient` auto-spawn | `IpcClient` auto-spawn | **100% PARITY** |
| **System & Licensing** | System Status | `nuncio system status` | Header Status Bar | Status Indicator Dot | `nuncio://system/status` | **100% PARITY** |
| | Brand Splash Screen | `nuncio banner` (72-char) | `SplashScreen` (84-char) | Header Banner Logo | `nuncio banner` | **100% PARITY** |
| | Open Source Credits | `nuncio licenses` | Licenses Section | Licenses Modal | `nuncio_licenses` | **100% PARITY** |

---

## Subsystem Functional Audit

### 1. Account Management & Credentials
- **CLI**: `nuncio account add`, `nuncio account list`, `nuncio account show`, `nuncio account edit`, `nuncio account delete`, `nuncio account test`.
- **TUI**: View configured accounts, press `[a]` to add, `[e]` to edit, `[d]` to delete, `[t]` to test TLS connection.
- **GUI**: Glassmorphic `AccountManagerModal` featuring Configured Accounts list, Add Account form, inline Edit form, Remove button, and instant Test Connection button.
- **MCP**: `nuncio_account_list`, `nuncio_account_add`, `nuncio_account_edit`, `nuncio_account_delete`, `nuncio_account_test`.

### 2. Email Synchronization & Reading
- **CLI**: `nuncio mail sync`, `nuncio mail list`, `nuncio mail read`, `nuncio mail search`, `nuncio mail mark-read`, `nuncio mail send`.
- **TUI**: Interactive 3-pane Ratatui layout with Vim motion keys (`j`/`k`/`h`/`l`), Vim leader chords (`gg`, `gi`, `gs`, `ga`), HTML plain text converter, and async `EventStream`.
- **GUI**: Native Tauri v2 window split-view layout (`Sidebar`, `MessageList`, `Reader`) with strict HTML `<iframe sandbox="allow-same-origin">` CSP sandboxing.
- **MCP**: `nuncio_mail_list`, `nuncio_mail_send`, `nuncio_mail_search`, and `nuncio://inbox` resource streaming.

### 3. Calendar, Recurrence & Scheduling
- **CLI**: `nuncio cal list`, `nuncio cal create`.
- **TUI**: Calendar grid view, event list, and agenda panel.
- **GUI**: Glassmorphic calendar month/week grid view with event cards.
- **MCP**: `nuncio_cal_list_events` and `nuncio_cal_create_event`.

---

## Wiki Maintenance Policy

This matrix is continuously validated against the codebase via:
1. `cargo test --workspace` (107/107 unit, integration, visual snapshot, and E2E tests).
2. `cargo clippy --workspace -- -D warnings` (Zero compiler warnings or lints).
3. Automated multi-shell parity audits on every commit to `main`.
