# Nuncio Sovereign Mail & Calendar Suite — Official Wiki

<p align="center">
  <img src="assets/nuncio_app_icon.jpg" alt="Nuncio Application Icon" width="128" style="border-radius: 24px;" />
</p>

<h3 align="center">A New Way to Do Email &amp; Calendars.</h3>

<p align="center">
  <a href="https://nuncio.mx">Official Website: nuncio.mx</a> •
  <a href="https://github.com/KofTwentyTwo/nuncio">GitHub Repository</a> •
  <a href="https://github.com/KofTwentyTwo/nuncio/releases/tag/v1.0.0">Releases &amp; Downloads</a>
</p>

---

## 1. Visual Application Overview

![Nuncio Desktop Application Preview](assets/nuncio_gui_hero.jpg)

---

## 2. Four Great Interfaces & Central Daemon Architecture

Nuncio operates on a **Hybrid Daemon-First Architecture**. Centralized state management, SQLite WAL persistence, credential security enclaves, and protocol synchronizers reside inside a standalone background daemon (`nunciod`). Four decoupled interfaces communicate with `nunciod` over native IPC socket streams:

![Nuncio Suite Architecture Topology](assets/topology.svg)

---

## 3. Wiki Navigation Map

- **[Architecture Specification](Architecture-Specification)**: C4 topology, 4-byte IPC frame structure, SQLite WAL concurrency, and OS Keyring enclave security.
- **[Four Great Interfaces](Four-Great-Interfaces)**: In-depth usage guide for POSIX CLI, Vim Ratatui TUI, Desktop GUI, and Native MCP AI Agent interface.
- **[NSQL Filter Language Specification](NSQL-Filter-Language-Specification)**: Full NSQL compiler pipeline, AST node rules, `ON ACCOUNT` wildcard matching, and signed HTTP webhooks.
- **[Database Corruption & Self Healing](Database-Corruption-Self-Healing)**: Multi-stage corrupt DB detection, backup isolation, table salvage, and automatic remote resync.
- **[Multi Platform Release & Auto Update](Multi-Platform-Release-Auto-Update)**: GitHub Releases matrix, Homebrew tap installation, `.tar.gz`/`.zip`/`.dmg`/`.msi`/`.AppImage` packages, and SHA-256 self-updaters.

---

## 4. Quickstart Installation

```bash
# macOS & Linux via Homebrew
brew install koftwentytwo/tap/nuncio

# Cargo Package Manager
cargo install nuncio-cli nuncio-tui nuncio-mcp nunciod

# Direct One-Line Installer
curl -fsSL https://nuncio.mx/install.sh | sh
```
