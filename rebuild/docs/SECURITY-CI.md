# Security scanning in development

September 12, 2026. Workflow changes below are implemented locally; their first hosted run at the new checkpoint is still pending. This document distinguishes scan coverage from required merge checks and from provider acceptance.

## Development means `dev`

Read-only GitHub API inspection confirmed that `main` is the default branch and `dev` and `rc` exist. There is no `develop` branch. The security workflow now covers pushes to `main`, `dev`, `develop`, `rc`, and the active `feature/nuncio-google-first-rebuild` branch. Including `develop` also preserves the requested coverage if that name is introduced later; no branch was created or renamed.

Every `pull_request` event is eligible, regardless of target branch or changed paths. CodeQL is no longer restricted to PRs targeting `main`. Normal PR events preserve GitHub's reduced permissions for forks and Dependabot. There is no privileged `pull_request_target` checkout.

The weekly Monday 01:30 UTC schedule remains, and manual dispatch is available. GitHub scheduled workflows run from the default branch; this is not a claim that the weekly job scans an idle `dev` branch. Development changes are scanned by push/PR, and maintainers can manually run the workflow at `dev` when a fresh scan is needed. The changed workflow must reach each branch before that branch's push uses it; the schedule and Dependabot configuration must reach the default branch before their default-branch behavior changes. [GitHub workflow events](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows).

## Coverage and execution

| Layer | Current implementation | Limits |
|---|---|---|
| Source security | Existing `.github/workflows/codeql.yml`, now Rust, Python, JavaScript/TypeScript and GitHub Actions, all with `security-extended` | Static findings are not proof of secure behavior or successful live integration |
| Vulnerable Cargo dependencies | Same workflow, failing advisory jobs for original, rebuild and independent client workspaces: cargo-deny checks selected graphs; cargo-audit checks every package in each complete lockfile | RustSec knowledge is time-dependent; successful scans apply to their recorded lockfiles/database |
| Broader rebuild dependency policy | Existing rebuild release-check job runs cargo-deny advisories/licenses/sources using `rebuild/deny.toml` | Remains part of rebuild CI, not replaced by advisory-only jobs |
| Secret detection | GitHub native secret scanning and push protection were already enabled and remain enabled | Provider/non-provider pattern support differs; not a guarantee that every arbitrary password will be recognized |
| Update proposals | Dependabot Cargo directories now include original, rebuild and independent client workspaces; existing Actions update configuration remains | PR creation is not a security gate or automatic merge; default-branch configuration applies |
| Functional/security invariants | Existing offline rebuild security/system/subprocess tests and original quality gates remain | Synthetic local providers do not establish named live-provider or native-keystore acceptance |

