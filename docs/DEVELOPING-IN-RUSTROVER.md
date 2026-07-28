# Developing Nuncio in RustRover

A practical guide to building, running, testing, and dogfooding Nuncio from inside
JetBrains RustRover. For the plain-CLI version of the run/use steps see
[`RUNNING.md`](RUNNING.md); for the architecture and plan see
[`ARCHITECTURE.md`](ARCHITECTURE.md) and [`ROADMAP.md`](ROADMAP.md).

---

## 1. One-time setup

The repo ships a shared RustRover configuration (`.idea/` — run configs, code
style, and Cargo registration are tracked; per-machine state like `workspace.xml`
is gitignored). On a fresh checkout:

1. **Open the folder** in RustRover and let it index. It reads the workspace from
   the root `Cargo.toml` — you should see the 9 members
   (`nuncio-core`, `-mail`, `-cal`, `-contacts`, `-store`, `-filter`, `-proto`,
   `nunciod`, `nuncio-cli`). After any pull that adds/moves crates, use
   **File → Reload Cargo Project** (RustRover usually offers a banner).
2. **Toolchain — must be native (not WSL).** This project builds and runs
   locally; nothing remote is needed. Check **Settings → Rust → Toolchain
   location** points at your native Windows rustup
   (`%USERPROFILE%\scoop\apps\rustup\current\.cargo\bin` here), *not* a
   `\\wsl$\…` / `\\wsl.localhost\…` path. If it's WSL, the daemon can't reach the
   Windows keyring and won't start. The channel (1.97.1) is pinned by
   `rust-toolchain.toml` and installed automatically.
3. **Clippy as the linter** (recommended): **Settings → Rust → External Linters →
   Clippy**, tick "Run external linter on the fly". Now the editor flags the same
   `-D warnings` issues the commit gate enforces.
4. **rustfmt on save** is already configured (`.idea/codeStyles`), so files
   reformat as you save.

---

## 2. What's what (project map)

- **`nunciod`** — the daemon. **This is the product**: it owns the SQLite store,
  credentials (Windows Credential Manager), protocol sync, and serves the gRPC API
  on `127.0.0.1:9420`.
- **`nuncio-cli`** — the reference client. A thin gRPC client of the daemon; how
  you drive it by hand.
- **`nuncio-proto`** — the versioned gRPC contract (`.proto` + generated stubs).
- **Engine libs** — `nuncio-core` (event bus, domain types), `-store` (DB, search,
  ciphers, keyring), `-mail` (IMAP/JMAP/SMTP), `-cal`, `-contacts`, `-filter` (NSQL).
- **`_reference/`** — the archived TUI/GUI/MCP shells from the old architecture,
  **not part of the workspace** (kept only to mine when we build the real client
  repos later). Don't develop here.

---

## 3. Running it (dogfooding)

Pick a configuration from the **run-config dropdown** (top-right, next to ▶/🐞).

### Start the daemon
Select **Run nunciod (daemon)** → ▶ (Shift+F10). It builds, then runs in the
foreground in a Run tool-window tab; you'll see it bind the gRPC API on
`127.0.0.1:9420`. **Leave it running.** Stop with the red ■.

