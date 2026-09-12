# Independent generated-client smoke

This small consumer has its own Cargo workspace and lockfile. Its build generates
a client directly from the published `nuncio.v2` System protobuf; it imports no
engine, store, CLI, daemon, or Nuncio protocol-client implementation. It reads
authenticated status and one replayed change through the real daemon's API.
The daemon must already have a committed change. Native apps are outside scope.

Build with `cargo build --locked --manifest-path rebuild/clients/smoke/Cargo.toml
--target-dir rebuild/target/external-client`. The binary takes one numeric loopback
HTTP endpoint argument and the complete authorization header value on stdin, then
EOF. Never put a real token in an argument, shell history, source or log. The
example has a15-second deadline and reports generic failure without token details.

Its subprocess integration test uses the existing independent mock and actual
test daemon/CLI selected by `NUNCIO_E2E_DAEMON` and `NUNCIO_E2E_CLI`. Credentials
are read from that synthetic profile and piped to the child. The test verifies
status/change identity, invalid authorization rejection, no credential output and
no provider sends/notifications. The mock is a dev-dependency only. Production
package inclusion and the main verification-runner integration remain pending.