CodeQL Rust supports `build-mode: none` and extracts Rust source across the repository; the scope is not restricted to the original workspace or the rebuild. This mode still uses rust-analyzer to execute build scripts and compile procedural macros, even though it does not perform a full application build. No provider tests or production profiles are invoked by this workflow. The normal `pull_request` event and absence of credentials are important when analyzing untrusted changes. The pinned Rust toolchain is prepared explicitly. [CodeQL Rust build behavior](https://docs.github.com/en/code-security/reference/code-scanning/codeql/build-options-for-compiled-languages#building-rust).

The extended suite includes the default queries and additional security queries. It is configured for all four languages; JavaScript coverage is retained for the archived/reference sources and Actions coverage checks workflow security. [CodeQL query suites](https://codeql.github.com/codeql-query-help/).

The CodeQL init/analyze actions are pinned to `f205ea1c3313d32999d8d6a48b4f6530d4437b38`, the commit behind the verified upstream annotated tag `v4.37.4`. This also incorporates the open CodeQL dependency update. Checkout is pinned to the repository's existing v7.0.1 commit, with persisted Git credentials disabled. Workflow-wide permissions are `contents: read`; only the analysis job adds `actions: read` and `security-events: write` for result publication. Advisory jobs have no write permission or provider credentials. Neither job uses `continue-on-error`.

CodeQL result uploads for fork/Dependabot PRs use the supported PR event behavior rather than attempting to grant untrusted pushes write permission. [GitHub's CodeQL permission guidance](https://docs.github.com/en/code-security/reference/code-scanning/troubleshoot-analysis-errors/resource-not-accessible).

Advisory jobs provision the pinned `cargo-deny 0.19.8` and `cargo-audit 0.22.2`, fetch the locked dependency graph and RustSec database, and then check with Cargo offline/locked. `NUNCIO_ADVISORY_DB` selects the same absolute rebuild evidence path for fetch and check. The existing cargo-deny policy denies yanked dependencies, reports unmaintained dependencies, and limits database age to seven days; vulnerabilities fail the job. Scanners need network access to refresh tools/advisories and publish CodeQL results. That is separate from the provider-test egress-denial policy. [cargo-deny advisories](https://embarkstudios.github.io/cargo-deny/checks/advisories/index.html).

The full-lock check closes a demonstrated gap: the original compiled SQLite dependency graph passed while its lockfile still contained optional `sqlx-mysql` → `rsa`, affected by CVE-2023-49092. Each existing advisory job now passes its explicit `Cargo.lock` to cargo-audit, which checks all locked packages regardless of selected features. The tool is installed with `--locked --no-default-features`; binary scanning is unnecessary here. It reuses cargo-deny's fetched RustSec Git checkout, validates that exactly one exists, and runs `--no-fetch --deny warnings` with `CARGO_NET_OFFLINE=true`. No advisory ignores, severity/target filters, stale-database allowance, or yanked-check bypass are used. Vulnerabilities and informational warnings fail the job. [Pinned cargo-audit release](https://docs.rs/crate/cargo-audit/0.22.2), [official command behavior](https://github.com/rustsec/rustsec/blob/cargo-audit/v0.22.2/cargo-audit/src/commands/audit.rs).

The original quality workflow additionally accepts `develop` pushes/PRs. Rebuild CI additionally accepts `main`, `develop` and `rc` pushes while retaining `dev`, the active feature branch, existing path filters, all PR targets, and the installer worker's exact Apple Silicon artifact-upload block. No new workflow was introduced.

## Secrets and repository settings

The repository metadata reported:

- Public visibility; Dependabot security updates enabled.
- Secret scanning enabled; secret-scanning push protection enabled.
- Non-provider secret patterns disabled; validity checks disabled.
- CodeQL default setup `not-configured`; the explicit Actions workflow owns scanning. Its listed possible languages are not evidence of additional executed scans.

GitHub secret scanning examines Git history across all branches, so existing native coverage already includes development branches. A second hosted secrets workflow was not added merely to duplicate that service. The local Gitleaks check below provides additional current-source evidence. [GitHub secret scanning scope](https://docs.github.com/en/code-security/concepts/secret-security/secret-scanning).

No settings were changed. Enabling non-provider patterns or validity checks would require a separate reviewed settings change. No alert was dismissed, no ignore was added, and no credential was printed or rotated during this task. The changed Dependabot `directories` syntax is supported; it retains the existing daily Cargo and weekly Actions cadence. [Dependabot directory configuration](https://docs.github.com/en/code-security/reference/supply-chain-security/dependabot-options-reference#directories-or-directory).

## Verification receipts

Local evidence is under `rebuild/test-results/security-ci/` in the worktree; it is not a release artifact or a replacement for hosted scan results.

| Check | Actual result |
|---|---|
| Parse the four edited YAML files with PyYAML BaseLoader | Exit 0; workflow `on` keys and mappings parsed correctly |
| Structural workflow assertions | Exit 0; required branches, all-target PR events, no path exclusion in security workflow, four languages, none-mode, pinned action SHAs, restricted permissions, three existing manifest/lockfile pairs, and unchanged installer artifact identity verified |
| `git diff --check -- .github/workflows/codeql.yml .github/workflows/ci.yml .github/workflows/rebuild-ci.yml .github/dependabot.yml` | Exit 0 |
| Local Gitleaks 8.30.1 current-source snapshot | Scanner exit 1; 683 files, about 7.37 MB, exactly one redacted finding; classification below |
| Client advisory command before specifying the prepared database | Scanner exit 1 because cwd-relative advisory database was absent; retained as `client-advisories.json`/`.log` |
| Same command with `NUNCIO_ADVISORY_DB` selecting prepared `rebuild/test-results/advisory-db` | Scanner exit 0, `advisories ok`; `client-advisories-prepared.json`/`.log` |
| Full-lock workflow structure and `bash -n` | Exit 0; three explicit existing lockfiles, pinned scanner install, offline/no-fetch flags and strict warning rejection checked; `full-lock-checks.json` |
| Execute the extracted CI full-lock shell against the client lockfile | Exit 0, using cargo-audit 0.22.2; `full-lock-client-cache-access.log` |
| Execute the same shell against an isolated synthetic lockfile containing `rsa 0.9.8` | Expected scanner exit 1; detected RUSTSEC-2023-0071; `full-lock-known-vulnerable-cache-access.log` |
| Dependency agent's full-lock scans of original/rebuild/client | Each exit 0, zero vulnerabilities and empty advisory warnings; receipts under `dependency-security/{original,rebuild,client}-lock.audit.{json,stderr,receipt.json}` |

The advisory command, from the isolated worktree root, was:

```sh
NUNCIO_ADVISORY_DB="$PWD/rebuild/test-results/advisory-db" \
  cargo deny --manifest-path rebuild/clients/smoke/Cargo.toml \
  --locked --offline check -c rebuild/deny.toml advisories
```

To reproduce the complete-lock scan after the same provisioning step, run from the worktree root with Bash; repeat with `rebuild/Cargo.lock` and `rebuild/clients/smoke/Cargo.lock`:

```bash
export NUNCIO_ADVISORY_DB="$PWD/rebuild/test-results/advisory-db"
databases=("$NUNCIO_ADVISORY_DB"/advisory-db-*)
test "${#databases[@]}" -eq 1
test -d "${databases[0]}/.git"
CARGO_NET_OFFLINE=true cargo audit --file Cargo.lock \
  --db "${databases[0]}" --no-fetch --deny warnings
```

Local validation used the task-local binary at `rebuild/test-results/dependency-security/audit-tool/bin/cargo-audit`, installed by the dependency agent, and RustSec database commit `b50980aad8b8f14f77e25a97b32dd94bf008b0af` (1,243 advisories). The first sandboxed shell checks returned the expected exits but warned that the crates.io cache lock was inaccessible. Their logs are preserved. The narrowly escalated offline rerun had cache access, no index-access warning, and the same expected exits; see `full-lock-cache-access-checks.json`. Neither client nor synthetic lockfile was changed by the scanner. The original/rebuild dependency remediation agent owns lockfile corrections and complete scan receipts; see [DEPENDENCY-SECURITY-REPORT.md](DEPENDENCY-SECURITY-REPORT.md).

Gitleaks scanned a temporary copy of tracked files plus new source/docs/scripts, with `--redact --no-banner --report-format json`. Its single `generic-api-key` finding is `_reference/nuncio-gui/src-tauri/tauri.conf.json:53`, JSON property `/plugins/updater/pubkey`. A nonprinting parser verified it is a base64-encoded minisign **public** key, not a private key or credential. The redacted finding and scanner exit are retained in `gitleaks-redacted.json`, `gitleaks.log`, and `local-scan.json`; the whole-repository scanner is not falsely reported as exit 0. This snapshot check is not a full-history Gitleaks scan.

No local CodeQL CLI or actionlint executable was available. Workflow structure and command execution were checked locally; actual CodeQL extraction/query execution/upload must be verified in the next hosted run. The cargo-audit installation stayed in the ignored task-local evidence directory. No live account was accessed and no repository settings were modified.

## Hosted evidence and enforcement still to verify

Read-only API inspection found that the prior scheduled CodeQL run [34074376845](https://github.com/KofTwentyTwo/nuncio/actions/runs/34074376845) succeeded on September 7 at `157c7c27b0525a34fe854afdc7054e006cb8e121`. Its only jobs were `Analyze CodeQL (actions)` and `Analyze CodeQL (javascript-typescript)`. Recent analysis records likewise contained those two categories. Their success does not establish Rust/Python or development-branch coverage for this change.

After the root agent's authorized checkpoint push, verify all four CodeQL jobs and the three advisory jobs at that exact commit. Inspect Rust extraction diagnostics for the original and rebuild paths, successful analysis upload, and the `rust`/`python` categories in the code-scanning API. Inspect findings, not only green workflow status: analysis can succeed while reporting vulnerabilities. Keep the exact run URL/SHA/results with the checkpoint. An approved PR targeting `dev` then provides target-branch execution evidence; do not claim a dev-hosted run solely from the feature-branch trigger check.

The `dev` branch was reported protected, but its branch-protection API returned `required_status_checks: null` and `required_pull_request_reviews: null`. Existing scans therefore are not established as mandatory merge gates by that configuration. Separate repository rulesets were not inspected; no claim is made that every possible enforcement layer is absent.

Proposed settings review after the new jobs have run successfully: require the four `Analyze CodeQL (…)` checks, the three `Dependency advisories (…)` checks and applicable quality gates on `dev`, `rc` and `main`; use a code-scanning merge-protection rule if security-alert severity itself should block merges. Exact check names must be taken from the resulting run. Do not add path-filtered rebuild checks as unconditional requirements without handling PRs that legitimately do not run rebuild CI. Ruleset/branch-protection changes and alert policy remain unapproved and were not applied.
