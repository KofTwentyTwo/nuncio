# TODO: Nuncio Roadmap & Task Tracking

## Initial Setup
- [x] Create workspace plan and roadmap documentation
- [x] Conduct deep crate research and author master architecture document (`docs/ARCHITECTURE.md`)
- [x] Define multi-agent execution plan (Master: `agy`, Subagents: `claude`, `codex`, `agy-worker`)
- [x] Initialize Cargo workspace structure with crate scaffolding (`nuncio-core`, `mail`, `cal`, `store`, `cli`, `tui`, `gui`)
- [x] Initialize git repository locally & create `KofTwentyTwo/nuncio` on GitHub
- [x] Push initial commit to origin `main` & create GitHub Project #5 / Issues #1-#4
- [x] Setup GitHub governance files (`SECURITY.md`, `CONTRIBUTING.md`, `CLAUDE.md`, `ci.yml`)
- [x] Enforce 100% unit test line coverage & `.cargo/config.toml` warnings-as-errors gate

## Phase 1: Core Engine & Local Storage
- [ ] `nuncio-core` engine orchestrator and async event bus (Assigned: `claude`)
- [ ] `nuncio-store` SQLite WAL database schema and FTS5 trigram indexes (Assigned: `claude`)
- [ ] `keyring` OS credential vault integration and `age` blob encryption (Assigned: `claude`)
- [ ] Ephemeral database test helpers and 100% unit test coverage verification (Assigned: `claude`)

## Phase 2: Mail & Calendar Protocol Libraries
- [ ] `nuncio-mail` `mail-parser` zero-copy MIME parser & `jmap-client` engine (Assigned: `claude`)
- [ ] `nuncio-mail` `async-imap` dual-socket manager & `lettre` SMTP sender (Assigned: `claude`)
- [ ] `nuncio-cal` `calcard` JSCalendar parser & `rrule` recurrence expansion engine (Assigned: `claude`)
- [ ] `nuncio-cal` `reqwest` + `quick-xml` WebDAV CalDAV REPORT queries (Assigned: `claude`)
- [ ] Offline protocol server mocks (`wiremock`, `MockMailBackend`, `MockKeyring`) (Assigned: `claude`)

## Phase 3: Command Line & Terminal User Interfaces
- [ ] `nuncio-cli` `clap` v4 subcommands (`mail`, `cal`, `sync`, `status`) and JSON output (Assigned: `codex`)
- [ ] `nuncio-tui` Ratatui layout engine, composer, and `html2text` renderer (Assigned: `agy-worker`)
- [ ] Headless E2E integration test suite for CLI and TUI commands (Assigned: `codex` / `agy-worker`)

## Phase 4: Desktop GUI & Packaging
- [ ] `nuncio-gui` Tauri v2 desktop UI shell wrapping `nuncio-core` IPC (Assigned: `agy-worker`)
- [ ] HTML email iframe sandboxing and `nuncio-mail://` attachment scheme (Assigned: `agy-worker`)
- [ ] Windows, Linux, and macOS automated release pipeline (Assigned: `agy`)
