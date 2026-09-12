# API publication and semantic versioning

**Status:** Proposed implementation plan; publication is not authorized.

**Date:** September 12, 2026.

**Scope:** The independent `rebuild/` workspace, engine and CLI first. Native clients remain later work.

## Recommendation

Keep the authenticated `nuncio.v2` gRPC API. Distribute an independently versioned, platform-independent contract archive containing protobuf sources, the compiled descriptor, generated HTML reference, usage documentation, examples, provenance and checksums. Use local `grpcurl` and optionally `grpcui` for exploration without enabling server reflection. Add Buf compatibility checking to the existing descriptor freeze. Start with the existing repository and artifact infrastructure; a schema registry and an HTTP gateway are unnecessary for the first delivery.

This publishes the contract developers consume. It does not publish a running daemon, expose user data, or change the numeric-loopback-only connection policy.

## Current evidence and gaps

The following paths are relative to `rebuild/`:

| Existing component | What it establishes | Remaining publication work |
|---|---|---|
| `crates/nuncio-proto/proto/nuncio/v2/*.proto` | Six service files; current account extension brings the source to 49 RPCs | Version the distributable contract independently and document methods/fields for external consumers |
| `crates/nuncio-proto/build.rs`, `src/lib.rs` | Locked tonic/prost generation and embedded `DESCRIPTOR`; build deliberately omits source comments from the descriptor | Generate reference docs from source; retain the current descriptor representation for runtime identity |
| `crates/nuncio-proto/tests/contract.rs` | Current descriptor equals reviewed freeze; separate pre-account-management descriptor checks old fields and methods survive | General compatibility gate against immutable distributed baselines, plus semantic review |
| `scripts/package.py` | Fresh application build must match freeze; archive already includes `api/`, docs, hashes and build provenance | Standalone API archive and explicit API version/hash in application metadata |
| `clients/smoke/` | Separate generated System client, with no engine dependency in normal/build dependencies | Generate from an extracted API archive; test an older client against the current daemon |
| `System.GetStatus` | `version` identifies the application; `api_version` currently returns literal `nuncio.v2` | Additive contract-version discovery; preserve existing field meaning |
| `crates/nuncio-proto/src/client.rs`, daemon auth interceptor | Numeric HTTP loopback endpoint and profile bearer authorization; rebuild default port is 9421 | Document the same rules in consumer examples |

The workspace package version is currently `0.1.0`; account-management storage is schema 23. Neither is an API semantic version. The frozen descriptor is a local reviewed baseline, not evidence that a public API release was made. Account-management verification and fresh application artifacts are owned by the active implementation; this plan makes no new gate or release claim.

## Version policy

Introduce `API-VERSION` as the single source of the contract artifact's SemVer. Recommend `2.0.0-alpha.1` for the first newly named bundle after its tests pass. Starting its major at 2 keeps the existing `nuncio.v2` namespace intelligible; it does not claim a previously released `1.x` contract or a stable product.

| Identifier | Proposed policy |
|---|---|
| API artifact | `2.0.0-alpha.1`, later `2.0.0-alpha.2`, then `2.0.0-rc.1`, and only after explicit readiness approval `2.0.0` |
| Protobuf package | Keep `nuncio.v2` for compatible evolution; an incompatible contract needs a new `nuncio.v3` package and API major 3, with explicit coexistence/migration policy |
| Application | Keep its own Cargo/application SemVer and `v…` release identity; record the exact API artifact version and descriptor hash it implements |
| Database schema | Monotonic migration number, currently 23; no relation to API minor/major increments |
| Generated SDK | Separately versioned language package, recording contract version, generator and runtime versions; SDK source compatibility can change independently |
| CLI JSON/action files | Keep their existing separately documented schema versions |

