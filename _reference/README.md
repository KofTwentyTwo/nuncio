# `_reference/` — archived pre-gRPC presentation shells

This directory holds three presentation shells that were removed from the
Cargo workspace (backlog **0.D.2**, GH-145) as part of shrinking the
workspace to just the engine, the daemon, and the reference CLI:

- `nuncio-tui` — the Ratatui terminal UI shell
- `nuncio-gui` — the Rust GUI core plus its `src-tauri` Tauri desktop shell
  and its `ui/` React/Vite frontend
- `nuncio-mcp` — the MCP (Model Context Protocol) server shell for LLM tool
  access

They were built directly against the in-process `nuncio-core`/`nuncio-mail`/
`nuncio-cal`/`nuncio-store`/`nuncio-filter`/`nuncio-contacts` library APIs,
from the pre-gRPC phase of the project. Per the target architecture in
[`docs/ROADMAP.md`](../docs/ROADMAP.md), `nunciod` is moving to publish a
versioned gRPC API over loopback, and every presentation shell other than
the in-repo `nuncio-cli` reference client is planned to live in its own
separate native client repository (**Phase 5 — Client projects**):
`nuncio-tui`, `nuncio-gui-macos`, `nuncio-gui-windows`, `nuncio-mcp`, etc.

**This code is retained intact, not deleted**, so it can be mined for UI/UX
ideas, keybindings, view layout, and MCP tool-surface design when those
Phase 5 client repos are built against the new gRPC API. It is **not**
built, checked, or tested by the root Cargo workspace — none of these
directories are members of the workspace `Cargo.toml`, so
`cargo build --workspace`, `cargo check-all`, and `cargo test --workspace`
never touch them.

Each crate's internal `path = "../..."` dependencies on the engine crates
have been updated to point at `../../crates/<name>` (e.g.
`_reference/nuncio-tui/Cargo.toml` now points at `../../crates/nuncio-core`)
so the crate sources still resolve correctly relative to their new location.
They are **not guaranteed to build standalone** as-is: they still declare
`*.workspace = true` fields (`version`, `edition`, `lints`, and workspace
`[dependencies]` aliases) that only resolve inside a Cargo workspace, and
they are not their own workspace root. Restoring a standalone build (for
mining or resurrection) will require either vendoring those workspace
values directly into each `Cargo.toml`, or giving `_reference/` its own
`[workspace]` root.
