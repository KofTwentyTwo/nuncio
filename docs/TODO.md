# TODO: Nuncio Roadmap & Task Tracking

> **Authoritative Source of Truth**: All task tracking, issue status, and milestone progress are authoritatively maintained on GitHub:
> - **GitHub Project Board**: [Nuncio Roadmap Project #5](https://github.com/users/KofTwentyTwo/projects/5)
> - **GitHub Milestones**: [KofTwentyTwo/nuncio/milestones](https://github.com/KofTwentyTwo/nuncio/milestones) (`v0.1.0` through `v1.0.0`)
> - **GitHub Issues**: [KofTwentyTwo/nuncio/issues](https://github.com/KofTwentyTwo/nuncio/issues)

---

## Stage 1: Core Engine & Storage Infrastructure (`v0.1.0`)
- [ ] [#5 - M1.1: Event Bus Channels (CoreCommand, CoreEvent, mpsc, watch)](https://github.com/KofTwentyTwo/nuncio/issues/5) (Assigned: `claude`)
- [ ] [#6 - M1.2: Account & Configuration Schema (AccountConfig)](https://github.com/KofTwentyTwo/nuncio/issues/6) (Assigned: `claude`)
- [ ] [#7 - M1.3: SQLite Engine & WAL Migrations (sqlx)](https://github.com/KofTwentyTwo/nuncio/issues/7) (Assigned: `claude`)
- [ ] [#8 - M1.4: SQLite FTS5 Trigram Indexing & Triggers](https://github.com/KofTwentyTwo/nuncio/issues/8) (Assigned: `claude`)
- [ ] [#9 - M1.5: OS Vault Integration (keyring) & Fallback](https://github.com/KofTwentyTwo/nuncio/issues/9) (Assigned: `claude`)
- [ ] [#10 - M1.6: Attachment Blob Encryption (age & AES-GCM)](https://github.com/KofTwentyTwo/nuncio/issues/10) (Assigned: `claude`)

## Stage 2: Mail Protocol Engine & MIME Parser (`v0.2.0`)
- [ ] M2.1: Zero-Copy MIME Parser (`mail-parser` & `bytes::Bytes`) (Assigned: `claude`)
- [ ] M2.2: Outbound SMTP Transport (`lettre` async Tokio client) (Assigned: `claude`)
- [ ] M2.3: JMAP Client Protocol Engine (`jmap-client` `Email/changes`) (Assigned: `claude`)
- [ ] M2.4: IMAP IDLE Dual-Socket Manager (`async-imap` IDLE push stream) (Assigned: `claude`)
- [ ] M2.5: Protocol Server Mocks (`wiremock` & `MockMailBackend`) (Assigned: `claude`)

## Stage 3: Calendar Engine & Recurrences (`v0.3.0`)
- [ ] M3.1: iCal/vCard to JSCalendar Parser (`calcard`) (Assigned: `claude`)
- [ ] M3.2: Recurrence Expansion Engine (`rrule` finite window math) (Assigned: `claude`)
- [ ] M3.3: CalDAV REPORT Client (`reqwest` + `quick-xml`) (Assigned: `claude`)
- [ ] M3.4: CalDAV Server Mocks (`wiremock`) (Assigned: `claude`)

## Stage 4: Command Line Interface (`v0.4.0`)
- [ ] M4.1: CLI Subcommand Parser (`clap` v4) (Assigned: `codex`)
- [ ] M4.2: JSON Pipeline Output Formatting (`--json`) (Assigned: `codex`)
- [ ] M4.3: Headless CLI E2E Integration Suite (Assigned: `codex`)

## Stage 5: Terminal User Interface (`v0.5.0`)
- [ ] M5.1: TUI Layout & Navigation (`ratatui` + `crossterm`) (Assigned: `agy-worker`)
- [ ] M5.2: Mail Viewer & Composer (Assigned: `agy-worker`)
- [ ] M5.3: HTML Email Text Renderer (`html2text`) (Assigned: `agy-worker`)
- [ ] M5.4: Calendar Grid Views (Assigned: `agy-worker`)

## Stage 6: Sandboxed Desktop GUI (`v0.6.0`)
- [ ] M6.1: Tauri v2 IPC Bridge (Assigned: `agy-worker`)
- [ ] M6.2: Untrusted HTML Email Sandboxing (`<iframe sandbox>` + CSP) (Assigned: `agy-worker`)
- [ ] M6.3: Native OS System Integration (a11y, system tray, notifications) (Assigned: `agy-worker`)

## Stage 7: Integration, Audit & Hardening (`v0.7.0`)
- [ ] M7.1: Full Workspace E2E Integration Suite (Assigned: `agy`)
- [ ] M7.2: Security & Memory Audit (Assigned: `agy`)

## Stage 8: Production Release (`v1.0.0`)
- [ ] M1.0: Cross-Platform Automated Release Pipeline & Binary Publishing (Assigned: `agy`)
