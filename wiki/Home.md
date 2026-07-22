# Welcome to the Nuncio Wiki

Nuncio ([nuncio.mx](https://nuncio.mx)) is a cross-platform mail and calendar application written in Rust, supporting Windows, macOS, and Linux with native Command Line (CLI), Terminal (TUI), and Desktop (GUI) interfaces.

## Wiki Navigation

- [[Architecture Specification]]: Decoupled engine design, crate breakdown, protocol traits, storage schema, search indexing, and security architecture.
- [[Roadmap and Phases]]: High-level project phases, task tracking, and milestone goals.

## Architecture Highlights

Nuncio follows a library-first architecture:
- Core libraries (`nuncio-core`, `nuncio-mail`, `nuncio-cal`, `nuncio-store`) contain 100% of domain rules, protocol engines, offline sync, SQLite FTS5 search indexing, and cryptographic key storage.
- Thin presentation shells (`nuncio-cli`, `nuncio-tui`, and `nuncio-gui`) consume async event streams (`tokio::sync::watch`) and dispatch command enums (`tokio::sync::mpsc`).

## Codebase Repository
- GitHub Repository: [KofTwentyTwo/nuncio](https://github.com/KofTwentyTwo/nuncio)
- GitHub Project Board: [Nuncio Roadmap Project #5](https://github.com/users/KofTwentyTwo/projects/5)