On first run it creates `%USERPROFILE%\.nuncio\nuncio.db` and provisions its keys +
the CLI's bearer token in the Windows Credential Manager. Override the DB path with
the `NUNCIO_DB_PATH` env var (add it under the config's *Environment variables*),
or the gRPC address with `NUNCIO_GRPC_ADDR`.

### Drive it with the CLI
The CLI talks to the running daemon, so **start the daemon first**. Two ways:

- **One-click configs** (in the dropdown): *Run nuncio-cli (system status)*,
  *(account list)*, *(mail list)*, *(mail sync)*.
- **Terminal** (Alt+F12) for anything else — this is best for varied dogfooding
  and for commands that take input:
  ```
  cargo run -p nuncio-cli -- account add --email you@kof22.com   # prompts for password
  cargo run -p nuncio-cli -- mail read --id <id>
  cargo run -p nuncio-cli -- mail send --to a@b.com --subject hi --body test
  cargo run -p nuncio-cli -- --json mail list
  ```
  `account add` defaults to `mail.kof22.com` (993/465); override with
  `--imap-host/--imap-port/--smtp-host/--smtp-port`. Add `--json` anywhere for
  scriptable output. If the daemon isn't running you'll get an honest
  "daemon unreachable" error, not a hang.

> Heads-up: the IMAP/SMTP engines have so far only been tested against mocks —
> **your first real `mail sync`/`mail send` is the first live-server test.** When
> something breaks, note the error; it's a fix, not a surprise.

---

## 4. Testing

RustRover's Cargo test integration is the fastest inner loop.

- **Run/debug ONE test:** click the green ▶ **gutter icon** beside any
  `#[test]`/`#[tokio::test]` fn (or beside the `mod tests`). Right-click for
  **Run** vs **Debug**. Debug drops you at breakpoints with full inspection (the
  MSVC debugger is set up).
- **Run a crate's tests:** right-click the crate in the Project view → **Run
  tests**, or use the **Cargo Test All** config for the whole workspace
  (`cargo test --workspace`).
- **Results:** the **Test** tool window shows a pass/fail tree; use *Rerun Failed
  Tests* while iterating.
- **Tests are self-contained.** Every test spins up its own ephemeral daemon,
  mock IMAP/SMTP, and an in-memory `MockKeyring`, on a temp database. So you do
  **not** need the daemon running, a network, or the real keyring to run tests —
  only manual CLI dogfooding needs the live daemon.

---

## 5. Quality gates (run before you commit/push)

The Git pre-commit hook runs `fmt` + `clippy -D warnings` (all targets) + the full
test suite on every `git commit`, so make these green first:

- **Cargo Verify (fmt + clippy + tests)** — the whole gate in one click (= `cargo verify`).
- **Cargo Check All** — just clippy, all targets, warnings-as-errors.
- **Cargo Test All** — the suite.
- **Cargo Coverage** — informational `llvm-cov` (needs `cargo-llvm-cov`; not a gate).
- **Cargo Build Release (daemon + CLI)** — optimized build of the two binaries
  (= `cargo build-release`), i.e. what you ship for real dogfooding.

Because the toolchain is pinned (1.97.1) and native, **green here == green in CI** —
so a clean *Cargo Verify* means a clean commit and (once CI minutes are back) a
clean pipeline.

---

## 6. The run configurations at a glance

| Config | Command | Needs daemon running? |
| :--- | :--- | :---: |
| Run nunciod (daemon) | `run -p nunciod` | — (it *is* the daemon) |
| Run nuncio-cli (system status) | `run -p nuncio-cli -- system status` | yes |
| Run nuncio-cli (account list) | `run -p nuncio-cli -- account list` | yes |
| Run nuncio-cli (mail list) | `run -p nuncio-cli -- mail list` | yes |
| Run nuncio-cli (mail sync) | `run -p nuncio-cli -- mail sync` | yes |
| Cargo Build Release (daemon + CLI) | `build-release` | no |
| Cargo Check All | `check-all` | no |
| Cargo Test All | `test-all` | no |
| Cargo Verify (fmt + clippy + tests) | `verify` | no |
| Cargo Coverage | `cov` (informational) | no |

To make your own: **Run → Edit Configurations → + → Cargo Command**, set the
command (e.g. `run -p nuncio-cli -- mail read --id abc`), and tick *Store as
project file* to share it (it lands in `.idea/runConfigurations/`).

---

## 7. A normal development loop

1. Edit code; the editor lints (clippy) and formats-on-save as you go.
2. Run the nearest **test via its gutter icon** to check your change; **Debug** it
   if you need to step through.
3. When the feature/fix is done, run **Cargo Verify** — that's the commit gate.
4. Commit (the hook re-runs the gate). Conventional Commits, feature branches off
   `dev` (see `CLAUDE.md`).
5. To see it end-to-end, run **nunciod** and drive it from the Terminal.

---

## 8. Gotchas / FAQ

- **"It builds/runs via Ubuntu."** The Rust toolchain is set to WSL — fix in
  Settings → Rust → Toolchain location (see §1.2). Nothing here needs WSL.
- **"CLI says: nunciod daemon unreachable."** Start **Run nunciod** first; the CLI
  is a client of the daemon.
- **"clippy fails on `unwrap()` in a test."** Production code forbids
  `unwrap`/`expect`/`panic`; test code is exempt (allowed at each crate root). Put
  the code in a `#[cfg(test)]` context, or handle the error in non-test code.
- **"Where's the TUI/GUI?"** Archived under `_reference/`; they'll be rebuilt as
  separate native client repos (roadmap Phase 5). They are not in the workspace.
- **CI is currently paused** (GitHub Actions minutes). That's fine — the pinned
  native toolchain makes your local *Cargo Verify* equivalent to CI.

---

## 9. Where to read more

- [`ROADMAP.md`](ROADMAP.md) — the plan and target architecture.
- [`BACKLOG.md`](BACKLOG.md) — engineering-ready stories.
- [`RUNNING.md`](RUNNING.md) — the plain-terminal run/use guide.
- [`ARCHITECTURE.md`](ARCHITECTURE.md) — how the daemon + gRPC API + engines fit together.
- [`../CLAUDE.md`](../CLAUDE.md) — conventions, gates, and the IDE summary.
