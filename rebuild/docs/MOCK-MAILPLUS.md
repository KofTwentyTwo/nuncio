# Independent local MailPlus test services

The local MailPlus test environment uses independent Dovecot IMAP and Mailpit SMTP implementations. Neither imports Nuncio engine/provider types. `tests/imap/services.py` owns their disposable Docker composition, credentials, certificate authority, protocol readiness and cleanup. `tests/imap/proxy.py` adds deterministic protocol fault boundaries without owning mailbox or delivery state. The standalone mock and Rust control harness are implemented. Production IMAP account authentication and ingestion now have separate system and actual daemon/CLI suites. Durable IMAP/SMTP mutations remain in progress.

## Pinned services and local operation

The tested images are Dovecot2.4.5 (`sha256:c807be4fb5a97d9c3a90770569d3a6c4cbdcb36742ad41f90409cbd929166553`) and Mailpit1.31.1 (`sha256:98b916bd3c8d61f7633a52d3ea2f58d00620cb01ca57ab59edde68c347a95365`). The composition always uses these immutable references and refuses implicit pulls. Image downloads are a separate setup operation. Sources: [Dovecot image documentation](https://doc.dovecot.org/2.4.5/installation/docker.html), [Mailpit image documentation](https://mailpit.axllent.org/docs/install/docker/).

From the worktree root, run:

```sh
python3 rebuild/tests/imap/run-tests.py
```

This requires the pinned images already downloaded, Docker/Compose, Python3.11+ and OpenSSL. It uses synthetic accounts and numeric-loopback services. Exact test commands, exit statuses, logs and retained run artifacts live in `rebuild/test-results/imap-services/`. No account registration, provider credentials, OS trust-store changes or normal Nuncio installation is required.

For a standalone direct-server environment:

```sh
python3 rebuild/tests/imap/services.py start --directory "$PWD/rebuild/test-results/mailplus-local"
python3 rebuild/tests/imap/services.py stop --directory "$PWD/rebuild/test-results/mailplus-local"
```

Start requires a new directory and announces the path to ready.json. Read ports and local paths from that file after readiness succeeds. Stop removes only the uniquely named composition recorded in that directory; retained runtime files remain available for inspection. Repeated starts need new directories. For the full mock, use `mock-mailplus.py` as shown below; the direct-server command intentionally has no fault proxy.

Host listeners bind 127.0.0.1 with ephemeral ports. Startup requires a local Docker Unix socket; every container operation is pinned to that validated socket, and cleanup retains it with the unique project identity. Provider containers attach only to one internal network. Because Docker29.7.2 does not publish their ports directly, a pinned unprivileged HAProxy relay exposes six fixed TCP targets through a separate ingress network, with IPv4/IPv6 forwarding disabled. TLS and protocol bytes pass through unchanged. Startup and independent tests inspect network isolation and published bindings. Mailpit relaying, reverse DNS and automatic GitHub version checks are disabled. The host egress wrapper independently verifies loopback access and denied external traffic in parent/child processes. See [TESTING.md](TESTING.md) and current verification evidence; these controls do not establish live MailPlus compatibility or hosted CI success.

## Authentication and TLS

Each environment generates fresh synthetic alpha@example.test and beta@example.test passwords and a separate observation-API password. Values never appear in argv or test logs. The runtime directory is0700; credentials.json is0600. Individual server bind files are readable inside the rootless container and protected on the host by the private ancestor. The local CA and server certificate are generated per environment, expire after seven days, and cover localhost/127.0.0.1. The CA private key stays outside container mounts. No trust material is installed system-wide.

Dovecot runs rootless with a read-only root filesystem and temporary mail/runtime volumes. Its shipped executables require the documented SYS_CHROOT capability; other capabilities are dropped and no-new-privileges remains enabled. Dovecot requires TLS before password authentication. Both IMAPS and IMAP STARTTLS are exercised. Separate Mailpit instances require STARTTLS and implicit TLS respectively, with independent capture databases. Both require credentials; each observation HTTP API has separate Basic authentication. [Dovecot password files](https://doc.dovecot.org/latest/core/config/auth/databases/passwd_file.html), [Mailpit SMTP TLS/authentication](https://mailpit.axllent.org/docs/configuration/smtp/).

## Independent observations and fault behavior

Server conformance tests cover separate IMAP users, exact APPEND/FETCH bytes, modified UTF-7 and quoted mailbox names, UID COPY with COPYUID, UID-scoped expunge retaining an unrelated deleted UID, mailbox recreation changing UIDVALIDITY, malformed empty FETCH rejection, SMTP authentication, and two accepted deliveries with an identical Message-ID. Mailpit keeps all messages for the disposable run and does not deduplicate IDs. Strict RFC header validation is enabled.

Mailpit reception prepends Return-Path and Received trace headers. It also prepends a Bcc header for envelope recipients absent from the visible MIME recipient headers; a separate test requires that exact synthetic Bcc prefix and independently checks the authenticated-user tag and ReturnPath. Tests validate those exact prefixes and require every submitted MIME header/body byte to survive as an exact suffix; IMAP APPEND/FETCH requires complete byte equality. Captured messages are observed through Mailpit's API, independently of client results. [SMTP trace headers](https://www.rfc-editor.org/rfc/rfc5321.html#section-4.4), [Mailpit runtime options](https://mailpit.axllent.org/docs/configuration/runtime-options/).

The loopback TLS proxy records command counts and observed successful acknowledgements without recording credentials or payloads. Those counters supplement direct Dovecot/Mailpit observations. Successful SMTP MAIL/RCPT/DATA responses also produce a bounded envelope record with actual accepted recipients and an unstuffed wire-byte hash/size; MIME To/Cc headers cannot substitute for that envelope. Unknown protocol verbs are counted as UNKNOWN, never arbitrary credential-like input. Faults match protocol, command and before/after boundary. Supported actions are disconnect, an explicitly released hold, pre-dispatch rejection, and truncation of a FETCH response literal. Post-acceptance rejection is forbidden: accepted remote work can lose its acknowledgement, but the mock must not falsely model that as a positive rejection.

Capability profiles can hide MOVE, UIDPLUS, CONDSTORE and QRESYNC. Hidden MOVE/UID EXPUNGE commands are explicitly rejected; UIDPLUS response codes are removed when the profile lacks UIDPLUS. Hidden CONDSTORE also hides QRESYNC because QRESYNC implies CONDSTORE. Hidden extension modifiers are rejected, unknown ENABLE names are forwarded as unknown, and hidden mod-sequence responses are suppressed. Dovecot2.4.5 omits the empty ENABLED response for all-unknown names; this profile emits the RFC5161-required empty response after backend acceptance when all requested names were hidden. The underlying Dovecot state remains authoritative. These profiles cover the release command subset, not an independent implementation of every IMAP extension. [ENABLE](https://www.rfc-editor.org/rfc/rfc5161.html#section-3.1), [QRESYNC/CONDSTORE](https://www.rfc-editor.org/rfc/rfc7162.html#section-3.2.3). The proxy handles synchronized/non-synchronized literals, SASL continuations, STARTTLS and IDLE/DONE with asynchronous notifications. Its contract checks exact APPEND continuation count, independent remote copy/delivery counts after lost acknowledgements, withheld APPEND after storage, truncated FETCH with unchanged remote bytes (the fault remains pending across metadata-only FETCH), and zero accepted deliveries after rejection. Unexpected asynchronous exceptions fail the tests.

Current bounds: 64 simultaneous proxy sessions, 100 pending faults, 1,000 observed SMTP deliveries (further DATA is rejected before dispatch), 100 recipients per envelope, 64KiB framing/SMTP response lines, 70MiB literals/DATA, a 60-second session deadline and bounded TLS shutdown. These are test-service bounds; production resource/timeout acceptance is separate. Actual daemon/CLI adapter recovery and production resource acceptance remain. Passing these local services does not establish Synology MailPlus compatibility.


## Full standalone mock and out-of-band controls

```sh
python3 rebuild/tests/imap/mock-mailplus.py --directory "$PWD/rebuild/test-results/mailplus-full"
```

The process announces only `{"ready_file":".../mock-ready.json"}` after real protocol readiness and deterministic seeding. The private ready file contains schema version1, proxy ports (`imap`, `imaps`, `smtp`, `smtps`), local CA/credential paths, capability profile, and the server-Sent mode. Neither announcement nor ready file contains password values. Keep stdin/stdout attached to the controlling process. EOF, SIGTERM/SIGINT, or a shutdown request removes this composition; SIGKILL cannot run cleanup, so use `services.py stop --directory ...` with the retained runtime path.

Controls use one strict JSON object per line, at most96MiB including base64 fixtures. Every request is `{"id":"unique-request","control":{...}}`; replies preserve that ID and contain `ok` plus `result` or a fixed error category. Duplicate/unknown fields are rejected. Commands are serialized over private inherited pipes; no control TCP endpoint exists.

| Command | Required control fields besides command | Effect/evidence |
|---|---|---|
| seed | None | Pauses proxy admission, drops active clients, recreates backends, and reseeds both accounts with the same UIDs/UIDVALIDITY/MIME bytes; passwords/CA/proxy ports remain stable. |
| mailbox / raw | account, mailbox; raw also needs uid | Read-only independent UIDVALIDITY, UID/flags/hash/size observations or exact base64 bytes. |
| append / flags | account, mailbox; append: raw_base64; flags: uid, add, remove | External Dovecot changes. Flags accept explicit supported system flags. |
| uidvalidity | account, mailbox, value | Calls Dovecot's actual doveadm mailbox update; re-observes server state. |
| folder_create / folder_delete / folder_rename | account, mailbox; rename adds destination | Test-only external folder changes, including quoted/Unicode names. Folder administration is outside production product scope. |
| copy / expunge | account, mailbox, uid; copy adds destination | External UID COPY or UID-scoped deletion preserving unrelated Deleted messages. |
| smtp | Optional transport: starttls (default) or tls | Independent Mailpit capture count and message identities for that transport. |
| smtp_message / smtp_raw | transport, id | Captured metadata (including authenticated Username/Bcc) or original capture bytes. Reading metadata marks a capture read in Mailpit. |
| snapshot | None | Sanitized protocol request/positive-ack counters and accepted SMTP envelopes; independent server observations remain required. |
| inject | name, protocol, verb, phase, action | Named before/after fault, e.g. imap / UID COPY / after / disconnect. |
| wait_fault / release | name | Wait for a named boundary (10seconds) or release a withheld acknowledgement. |
| server_sent_status / wait_server_sent | None / count | Inspect optional server-managed Sent behavior or await a completed-copy count (15seconds). |
| shutdown | None | Acknowledge then cleanly stop the owned mock services. |

Example request: `{"id":"inspect","control":{"command":"mailbox","account":"alpha@example.test","mailbox":"INBOX"}}`. Accounts are synthetic alpha@example.test and beta@example.test; each initially has a multipart attachment message and an HTML-only message. PDF attachment fixtures test byte preservation, not renderer validity. Seed/reset preserves engine independence by changing real server state rather than returning production domain objects.

Add `--hide-capability MOVE` (repeat for UIDPLUS/CONDSTORE/QRESYNC) to model absent optional extensions. Add `--server-auto-sent` to model server-managed Sent. In that mode a separate observer consumes accepted Mailpit captures, selects the owner from Mailpit’s persisted authenticated `Username` metadata, and APPENDs the exact captured bytes to that user's Dovecot Sent folder. Duplicate Message-IDs remain separate captures/copies. A failed or unknown APPEND stops the observer with visible failure; it never blindly repeats. Explicit seed/reset restarts it. This configurable test policy does not assert a live Synology version's automatic Sent behavior.

The Rust harness is `nuncio_test_support::imap::MockMailPlus`, with `start`, `start_with_sent_policy`, `control`, private synthetic `credential`, and `shutdown`. Its normal dependencies do not include the engine/proto. Run the required contract target using `python3 rebuild/scripts/verify.py --suite imap_contract`. The Python runner gives each test a private artifact subtree, retains command/exit/cleanup records, terminates its process group on timeout (exit124), and cleans only projects created in that subtree. System and actual daemon/CLI IMAP suites consume the same independently running service.

Python checks use pinned Ruff0.16.7. Download into the rebuild tool cache as setup, then run offline with both cache and tool directories scoped to the workspace:

```sh
cd rebuild
UV_TOOL_DIR="$PWD/target/tool-cache/tools" uv tool run --cache-dir target/tool-cache --from ruff==0.16.7 ruff --version
UV_TOOL_DIR="$PWD/target/tool-cache/tools" uv tool run --offline --cache-dir target/tool-cache --from ruff==0.16.7 ruff check tests/imap
UV_TOOL_DIR="$PWD/target/tool-cache/tools" uv tool run --offline --cache-dir target/tool-cache --from ruff==0.16.7 ruff format --check tests/imap
```

Capability filtering removes each hidden token and its separator together. Its independent contract asserts valid single-space capability framing; a strict production parser exposed duplicate spaces that Python imaplib had accepted. The production parser was retained unchanged. Grammar reference: [RFC3501 section9](https://datatracker.ietf.org/doc/html/rfc3501#section-9).


The independent native MOVE fault contract verifies a rejected missing-target request leaves an after-accept fault pending. A later successful MOVE loses its final acknowledgement while preserving the earlier COPYUID mapping, exact destination bytes and an unrelated Deleted source message. Command counts are two requests/one acceptance. This grounds production transfer recovery in real Dovecot behavior; the Nuncio archive/transfer dispatcher has separate system and subprocess recovery evidence. The latest independent suite contains18 Python tests, wrapped by two Rust contract tests.

The AutoSent observer uses the pinned Mailpit [message storage implementation](https://github.com/axllent/mailpit/blob/v1.31.1/internal/storage/messages.go) and [API structures](https://github.com/axllent/mailpit/blob/v1.31.1/internal/storage/structs.go). Username is committed with message metadata. Tags are applied afterward and can come from message headers, so they are unsuitable as account ownership evidence. The independent contract includes misleading tags for both users while checking the actual authenticated owner’s Sent mailbox.

On AutoSent failure, its private runtime retains `auto-sent-error.json` containing only an exception type and up to eight source locations, without exception text, locals, source lines, or message content. The observer stops on an unknown APPEND outcome; diagnostics do not permit replay. Explicit seed/reset clears that fixture error.
