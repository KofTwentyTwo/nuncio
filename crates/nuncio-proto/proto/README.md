# `nuncio.v1` protobuf contract

This directory holds the versioned gRPC/protobuf contract for the `nunciod`
daemon: `nuncio/v1/nuncio.proto`, defining the `System`, `Accounts`, `Mail`,
`Filters`, `Export`, and `Audit` services.

The Rust crate `nuncio-proto` (one level up) compiles this `.proto` into the
in-repo Rust client/server stubs used by `nunciod`, `nuncio-cli`, and the
other in-repo presentation shells. This README is for anyone generating a
client in a **different** language against the same contract -- e.g. a
native Swift/macOS app, a C#/WinUI app, or a TypeScript/Node MCP bridge.

`descriptor.bin` alongside the `.proto` file is a compiled
`FileDescriptorSet` for the whole `nuncio.v1` package, committed as the
published, canonical snapshot of the wire contract. `nuncio-proto`'s own test
suite fails if a `.proto` edit changes what that descriptor compiles to
without the committed copy being deliberately updated to match -- see
`descriptor_matches_committed_golden` in `nuncio-proto/src/lib.rs`. Anyone
generating a client against this contract can diff their own compiled
descriptor against `descriptor.bin` to check they are codegen-ing against
the exact same wire shape this repo tested against.

## What you need before generating anything

- The daemon binds to a **loopback-only** address, `127.0.0.1:9420` by
  default (overridable via the `NUNCIO_GRPC_ADDR` environment variable, as
  `host:port`). It is plain HTTP/2 (`h2c`), never TLS-wrapped -- loopback
  traffic doesn't leave the host, so authentication is via bearer token
  instead of transport encryption.
- Every RPC on every service requires an `authorization: Bearer <token>`
  gRPC metadata header. The token is 32 random bytes, hex-encoded, minted
  once and persisted in the OS-native credential vault (Keychain / Credential
  Manager / Secret Service) under the daemon's `grpc-bearer-token` vault
  entry -- it is never logged and never sent in cleartext anywhere but this
  header. A client generated from this contract in any language still needs
  a way to read that same vault entry (or otherwise obtain the token from a
  process that can) before it can call any RPC; there is no unauthenticated
  RPC anywhere in this contract, including `GetStatus`.
- `System.Subscribe` is the one server-streaming RPC in the contract: it
  opens a long-lived stream of `Event` messages (sync progress, filter
  matches, database recovery, update availability, shutdown, etc.) that the
  daemon pushes for the lifetime of the call. Generated clients should treat
  it as a live push feed, not a request/response call -- keep reading from
  the stream until the daemon closes it or the connection drops, rather than
  expecting a single reply.

## Generating a client

All three toolchains below take the same two inputs: `nuncio.proto` in this
directory, and a `protoc` binary (or an equivalent pure-language compiler).
There are no `.proto` imports outside this file to worry about.

### Swift (grpc-swift)

```bash
# Run from this directory (crates/nuncio-proto/proto). protoc-gen-grpc-swift
# and protoc-gen-swift come from grpc-swift/grpc-swift-protobuf.
protoc \
  --swift_out=./Generated \
  --grpc-swift_out=./Generated \
  --proto_path=. \
  nuncio/v1/nuncio.proto
```

See the [grpc-swift](https://github.com/grpc/grpc-swift) plugin
documentation for the exact flags matching the grpc-swift version and Swift
Package Manager setup in use; the `Client` variant it emits pairs with
`GRPCChannelPool`/`ClientTransport` for connecting to `127.0.0.1:9420` and
attaching the bearer token as a call header.

### C# (Grpc.Tools)

Reference the `Grpc.Tools` and `Grpc.Net.Client` NuGet packages, then add
this to the consuming `.csproj` (paths relative to wherever `nuncio.proto`
was copied into the C# project, or referenced directly from this repo via a
relative `Link`):

```xml
<ItemGroup>
  <Protobuf Include="nuncio/v1/nuncio.proto" GrpcServices="Client" ProtoRoot="nuncio/v1" />
</ItemGroup>
```

`dotnet build` then generates the client stubs automatically via the MSBuild
integration `Grpc.Tools` provides; no manual `protoc` invocation is needed.
Attach the bearer token per call via `Metadata` on the `CallOptions`, or via
a `CallCredentials`/`CallInvoker` wrapper if every call should carry it
uniformly.

### TypeScript (ts-proto or connect-es)

Either generator works from the same `.proto`; pick based on whether a
`grpc-js`-style client (`ts-proto`) or a Connect-protocol client
(`connect-es`, which also speaks plain gRPC) fits the consuming project
better.

```bash
# Run from this directory (crates/nuncio-proto/proto).

# ts-proto (emits classic grpc-js-style client + message types)
protoc \
  --plugin=protoc-gen-ts_proto=./node_modules/.bin/protoc-gen-ts_proto \
  --ts_proto_out=./generated \
  --ts_proto_opt=outputServices=grpc-js \
  --proto_path=. \
  nuncio/v1/nuncio.proto

# connect-es (via buf generate; requires a buf.gen.yaml pinning
# @bufbuild/protoc-gen-es and @connectrpc/protoc-gen-connect-es, with
# this directory as buf's input root)
buf generate --path nuncio/v1/nuncio.proto
```

This is the natural fit for an `nuncio-mcp` TypeScript bridge: generate the
client once, dial `127.0.0.1:9420`, attach the bearer token as an
`authorization` header on the underlying transport, and subscribe to
`System.Subscribe` for push events instead of polling.

## Versioning policy

The `nuncio.v1` package is stable. Concretely:

- **Allowed within `nuncio.v1`** (no version bump): adding a new RPC to an
  existing service; adding a new service; adding a new message; adding a new
  *optional* field to an existing message using a previously-unused field
  number; adding a new `enum` value; adding a new `oneof` case to `Event`.
  These are all wire-compatible -- old clients silently ignore fields/cases
  they don't recognize, and the daemon never *requires* a new field to
  service an old request.
- **Never done within `nuncio.v1`**: removing or renaming a field; changing
  a field's number; changing a field's type; removing or renaming an RPC or
  service; changing an RPC's request/response message; repurposing a
  `oneof` field number for a different case. Any of these breaks old
  generated clients talking to a new daemon (or vice versa).
- **If a breaking change is genuinely required**, it goes in a new
  `nuncio.v2` package (a new `.proto` file, new generated module, served
  alongside -- not instead of -- `nuncio.v1` for as long as both are
  supported), never by mutating `nuncio.v1` in place.
- The committed `descriptor.bin` is the enforcement mechanism: any edit to
  `nuncio.proto` that changes the compiled wire shape fails
  `nuncio-proto`'s test suite until `descriptor.bin` is deliberately
  regenerated and committed alongside the `.proto` change, which is exactly
  the point where "is this actually additive?" gets asked and answered.
