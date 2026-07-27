# Running Nuncio (dogfooding guide)

How to build, run, and use the daemon + CLI locally. These steps are **verified**
on Windows (the daemon boots and the CLI round-trips over real gRPC + the OS
keyring). Commands use `nuncio-cli.exe`; on macOS/Linux drop the `.exe`.

> **Status:** pre-alpha. The mail spine (accounts, sync, read, send) is real and
> works, but the IMAP/SMTP engines have so far only been exercised against mock
> servers — **your first real `mail sync` is the first live-server test**, and it
> may surface real-world issues (server quirks, auth methods, TLS). That's the
> point of dogfooding; report anything odd and it becomes a fix.

## 1. Build

```
cargo build-release          # alias for: build --release -p nunciod -p nuncio-cli
```

Produces two self-contained executables (copy them anywhere; they only need the
network and the OS credential store):
- `target\release\nunciod.exe` — the daemon (the product)
- `target\release\nuncio-cli.exe` — the CLI (how you drive it)

(Use plain `cargo build -p nunciod -p nuncio-cli` for a faster debug build while
we're iterating.)

## 2. Start the daemon

```
target\release\nunciod.exe
```

It runs in the foreground and:
- serves the gRPC API on `127.0.0.1:9420` (loopback only) — services `System`,
  `Accounts`, `Mail`, `Filters`, all bearer-token authenticated;
- opens its database at `%USERPROFILE%\.nuncio\nuncio.db` (override with
  `NUNCIO_DB_PATH`); the `~/.nuncio` directory is created on first run;
- provisions its keys + the CLI's bearer token in **Windows Credential Manager**
  (never on disk);
- the autonomous auto-updater is intentionally **disabled** (pending a Phase 4
  fail-open fix).

Leave it running; `Ctrl+C` stops it. Use the CLI from a second terminal.

## 3. Use it

```
:: confirm the CLI reaches the daemon (verified):
nuncio-cli.exe system status                 ::  -> "Idle (nunciod v0.1.0, 127.0.0.1:9420)"

:: add your real account (defaults target mail.kof22.com:993/:465; prompts for the
:: password, which goes straight to the OS keyring):
nuncio-cli.exe account add --email you@kof22.com
        :: override if needed: --imap-host --imap-port --smtp-host --smtp-port --imap-tls-mode
nuncio-cli.exe account list

:: the real spine:
nuncio-cli.exe mail sync                      :: real IMAP fetch -> local store
nuncio-cli.exe mail list
nuncio-cli.exe mail read --id <message-id>
nuncio-cli.exe mail search --query "invoice"
nuncio-cli.exe mail send --to a@b.com --subject "hi" --body "test"

:: filters, folders, etc.:
nuncio-cli.exe filter list
nuncio-cli.exe folder list
```

Add `--json` to any command for machine-readable output. `nuncio-cli.exe <noun> --help`
lists every verb and flag.

## The dogfooding loop

Use it for real; when something breaks or feels wrong, tell me (or I'll hit it
too). I reproduce against the real path, fix it, commit, and you pull `dev`.
Because the toolchain is pinned, every fix is validated locally exactly as CI
would validate it — so fixes land without waiting on CI.
