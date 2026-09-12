# Dependency review

Current hosted confirmation: [security run 34704460237](https://github.com/KofTwentyTwo/nuncio/actions/runs/34704460237)
passed all seven jobs at signed/pushed `164b021`. The corrected original complete
lock passed cargo-audit (509 packages), and rebuild/client advisory jobs passed.
Nine action findings are verified fixed. The earlier failure and local evidence
below remain historical; no PR was merged or closed. See [SECURITY-CI.md](SECURITY-CI.md)
for the actual code-scanning findings and their limits.


## September 12 repository-wide follow-up

[DEPENDENCY-SECURITY-REPORT.md](DEPENDENCY-SECURITY-REPORT.md) records the current
GitHub inventory, all eleven open dependency-update PRs, and remediation across
the original workspace, rebuild, and independent client. GitHub reported zero
open Dependabot security alerts, but the original workspace still required h2,
keyring, age, event-listener, and optional SQLx/RSA remediation. These original
workspace changes do not alter the rebuild/client lockfiles. Their fresh
advisory checks remain clean. Hosted CI subsequently found yanked optional
`chacha20 0.10.1` in the original lockfile, missed by the initial cached-index
local audit. A minimal update to 0.10.2 now passes cargo-audit 0.22.2 with an
explicit online index refresh and zero vulnerabilities/warnings; the original
workspace again passes 790 tests plus formatting/Clippy. The original hosted
failure remains recorded and its corrected hosted rerun is pending. Receipts and GitHub closure conditions are
tracked in that report. Existing package notices below describe the earlier
rebuild artifact, not a rebuilt original-workspace distribution.

## Earlier rebuild checkpoint

Dependency review and all237 packaged third-party notices passed after remediation. Hosted platform checks and native-keystore acceptance remain separate pending conditions. The first cargo-deny0.19.8 scan of the locked
workspace completed on September12,2026UTC with exit5. Its current RustSec database
reported three unmaintained packages: derivative2.2.0 (RUSTSEC-2024-0388) and
instant0.1.13 (RUSTSEC-2024-0384), through keyring2/secret-service3/zbus3; and the
direct rustls-pemfile2.2.0 dependency (RUSTSEC-2025-0134). No vulnerability advisory
was reported in that run. This does not make the failed audit clean or prove the
absence of vulnerabilities. Raw diagnostics and database checkout are retained in
`test-results/task15-dependencies/`; remediation and subsequent passing checks are recorded below.

The initial license policy also rejected0BSD (mailparse/quoted_printable, test
dependencies) andCDLA-Permissive-2.0 (webpki-roots trust data). The reviewed policy
in [deny.toml](../deny.toml) permits those licenses and retains all advisory errors.
[0BSD](https://spdx.org/licenses/0BSD.html) is a permissive software license.
[CDLA-Permissive-2.0](https://cdla.dev/permissive-2-0/) requires distributing its
agreement with redistributed data; packaging must include the trust-data license.
The verified archive includes the trust-data and native vendored-library notices; see the inventory evidence below.

Run `cargo deny --locked check advisories licenses sources` from rebuild/. Fetch
the public advisory database before enabling test egress denial; offline checking
must retain a database no more than seven days old. `NUNCIO_ADVISORY_DB` optionally
selects a local database directory. This tool variable is not a provider credential.
No advisories are ignored. The file describes a build/review policy, not a change
to the original project's license or a release approval.

## Remediation and current check

The locked build now uses keyring4.2.0 with its explicit v1 feature: native macOS
Keychain/Windows Credential Manager and Unix Secret Service, as documented in the
[upstream API](https://docs.rs/keyring/4.2.0/keyring/v1/index.html). Service/user
identifiers and hex-encoded secret storage remain unchanged; deletion uses the
current delete_credential method. No real OS keychain was read or changed by tests.
The existing rustls pki_types PEM parser replaces rustls-pemfile, retaining empty/
malformed certificate refusal and certificate-chain/hostname validation.

Targeted lock update14944/0 and fetch0 removed derivative, instant and
rustls-pemfile. Fresh offline cargo-deny advisories/licenses/sources check0 reports
zero errors/warnings (352 license helps). Raw evidence: task15-dependencies/
review-after.jsonl; the original failed review remains retained. This is a scan
against the dated advisory database, not proof of zero vulnerabilities. MacOS
compilation/regressions, Linux runtime checks and packaged notices are separate.

## Archive notices

The clean package build initially failed62175/1 when published crates lacked
license files. The packager now includes pinned upstream notices with SHA256
provenance from each crate's .cargo_vcs_info.json revision. Stalwart notices live
in LICENSES subdirectories; other monorepo crates omit root notices from published
packages. stop-token0.7.0 has no separate license file in its pinned tree: the
archive includes its original dual-license Cargo declaration and the standard
Apache-2.0 license for that declared option. The inventory does not treat README
examples as substitute license texts. Unknown missing notices still fail packaging.

Source notices: licenses/upstream/index.json. Offline inventory check passed0 for
all237 compiled third-party packages, including SQLCipher, vendored OpenSSL,
BoringSSL/ring and webpki trust-data notices. Clean package rerun80508/0 built and
exercised extracted production binaries. Exact archive and checks are recorded in
VERIFICATION; no installation or release was performed.
