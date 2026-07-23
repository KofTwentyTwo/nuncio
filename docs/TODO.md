# TODO: Nuncio Roadmap & Task Tracking

> **Authoritative Single Source of Truth**: All task tracking, issue status, and milestone progress are authoritatively maintained on GitHub:
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
- [ ] [#11 - M2.1: Zero-Copy MIME Parser (mail-parser & bytes::Bytes)](https://github.com/KofTwentyTwo/nuncio/issues/11) (Assigned: `claude`)
- [ ] [#12 - M2.2: Outbound SMTP Transport (lettre async Tokio client)](https://github.com/KofTwentyTwo/nuncio/issues/12) (Assigned: `claude`)
- [ ] [#13 - M2.3: JMAP Client Protocol Engine (jmap-client Email/changes)](https://github.com/KofTwentyTwo/nuncio/issues/13) (Assigned: `claude`)
- [ ] [#14 - M2.4: IMAP IDLE Dual-Socket Manager (async-imap IDLE push stream)](https://github.com/KofTwentyTwo/nuncio/issues/14) (Assigned: `claude`)
- [ ] [#15 - M2.5: Protocol Server Mocks (wiremock & MockMailBackend)](https://github.com/KofTwentyTwo/nuncio/issues/15) (Assigned: `claude`)

## Stage 3: Calendar Engine & Recurrences (`v0.3.0`)
- [ ] [#16 - M3.1: iCal/vCard to JSCalendar Parser (calcard)](https://github.com/KofTwentyTwo/nuncio/issues/16) (Assigned: `claude`)
- [ ] [#17 - M3.2: Recurrence Expansion Engine (rrule finite window math)](https://github.com/KofTwentyTwo/nuncio/issues/17) (Assigned: `claude`)
- [ ] [#18 - M3.3: CalDAV REPORT Client (reqwest + quick-xml)](https://github.com/KofTwentyTwo/nuncio/issues/18) (Assigned: `claude`)
- [ ] [#19 - M3.4: CalDAV Server Mocks (wiremock)](https://github.com/KofTwentyTwo/nuncio/issues/19) (Assigned: `claude`)

## Stage 4: Command Line Interface (`v0.4.0`)
- [ ] [#20 - M4.1: CLI Subcommand Parser (clap v4)](https://github.com/KofTwentyTwo/nuncio/issues/20) (Assigned: `codex`)
- [ ] [#21 - M4.2: JSON Pipeline Output Formatting (--json)](https://github.com/KofTwentyTwo/nuncio/issues/21) (Assigned: `codex`)
- [ ] [#22 - M4.3: Headless CLI E2E Integration Suite](https://github.com/KofTwentyTwo/nuncio/issues/22) (Assigned: `codex`)

## Stage 5: Terminal User Interface (`v0.5.0`)
- [ ] [#23 - M5.1: TUI Layout & Navigation (ratatui + crossterm)](https://github.com/KofTwentyTwo/nuncio/issues/23) (Assigned: `agy-worker`)
- [ ] [#24 - M5.2: Mail Viewer & Composer (ratatui)](https://github.com/KofTwentyTwo/nuncio/issues/24) (Assigned: `agy-worker`)
- [ ] [#25 - M5.3: HTML Email Text Renderer (html2text)](https://github.com/KofTwentyTwo/nuncio/issues/25) (Assigned: `agy-worker`)
- [ ] [#26 - M5.4: Calendar Grid Views (ratatui)](https://github.com/KofTwentyTwo/nuncio/issues/26) (Assigned: `agy-worker`)

## Stage 6: Sandboxed Desktop GUI (`v0.6.0`)
- [ ] [#27 - M6.1: Tauri v2 IPC Bridge](https://github.com/KofTwentyTwo/nuncio/issues/27) (Assigned: `agy-worker`)
- [ ] [#28 - M6.2: Untrusted HTML Email Sandboxing (iframe + CSP)](https://github.com/KofTwentyTwo/nuncio/issues/28) (Assigned: `agy-worker`)
- [ ] [#29 - M6.3: Native OS System Integration (a11y, system tray, notifications)](https://github.com/KofTwentyTwo/nuncio/issues/29) (Assigned: `agy-worker`)
- [ ] [#30 - M6.4: GUI Cross-Platform Build Validation](https://github.com/KofTwentyTwo/nuncio/issues/30) (Assigned: `agy-worker`)

## Stage 7: Integration, Audit & Hardening (`v0.7.0`)
- [ ] [#31 - M7.1: Full Workspace E2E Integration Suite](https://github.com/KofTwentyTwo/nuncio/issues/31) (Assigned: `agy`)
- [ ] [#32 - M7.2: Security & Memory Audit](https://github.com/KofTwentyTwo/nuncio/issues/32) (Assigned: `agy`)

## Stage 8: Production Release (`v1.0.0`)
- [ ] [#33 - M1.0: Production Release & Cross-Platform Packaging](https://github.com/KofTwentyTwo/nuncio/issues/33) (Assigned: `agy`)
