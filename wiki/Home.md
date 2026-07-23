# Welcome to the Nuncio Wiki

Official domain: [nuncio.mx](https://nuncio.mx)

> **Etymology**: Derived from the Latin verb ***nūntiō*** ("I announce", "I declare", "I deliver a message") and noun ***nūntius*** ("messenger", "courier", "bearer of tidings"). Nuncio is built as the ultimate cross-platform messenger and calendar courier for Windows, macOS, and Linux.

---

## Single Source of Truth for Planning & Execution

Project planning, issue tracking, atomic micro-milestones, and subagent assignments are authoritatively managed directly on GitHub:

- **GitHub Project Board**: [Nuncio Roadmap Project #5](https://github.com/users/KofTwentyTwo/projects/5)
- **GitHub Milestones**: [KofTwentyTwo/nuncio/milestones](https://github.com/KofTwentyTwo/nuncio/milestones) (`v0.1.0` through `v1.0.0`)
- **GitHub Issues**: [KofTwentyTwo/nuncio/issues](https://github.com/KofTwentyTwo/nuncio/issues)

---

## Navigation

- [[Architecture Specification|Architecture-Specification]]
- [[Executive Review|Executive-Review]]
- [[Roadmap and Project Phases|Roadmap-and-Phases]]

---

## Workspace Structure

- `crates/nuncio-core`: Workspace management, account orchestration, async event loop.
- `crates/nuncio-mail`: IMAP4rev1, JMAP (RFC 8620/8621), and SMTP protocol engines.
- `crates/nuncio-cal`: CalDAV and iCalendar (RFC 5545) client libraries.
- `crates/nuncio-store`: Encrypted local persistence and SQLite FTS5 search indexing.
- `crates/nuncio-cli`: Scriptable command-line interface binary for Unix pipes and automation.
- `crates/nuncio-tui`: Terminal user interface powered by Ratatui.
- `crates/nuncio-gui`: Native desktop graphical user interface shell.
