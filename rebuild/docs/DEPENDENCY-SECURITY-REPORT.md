# Dependency security remediation — September 12, 2026

Current hosted confirmation: [security run 34704460237](https://github.com/KofTwentyTwo/nuncio/actions/runs/34704460237)
passed all seven jobs at signed/pushed `164b021`. The corrected original complete
lock passed cargo-audit (509 packages), and rebuild/client advisory jobs passed.
Nine action findings are verified fixed. The earlier failure and local evidence
below remain historical; no PR was merged or closed. See [SECURITY-CI.md](SECURITY-CI.md)
for the actual code-scanning findings and their limits.


The read-only GitHub inventory returned **zero open Dependabot security alerts**
and **eleven open Dependabot version-update pull requests**, all targeting `main`.
Those are different queues: a zero alert count did not make the original
workspace's dependency graph clean. This report covers the original workspace,
the rebuild, and the independent API smoke client's lockfile in
`feature/nuncio-google-first-rebuild`; the original `dev` checkout was untouched.

Inventory receipts, before/after locks, commands, timestamps, compiler diagnostics,
and scan output are retained locally in
`rebuild/test-results/dependency-security/`. The advisory database was refreshed
from RustSec and checked at commit
`b50980aad8b8f14f77e25a97b32dd94bf008b0af` using cargo-deny 0.19.8.
No advisory exceptions or GitHub alert dismissals were added.

The original workspace passes **790 tests, zero failed/ignored**, formatting,
Clippy, and cargo-deny after the hosted-index follow-up below. All three complete
lockfiles initially passed local cargo-audit 0.22.2; that cached-index result did
not detect a yanked optional package subsequently identified by hosted CI. The
corrected original lockfile now passes with an explicit online index refresh,
**zero vulnerabilities and zero warnings**. The initial original lockfile,
rescanned with that same tool/database, fails with two vulnerability advisories,
one unsoundness advisory, and three maintenance advisories.

## Hosted index follow-up

Hosted [original advisory job 103577787655](https://github.com/KofTwentyTwo/nuncio/actions/runs/34702939715/job/103577787655)
at `549a5983db507670274422d6f702122b56210702` failed on yanked `chacha20 0.10.1`,
despite the preceding selected-graph cargo-deny check passing. The optional
lockfile chain is `reqwest` → `quinn` → `quinn-proto` → `rand 0.10.2` →
`chacha20 0.10.1`; it is absent from the selected graph even with `--target all`.
The fresh [crates.io index](https://index.crates.io/ch/ac/chacha20) marks 0.10.0
and 0.10.1 yanked and 0.10.2 unyanked. This is a yank finding, with no CVE/GHSA
assigned in the scan; no reason for the yank is inferred.

`cargo update --package chacha20@0.10.1 --precise 0.10.2` changes only that
version/checksum and the referring `rand` lock entry. Rebuild/client locks remain
unchanged. Formatting, strict Clippy, all **790 tests**, and cargo-deny pass again.
Cargo-audit without `--no-fetch` explicitly refreshes both RustSec and crates.io,
then passes with zero vulnerabilities/warnings. Its human-readable output confirms
the index update without cache/access warnings. Current original lock SHA-256:
`e93af28bd2ef6610cfb420e7fc0c3a1c48443f207dc80bc8d92996affffa019b`.

Receipts are under `rebuild/test-results/dependency-security/yank-followup/`:
`summary.json`, all four gate logs/receipts, verified `tests-egress.json`,
`audit-online.json`, and `audit-online-human.log`. The original successful local
receipts remain unchanged: `--no-fetch` also disables registry-index refresh in
cargo-audit 0.22.2, so those receipts establish only the cached index's state.
The hosted failure is preserved, and a fresh hosted result on the corrected
commit is still required. No yank suppression or advisory ignore was introduced.

## Advisories and affected paths

| Advisory | Affected original scope and dependency path | Remediation |
|---|---|---|
| [RUSTSEC-2026-0258 / GHSA-q83h-524g-xf6h](https://rustsec.org/advisories/RUSTSEC-2026-0258.html), empty HTTP/2 DATA frame denial of service; no CVE alias in this advisory | Runtime `reqwest`/`tonic` → `hyper` → `h2 0.4.15` | Lock `h2 0.4.19`; first fixed version is 0.4.16. Rebuild/client already used 0.4.19. |
| [RUSTSEC-2023-0071 / CVE-2023-49092](https://rustsec.org/advisories/RUSTSEC-2023-0071.html), also GHSA-c38w-74pg-36hr and GHSA-4grx-2x9w-596c | Lockfile-only optional `sqlx`/`sqlx-macros-core` → `sqlx-mysql` → `rsa 0.9.10`; MySQL was not in the compiled SQLite graph | Disable unused SQLx default features, retaining `runtime-tokio` and `sqlite`. This removes MySQL, PostgreSQL, and RSA from the original lockfile. No patched RSA version is available; removal is the fix. |
| [RUSTSEC-2026-0221](https://rustsec.org/advisories/RUSTSEC-2026-0221.html), unsound cross-thread event tags; no CVE/GHSA alias | `sqlx-core`, and `async-imap` → `async-channel` → `event-listener-strategy` → `event-listener 5.4.1` | Update the affected 5.x instance to 5.4.2. The remaining 2.5.3 instance is below the advisory's affected range. Rebuild already used 5.4.2. |
| [RUSTSEC-2024-0388](https://rustsec.org/advisories/RUSTSEC-2024-0388.html), unmaintained `derivative` | Linux credential path: `nuncio-store` → `keyring 2.3.3` → `secret-service 3.1.0` → `zbus 3.15.2` → `derivative 2.2.0` | Upgrade keyring to 4.2.0 with its explicit `v1` feature; old dependency removed. |
| [RUSTSEC-2024-0384](https://rustsec.org/advisories/RUSTSEC-2024-0384.html), unmaintained `instant` | Same keyring path → `zbus` → old async filesystem/process dependencies → `futures-lite 1.13.0` → `fastrand 1.9.0` → `instant 0.1.13` | Same keyring upgrade removes the old dependency chain. |
| [RUSTSEC-2024-0370](https://rustsec.org/advisories/RUSTSEC-2024-0370.html), unmaintained `proc-macro-error` | `nuncio-store` → `age 0.10.1` → `i18n-embed-fl 0.7.0` → `proc-macro-error 1.0.4` | Upgrade age to 0.12.1, using `i18n-embed-fl 0.10.1` and `proc-macro-error3 3.1.1`. |
| [RUSTSEC-2026-0173](https://rustsec.org/advisories/RUSTSEC-2026-0173.html), unmaintained `proc-macro-error2` | Detected in the intermediate age 0.11.5 candidate, not the initial tree: `i18n-embed-fl 0.9.4` → `proc-macro-error2 2.0.1` | The smaller age 0.11 update was rejected after its failed scan. Age 0.12.1 removes this chain too. |

The last four are maintenance advisories, not CVEs. Updating maintained packages
and removing unused vulnerable dependencies avoids inventing advisory exceptions.
The RSA finding also explains why checking a compiled dependency graph alone is
insufficient for a repository-wide lockfile review.

## Open Dependabot PR coverage

| Open PR | Requested update | Feature-branch implementation |
|---|---|---|
| [#439](https://github.com/KofTwentyTwo/nuncio/pull/439) | tokio-rustls 0.26.4 → 0.26.5 | Original updated; rebuild/client already at 0.26.5. |
| [#438](https://github.com/KofTwentyTwo/nuncio/pull/438) | keyring 2.3.3 → 4.2.0 | Original updated with explicit native `v1` backend selection; rebuild already at 4.2.0. |
| [#437](https://github.com/KofTwentyTwo/nuncio/pull/437) | flate2 1.1.9 → 1.1.10 | Original updated; rebuild already at 1.1.10. |
| [#436](https://github.com/KofTwentyTwo/nuncio/pull/436) | mail-parser 0.9.4 → 0.11.8 | Original updated to 0.11.9, also the rebuild version. |
| [#435](https://github.com/KofTwentyTwo/nuncio/pull/435) | quick-xml 0.41.0 → 0.42.0 | Original updated with the UTF-8 event API adapter. |
| [#434](https://github.com/KofTwentyTwo/nuncio/pull/434) | aes-gcm 0.10.3 → 0.11.1 | Original direct dependency updated; old-format ciphertext remains supported. Age's independent HPKE dependency still uses patched aes-gcm 0.10.3 transitively. |
| [#432](https://github.com/KofTwentyTwo/nuncio/pull/432) | futures 0.3.33 → 0.3.34 | Original updated; rebuild already at 0.3.34. |
| [#404](https://github.com/KofTwentyTwo/nuncio/pull/404) | async-trait 0.1.91 → 0.1.92 | Original updated; rebuild/client already at 0.1.92. |
| [#383](https://github.com/KofTwentyTwo/nuncio/pull/383) | clap 4.6.4 → 4.6.6 | Original updated; rebuild/client already at 4.6.6. |
| [#382](https://github.com/KofTwentyTwo/nuncio/pull/382) | lettre 0.11.22 → 0.11.23 | Original updated; rebuild already at 0.11.23. |
| [#379](https://github.com/KofTwentyTwo/nuncio/pull/379) | CodeQL action v3 → v4.37.4 | Security workflow pins init/analyze to `f205ea1c3313d32999d8d6a48b4f6530d4437b38` (v4.37.4). |

The feature work does not merge or close these PRs. Their requested changes have
been incorporated or superseded in source; GitHub disposition remains a separate
review/merge step. The root coordinator owns commit and push decisions.

## Compatibility and verification

The original adapter changes are limited to quick-xml's already-decoded UTF-8
events, keyring's `delete_credential`, the current AES-GCM/age APIs, and fallible
OS randomness. The stable keyring service/account names and AES nonce-prefix
storage format are preserved. Rule identifiers remain random 128-bit values.
No secret storage, provider, or schema redesign was introduced by these updates.

Two committed fixtures were independently generated with the old aes-gcm 0.10.3
and age 0.10.1 libraries. Tests decrypt those fixed bytes with the new libraries,
alongside existing tamper, wrong-key, and incorrect-passphrase assertions. These
fixtures contain only synthetic data; their provenance is in
[`crates/nuncio-store/tests/fixtures/README.md`](../../crates/nuncio-store/tests/fixtures/README.md).

| Check at the initial dependency checkpoint | Result | Local receipt under `rebuild/test-results/dependency-security/` |
|---|---|---|
| `cargo fmt --all -- --check` | Exit 0 | `original-fmt-final.receipt.json` |
| `cargo clippy --locked --offline --workspace --all-targets -- -D warnings` | Exit 0 | `original-clippy-final.receipt.json` |
| Complete original `cargo test --locked --offline --workspace` under verified egress denial | Exit 0; 790 passed, zero failed/ignored across 43 test/doc-test result groups | `original-tests-final.log`, `original-tests-final-egress.json`, `final-summary.json` |
| Original cargo-deny advisories | Exit 0 | `original-final.receipt.json` |
| Rebuild cargo-deny advisories/licenses/sources | Exit 0 | `rebuild-final.receipt.json` |
| Independent client cargo-deny advisories | Exit 0 | `client-final.receipt.json` |
| Complete original, rebuild, and client lockfiles, cargo-audit 0.22.2 with `--deny warnings` | Three exits 0; each zero vulnerabilities/warnings, empty ignore list, no target filtering | `{original,rebuild,client}-lock.audit.{json,receipt.json}` |

All 123 original source/fixture/manifest files captured before the final suite
retained identical hashes afterward. The original lock SHA-256 at that initial checkpoint was
`af1495a132b34c44a3a811c289e81ec6d6c9152e703bd076cca17cac9498527c`.
The earlier 790-test run preceded the SQLx/event-listener cleanup; the final run
independently verifies the final graph. Initial compiler diagnostics and the
failing pre-remediation advisory/lockfile scans are retained.

The rebuild and independent client lockfiles were not changed. Their complete
product verification is coordinated separately in [VERIFICATION.md](VERIFICATION.md).
Native keychain interoperability and hosted CI on the final source remain
separate checks; automated tests do not access real mail/calendar providers or
native secrets. Cargo-audit was built with `--locked --no-default-features` into
the ignored task-local `audit-tool/` directory; no executable was installed into
the normal environment or added to PATH.

To reproduce advisory checks from the feature worktree root:

```bash
export NUNCIO_ADVISORY_DB="$PWD/rebuild/test-results/advisory-db"
cargo deny --manifest-path Cargo.toml fetch -c rebuild/deny.toml db
cargo deny --manifest-path Cargo.toml --locked --offline check -c rebuild/deny.toml advisories
cargo deny --manifest-path rebuild/Cargo.toml --locked --offline check -c rebuild/deny.toml advisories licenses sources
cargo deny --manifest-path rebuild/clients/smoke/Cargo.toml --locked --offline check -c rebuild/deny.toml advisories
python3 rebuild/scripts/egress.py --evidence rebuild/test-results/dependency-security/original-tests-final-egress.json -- cargo test --locked --offline --workspace
```

Full-lock scans used the following command for each of `Cargo.lock`,
`rebuild/Cargo.lock`, and `rebuild/clients/smoke/Cargo.lock`:

```bash
CARGO_NET_OFFLINE=true rebuild/test-results/dependency-security/audit-tool/bin/cargo-audit audit \
  --file Cargo.lock \
  --db "$NUNCIO_ADVISORY_DB/advisory-db-3157b0e258782691" \
  --no-fetch --deny warnings --json
```

The hashed directory is the actual Git checkout created by cargo-deny in this
receipt; a different setup must resolve its own database checkout path. No
`--ignore`, `--stale`, target restriction, or disabled yanked check was used.

The egress runner independently proves loopback access and external IPv4/IPv6
denial in both its process and a child before running the tests. Dependency
fetching and public advisory refresh occur separately from provider tests.

## GitHub closure conditions

The default branch reported by GitHub is `main`. Feature-branch remediation and
passing local scans do not update its dependency graph or close its bot PRs.
Promotion requires reviewed commits, successful hosted checks, an authorized
merge to the default branch, and GitHub's subsequent dependency rescan. No
security alert has been dismissed and no bot PR has been merged or closed here.
See [SECURITY-CI.md](SECURITY-CI.md) for the new security workflow and all three
Dependabot Cargo directory entries; hosted results must be recorded separately.
