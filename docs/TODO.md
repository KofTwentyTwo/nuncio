# TODO: Nuncio Roadmap & Task Tracking

## Initial Setup
- [x] Create workspace plan and roadmap documentation
- [ ] Initialize Cargo workspace structure with crate scaffolding
- [ ] Initialize git repository locally
- [ ] Create `KofTwentyTwo/nuncio` repository on GitHub
- [ ] Push initial commit to origin `main`
- [ ] Create GitHub Project / Issues for roadmap tracking

## Phase 1: Core Engine & Local Storage
- [ ] `nuncio-core` workspace engine setup and event loop
- [ ] `nuncio-store` SQLite storage layer and FTS5 indexing schema
- [ ] Account config and credential keyring management

## Phase 2: Mail & Calendar Protocol Libraries
- [ ] `nuncio-mail` IMAP sync manager with IDLE support
- [ ] `nuncio-mail` JMAP protocol client implementation
- [ ] `nuncio-cal` CalDAV sync engine and iCalendar parser

## Phase 3: Terminal User Interface (TUI)
- [ ] `nuncio-tui` Ratatui layout architecture
- [ ] Mailbox list, thread viewer, and composer screens
- [ ] Calendar grid views (day, week, month)

## Phase 4: Desktop GUI & Packaging
- [ ] Desktop UI shell integration
- [ ] Windows, Linux, and macOS cross-compilation pipeline
