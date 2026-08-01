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
  `Accounts`, `Mail`, `Filters`, `Export`, `Audit`, all bearer-token authenticated;
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
        :: override if needed: --imap-host --imap-port --smtp-host --smtp-port --imap-mode --smtp-mode
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

## Following & auditing the daemon

`nunciod` runs a layered `tracing` subscriber (`crates/nunciod/src/logging.rs`);
`nuncio-cli` runs its own, separate, stderr-only subscriber for its own logs.
Neither ever writes a bearer token or an account password to a log line.

### Daemon log levels

`nunciod`'s console (stderr) and rotating file sinks share one filter,
resolved once at startup with this precedence:

1. **`NUNCIO_LOG`** — a `tracing-subscriber` `EnvFilter` directive (e.g.
   `info`, `debug`, `nunciod=debug,tower=warn`). Wins over `RUST_LOG` if set
   to a non-blank value.
2. **`RUST_LOG`** — same directive syntax; used when `NUNCIO_LOG` is
   unset/blank.
3. Default: `info`.

A malformed directive never crashes the daemon — it falls back to the
default and logs a `warn!` once the subscriber is live.

```
:: run the daemon at debug level for one session
set NUNCIO_LOG=debug
target\release\nunciod.exe
```

### Log file location

The rotating file sink writes daily-rotated files next to the database, under
a `logs/` sibling directory: `<data_dir>\logs\nunciod.log.<date>` (e.g.
`%USERPROFILE%\.nuncio\logs\nunciod.log.2026-07-30`). If the log directory
cannot be created, the daemon degrades to stderr-only logging (with a
`warn!`) rather than failing to start.

### JSON log output

Set `NUNCIO_LOG_FORMAT=json` to switch the **file** sink to one JSON object
per line, for machine ingestion (e.g. shipping to a log aggregator). The
console sink always stays human-readable, since it's for an operator watching
a terminal. Any other value (or unset) keeps the file sink human-readable too.

### Correlating one request's logs (`request_id`)

Every gRPC call `nunciod` handles opens a root tracing span carrying a
per-process `request_id` (`<process-start-tag>-<counter>`), the RPC method
path, and the peer address; every log line emitted while that call is
handled — auth accept/reject, handler-level events, the terminal
`gRPC request completed` line with status + elapsed time — inherits it. To
follow one request end-to-end, grep the daemon log for its `request_id`:

```
:: human-readable file sink
findstr "a1b2c3d4e5f6-00000017" "%USERPROFILE%\.nuncio\logs\nunciod.log.2026-07-30"

:: JSON file sink (NUNCIO_LOG_FORMAT=json), pick a request_id from the
:: completion line's fields, then filter every line for it:
findstr "\"request_id\":\"a1b2c3d4e5f6-00000017\"" "%USERPROFILE%\.nuncio\logs\nunciod.log.2026-07-30"
```

(On macOS/Linux, use `grep` in place of `findstr`.) There is currently no way
to read a request's `request_id` back from the CLI's own output — it is
visible only in the daemon's logs, since it is a server-side correlation
identifier assigned when the RPC arrives.

### CLI verbosity (`-v`/`-vv`/`-vvv`)

`nuncio-cli` logs to stderr only — command output (including `--json`
payloads) always goes to stdout, so redirecting or piping stdout is never
polluted by log lines. By default the CLI logs only warnings. Repeat `-v` to
raise the level:

```
nuncio-cli.exe mail sync              :: warn only (default)
nuncio-cli.exe -v mail sync           :: info
nuncio-cli.exe -vv mail sync          :: debug
nuncio-cli.exe -vvv mail sync         :: trace
```

`NUNCIO_LOG`/`RUST_LOG`, if set (same precedence as the daemon: `NUNCIO_LOG`
beats `RUST_LOG`), override the `-v` count entirely — useful for scoping a
directive to a specific module (e.g. `NUNCIO_LOG=nuncio_cli=trace`) without
turning on trace logging for every dependency `-vvv` would also enable.

## The dogfooding loop

Use it for real; when something breaks or feels wrong, tell me (or I'll hit it
too). I reproduce against the real path, fix it, commit, and you pull `dev`.
Because the toolchain is pinned, every fix is validated locally exactly as CI
would validate it — so fixes land without waiting on CI.