For stable API releases, incompatible public behavior increments major, compatible capabilities and deprecations increment minor, and compatible corrections increment patch. Never overwrite an artifact/version. Prerelease versions identify an unstable candidate; build metadata does not establish version precedence. These are the SemVer rules, not a promise that a prerelease has passed live acceptance. [Semantic Versioning 2.0.0](https://semver.org/).

Nuncio should apply a stricter local policy during alpha: preserve already distributed `nuncio.v2` clients through additive changes, retain all candidate descriptors, and review any proposed break explicitly. The seven current account RPCs are additive; they do not require `nuncio.v3`. If they land before the first named bundle, include them in alpha.1; if alpha.1 already exists, issue alpha.2. After stable 2.0.0, the equivalent addition would be 2.1.0.

Do not change `GetStatusResponse.api_version` from `nuncio.v2` to a SemVer string: existing clients assert that meaning. A later additive change should expose `api_contract_version` and `api_descriptor_sha256` on unused field numbers. Build both from `API-VERSION` and the embedded canonical descriptor. Clients require the package major and capabilities they use; they must not demand equality of every application patch or descriptor hash. An older daemon with no new discovery fields remains recognizable, but a client needing newer methods must report that requirement or handle `UNIMPLEMENTED` honestly.

## What to ship

Proposed archive layout; the API archive is independent of CPU/OS:

```text
nuncio-api-2.0.0-alpha.1/
  API-VERSION
  API-METADATA.json
  CHANGELOG.md
  LICENSE
  proto/nuncio/v2/{accounts,calendar,mail,maintenance,operations,system}.proto
  descriptor/nuncio.v2.binpb
  docs/index.html
  docs/API.md
  examples/README.md
  SHA256.json
```

`API-METADATA.json` records contract version, protobuf package, full source commit, clean/dirty state, source hashes, canonical descriptor hash, baseline hash/version, exact tool versions/checksums and verification receipt identifiers. `SHA256.json` covers every file except itself; supply a separate archive SHA-256 sidecar. Reuse packaging's deterministic tar/gzip normalization and refusal to replace existing output. Published candidates must come from the verified clean commit; local exploratory output must disclose dirty input and cannot be relabeled as released.

Include the current canonical descriptor only as the consumer's default. Preserve historical descriptors in source/release evidence for compatibility checks, not as competing current contracts. The current six files have no external well-known-type imports; future imports must be included transitively in the descriptor and sources needed for generation. Descriptor bytes are a useful identity only with a fixed generation/normalization recipe.

Generate HTML from `.proto` comments using a pinned local `protoc-gen-doc` executable. It supports HTML and Markdown output from protobuf declarations and comments. Because the runtime build skips source information, generating docs solely from its descriptor would lose explanatory comments. Keep the HTML self-contained; verify no remote fonts/scripts/styles are required. [protoc-gen-doc documentation](https://github.com/pseudomuto/protoc-gen-doc).

Generated fields alone do not explain durable mutation semantics. Include the existing API guide and examples covering auth, deadlines, byte streams, scoped request UUIDs, expected versions, queued versus completed operations, uncertain effects, account lifecycle, pagination and change replay. Give examples synthetic IDs only; include no profile paths, account content, credentials or OAuth registration.

## Local build and compatibility commands

The commands below specify the proposed implementation; they were not executed as a publication pipeline during this documentation task. `buf`, `grpcurl`, `grpcui` and `protoc-gen-doc` were not found on the current PATH. Do not install them or silently download floating versions in a build. Record reviewed versions/checksums in the tool lock before implementing the new pipeline, and use already provisioned tools in offline CI.

Existing checks, run from `rebuild/`:

```sh
cargo test --locked --offline -p nuncio-proto --test contract
python3 scripts/check_client.py
```

Add `buf.yaml` at `rebuild/`:

```yaml
version: v2
modules:
  - path: crates/nuncio-proto/proto
breaking:
  use:
    - FILE
```

Keep this minimal configuration focused on compatibility; adopting a broad new naming/style lint is separate work. `FILE` covers source/file compatibility beyond wire encoding. Buf can compare against local protobuf descriptor sets, so this requires neither a registry nor network access. [Buf breaking checks](https://buf.build/docs/breaking/usage/), [rules](https://buf.build/docs/breaking/rules/), [descriptor inputs](https://buf.build/docs/reference/inputs/).

```sh
buf breaking --against crates/nuncio-proto/proto/nuncio.v2.pre-account-management.bin
```

Initially keep that checked-in baseline. After an authorized distribution, compare against its immutable descriptor as well; retain the earliest supported baseline and the most recent distributed one. The proposed `scripts/package_api.py` should resolve their exact local paths/hashes from release metadata, never fetch or replace them implicitly.

Proposed wrapper interface:

```sh
python3 scripts/package_api.py --check
python3 scripts/package_api.py --output dist/api-candidate
```

`--check` must validate SemVer and changelog, check old baseline compatibility and current freeze, verify pinned generators, build docs, and test the extracted contract. Packaging writes new local output only. Its underlying documentation command, with both variables pointing to reviewed pinned executables and the output directory already created, is:

```sh
"${NUNCIO_PROTOC}" \
  --proto_path=crates/nuncio-proto/proto \
  --plugin=protoc-gen-doc="${NUNCIO_PROTOC_GEN_DOC}" \
  --doc_out=dist/api-candidate/docs \
  --doc_opt=html,index.html \
  crates/nuncio-proto/proto/nuncio/v2/*.proto
```

Obtain the canonical descriptor from the same locked tonic/prost build and compare it with the reviewed freeze before copying it as `descriptor/nuncio.v2.binpb`. Do not substitute a differently configured `protoc` output and assume byte equality. If a separate docs descriptor with source information is useful later, label and hash it separately.

### Required checks

1. Keep exact generated-versus-current-freeze equality. Additive changes should fail this until deliberately reviewed; updating a golden is not a compatibility test.
2. Run Buf against immutable distributed baselines. Reject field-number reuse, removed/renamed methods, changed request/response or stream shapes, unsafe type/cardinality/oneof changes and relevant generated-source changes. Reserve retired numbers/names; reservation alone does not make removing supported functionality compatible. [Protobuf evolution guidance](https://protobuf.dev/programming-guides/proto3/#updating).
3. Enforce the version decision: identical artifacts cannot be republished under changed bytes; changed public contract needs a version/changelog entry; stable additive/deprecation changes need a minor bump; breaks need major/package review. Descriptor diff cannot classify behavioral changes, so the review explicitly covers auth, defaults, errors, ordering, pagination, idempotency, notification effects and retry semantics.
4. Add negative gate fixtures: deleted RPC, changed field type/number, stream-mode change and missing version bump must fail. A compatible optional field/new RPC with the correct version must pass.
5. Generate the independent client from the extracted archive in a fresh directory, not from checkout-relative sources. Keep a client generated from the previous supported contract and run it against the current daemon. Test current-client/old-daemon missing-capability handling where supported. Preserve bad-auth and streaming checks, using only synthetic local providers.
6. Verify archive hashes, provenance, no secrets, readable HTML/links, and all six services/current RPC inventory. Derive the inventory from descriptors instead of hardcoding 49 forever. Run twice with identical inputs to check deterministic output.

No descriptor tool proves all language SDK compatibility. For example, adding protobuf fields can break downstream Rust exhaustive struct literals after regeneration, even when old binary clients remain compatible. Test the supported SDK API patterns, document unknown-enum handling, and version an SDK independently when its public source API breaks. Pin generators/runtime dependencies; do not equate a compatible wire addition with a guarantee that all regenerated client source still compiles.

## Client generation and interactive exploration

Initially distribute the language-neutral bundle and working Rust client recipe. The existing tonic/prost setup is appropriate; it generates service clients from protobuf. A polished Rust SDK can later package generated types plus the small loopback/auth helper, without engine or storage. Do not publish the current workspace crate automatically: review its crate contents, independent version and production dependencies first. [tonic-prost-build](https://docs.rs/tonic-prost-build/latest/tonic_prost_build/).

For later native apps, generate Swift with the official gRPC Swift 2 toolchain and C# with `Grpc.Tools`/`Grpc.Net.Client`; pin those toolchains in their consuming repositories. Generated bindings are not a finished ergonomic SDK: auth, deadlines, change replay and operation tracking still need thin client helpers. Swift/C# runtime acceptance is future work, not established by the Rust smoke. [gRPC Swift 2](https://github.com/grpc/grpc-swift-2), [.NET client generation](https://learn.microsoft.com/en-us/aspnet/core/grpc/client?view=aspnetcore-10.0).

Schema exploration is available without a daemon or credentials:

```sh
grpcurl -protoset descriptor/nuncio.v2.binpb list
grpcurl -protoset descriptor/nuncio.v2.binpb describe nuncio.v2.Accounts
```

`grpcurl` consumes descriptors and can invoke all gRPC stream shapes; it is useful for bounded command-line diagnostics. The Nuncio CLI remains the supported profile/credential-aware reference client. [grpcurl](https://github.com/fullstorydev/grpcurl).

For a Swagger-like form, use optional `grpcui` as a short-lived local developer process. It accepts a descriptor and request metadata, avoiding reflection. Its streaming UI buffers requests/results and is unsuitable as the main viewer for the unbounded `WatchChanges` stream. [grpcui](https://github.com/fullstorydev/grpcui).

Example from an extracted bundle, against an already authorized synthetic daemon. The launcher supplies a process-scoped `NUNCIO_API_AUTHORIZATION` value containing the full authorization metadata; no token belongs in shell arguments, command history, examples or saved browser history:

```sh
grpcui -plaintext -use-reflection=false \
  -bind 127.0.0.1 -port 0 -open-browser=false \
  -protoset descriptor/nuncio.v2.binpb \
  -expand-headers -rpc-header 'authorization: ${NUNCIO_API_AUTHORIZATION}' \
  -method nuncio.v2.System.GetStatus \
  127.0.0.1:9421
```

The literal variable syntax is expanded by the tool, and RPC headers are hidden from its form. Those flags are documented in the project's CLI source. Use the read-only method restriction for the initial smoke. A broader explorer can invoke real mutations; running it against real accounts is a separate user action. A secure production keystore launcher is not implemented by this plan. [grpcui CLI options](https://raw.githubusercontent.com/fullstorydev/grpcui/master/cmd/grpcui/grpcui.go).

## Swagger/OpenAPI alternatives

| Option | Fit and cost | Decision |
|---|---|---|
| Protobuf bundle + static docs + optional grpcui | Reuses current contract, supports gRPC, no added daemon endpoint; two small build-time tools for checking/docs | Recommended |
| Rust `utoipa` + Swagger UI | Good for documenting real Rust HTTP routes, including Axum integrations. It does not turn tonic protobuf services into an HTTP JSON API | Use only if a real HTTP adapter becomes a requirement |
| gRPC-Gateway + generated OpenAPI | Can add HTTP mappings to protobuf and proxy HTTP clients to gRPC; adds gateway runtime and translation/auth/streaming behavior to test | Defer; do not add solely to get documentation |
| Buf Schema Registry/hosted SDK generation | Convenient hosted schema/distribution workflows; adds another publication target and dependency | Revisit when multiple external client teams need it |

`utoipa` is maintained Rust OpenAPI tooling and directly answers the Rust-tooling question. Its documented scope is REST/OpenAPI; Nuncio's existing Axum dependency alone does not create public REST routes. [utoipa](https://docs.rs/utoipa/latest/utoipa/). Gateway HTTP mappings need explicit annotations or configuration and a real adapter; an OpenAPI document by itself would advertise endpoints Nuncio does not implement. [gRPC-Gateway annotations](https://grpc-ecosystem.github.io/grpc-gateway/docs/tutorials/adding_annotations/).

## Implementation sequence and authorization boundary

1. Add `API-VERSION`, `API-CHANGELOG.md`, minimal `buf.yaml` and a reviewed generator/tool lock. Preserve both existing descriptors. Add compatibility/version fixtures and the local `scripts/package_api.py` wrapper, reusing existing packaging primitives.
2. Produce the local API archive and generated docs; extend the independent client to consume the extracted archive and retained baseline. Add the API version/hash to application build metadata and, as a separately reviewed additive change, status discovery.
3. Integrate local artifact verification with the existing rebuild CI. Coordinate with the testing-installer worker's application artifact upload rather than adding a competing application workflow. An approved CI artifact is a testing distribution with retention limits, not a permanent public release.
4. After checks pass, present the exact archive hash, full commit, version, compatibility results, destinations and visibility for publication authorization. Proposed first permanent destination: an immutable GitHub API release/tag such as `api-v2.0.0-alpha.1` with the archive and checksum. This namespace is a proposed exception to root application-only `v…` tag conventions. Publish versioned static docs at the approved hosting destination only if requested. Leave BSR, crates.io, NuGet and Swift package releases for an explicit subsequent decision.

This task creates this reviewable plan only. It does not install tools, change protobuf/runtime/workflows, regenerate the freeze, build a new distribution, tag, upload, release or expose any service. Those remaining implementation steps are listed precisely so the owner can execute them under the appropriate scope. Remote publication, registry namespaces, releases/tags, public documentation hosting and changes to visibility still require authorization; the approval must identify the concrete verified artifacts and destination.
