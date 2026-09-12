# Open PR review — September 12, 2026

Current hosted confirmation: [security run 34704460237](https://github.com/KofTwentyTwo/nuncio/actions/runs/34704460237)
passed all seven jobs at signed/pushed `164b021`. The corrected original complete
lock passed cargo-audit (509 packages), and rebuild/client advisory jobs passed.
Nine action findings are verified fixed. The earlier failure and local evidence
below remain historical; no PR was merged or closed. See [SECURITY-CI.md](SECURITY-CI.md)
for the actual code-scanning findings and their limits.


The complete open queue contains **11 PRs**, all Dependabot updates already
incorporated or superseded by the checked dependency/security work. There are no
additional human contribution or native-app PRs to recover. Each current diff,
head/base commit, changed-file list, review state, and check rollup was inspected.
No PR was merged, closed, approved, commented on, or modified remotely.

Inventory uses unfiltered `gh pr list --state open --limit 100` and independently
`gh api --paginate repos/KofTwentyTwo/nuncio/pulls?state=open&per_page=100`;
both return 11. All target `main` at base commit
`157c7c27b0525a34fe854afdc7054e006cb8e121`. All are non-draft and GitHub reports
them mergeable. There are no submitted reviews. Empty review decisions are not
approvals. The results below describe the queried PR heads, not current feature
branch CI or permission to merge.

## Exact heads and dispositions

Each row has the same proposed remote disposition: **retain open until the
consolidated replacement reaches the intended base through an authorized merge,
then close as superseded with the replacement commit reference**. These are
recommendations awaiting authorization; source implementation is already done.

| PR | Exact current head | Existing head checks | Replacement on feature branch |
|---|---|---|---|
| [#439](https://github.com/KofTwentyTwo/nuncio/pull/439) | `b1256d5e451544770ab35c4382096ea555e6d757` | 10 success; CLEAN | tokio-rustls 0.26.5 locked. |
| [#438](https://github.com/KofTwentyTwo/nuncio/pull/438) | `22421ab9d79ac2bfc0434bc1b8da0c8755d4e7fd` | 4 success; 3 failure; 1 skipped; BLOCKED | keyring 4.2.0; explicit native v1 feature and delete_credential adapter. |
| [#437](https://github.com/KofTwentyTwo/nuncio/pull/437) | `701e1318bfc35ca5e7223434eeee1188fb3b5cdd` | 10 success; CLEAN | flate2 1.1.10; corresponding compression dependency update retained. |
| [#436](https://github.com/KofTwentyTwo/nuncio/pull/436) | `02eedc02a16ae17b15b213a4e1fcced5449027b6` | 10 success; CLEAN | Superseded by mail-parser 0.11.9 (requested 0.11.8). |
| [#435](https://github.com/KofTwentyTwo/nuncio/pull/435) | `2d8dc0bc216e503420a677c111bc90da64298f47` | 10 success; CLEAN | quick-xml 0.42.0 plus current UTF-8 event API adapters. |
| [#434](https://github.com/KofTwentyTwo/nuncio/pull/434) | `36cff7b5528795af357826379547d07ffa38cb7e` | 4 success; 3 failure; 1 skipped; BLOCKED | Direct aes-gcm 0.11.1; fallible OS randomness and nonce adapters; old ciphertext fixture passes. |
| [#432](https://github.com/KofTwentyTwo/nuncio/pull/432) | `9ef9e25ca7ef6c30ad6145010ba623035b0b32dc` | 10 success; CLEAN | futures family 0.3.34 locked. |
| [#404](https://github.com/KofTwentyTwo/nuncio/pull/404) | `e208723b0a6ada77c3adf0cd03b1b917e5a40019` | 10 success; CLEAN | async-trait 0.1.92 locked. |
| [#383](https://github.com/KofTwentyTwo/nuncio/pull/383) | `5efd0106cb0e81bc6973e3e94bc18785b4456ad2` | 10 success; CLEAN | clap and clap_builder 4.6.6 locked. |
| [#382](https://github.com/KofTwentyTwo/nuncio/pull/382) | `f4ce73c5e667162e2bbc1ef13ab99152f2ac0183` | 10 success; CLEAN | lettre 0.11.23 with base64 0.23.1 locked. |
| [#379](https://github.com/KofTwentyTwo/nuncio/pull/379) | `03af6926ecc7615dff6726ab973141f78e709547` | 10 success; CLEAN | init/analyze pinned to f205ea1c3313d32999d8d6a48b4f6530d4437b38 (v4.37.4). |

Ten Cargo PRs modify only manifests/lockfiles; #379 changes only the two CodeQL
action references. The full dependency updates and necessary API adaptations are
in signed checkpoint `81a1bc6ce6ea84d82d38f88e21679d51c42cf85b`; the CodeQL
replacement is in `549a5983db507670274422d6f702122b56210702`. See
[DEPENDENCY-SECURITY-REPORT.md](DEPENDENCY-SECURITY-REPORT.md) for all versions,
dependency chains, and verification. These commits are on the feature branch;
this is not a claim that the changes have reached `main`.

## Demonstrated PR defects and fixes

- **#438:** its failed [Clippy job](https://github.com/KofTwentyTwo/nuncio/actions/runs/33590004197/job/100121894765)
  reports `E0599` for removed `keyring::Entry::delete_password` at
  `crates/nuncio-store/src/vault.rs:157` in that PR. The replacement uses
  `delete_credential`, preserves service/account identifiers, and selects the
  native backend explicitly. Its test and coverage jobs fail too; the platform
  matrix is skipped. The unadapted bot PR should not be merged directly.
- **#434:** its failed [Clippy job](https://github.com/KofTwentyTwo/nuncio/actions/runs/32807990339/job/97681709483)
  reports `E0432` for removed `aes_gcm::aead::OsRng` imports in `cipher.rs` and
  `vault.rs`. The replacement uses fallible OS randomness and current nonce APIs;
  encryption, tamper, incorrect-key, and old-ciphertext tests pass. Test/coverage
  failure and a skipped platform matrix remain on the old PR head.
- **#379:** its sole bot comment reports the missing `actions` label. A fresh
  labels API response confirms `actions` is absent and `github_actions` exists.
  The current `.github/dependabot.yml` now uses that existing label. The three
  Cargo directory entries and update schedules remain intact; no repository
  label was created and no remote settings were changed.

Nine green PR heads establish historical checks on their own snapshots. They do
not replace testing the combined dependency graph or the current rebuild. The
quick-xml adapters needed by newer original-workspace source were included in
the combined compatibility work even though #435 reports green on its base.

## Verification and newly observed hosted finding

The combined original compatibility checkpoint passed formatting, strict Clippy,
all **790 tests** (zero failed/ignored), and advisory scans, including independent
old AES-GCM/age ciphertext fixtures. Hosted security later identified yanked
`chacha20 0.10.1` in an optional complete-lock dependency omitted from the selected
graph. The earlier local full-lock scan had a cached registry index; it did not
establish freshness. That failure was preserved.

The bounded follow-up updates only the original lock entry to unyanked 0.10.2
and its reference. Formatting, strict Clippy, all **790 tests**, and cargo-deny
pass again. Cargo-audit 0.22.2 explicitly refreshes RustSec and crates.io and
passes with zero vulnerabilities/warnings; the corrected hosted rerun passed at 164b021. Details and exact current lock hash are in the dependency report.

The label correction is validated against the live label list and parsed YAML;
all configured labels exist and all three Cargo directories remain covered.
This PR review itself does not require another product test run: no additional
Rust source changes were needed. Rebuild crates/manifests/locks were untouched.

## Evidence and remaining actions

Ignored local receipts are in `rebuild/test-results/open-pr-review/`: both complete
inventories, all eleven full diffs, per-PR files/reviews/comments/commits, the two
failed Clippy logs, repository labels, and validation results. Dependency follow-up
gate receipts are in `rebuild/test-results/dependency-security/yank-followup/`.

After successful hosted checks, the remaining actions are an authorized promotion
of the consolidated work through the chosen branch flow and an authorized closure
of the eleven superseded PRs. Do not merge the old bot branches wholesale after
the replacement: #438/#434 lack required adaptations, and the other lockfile
patches duplicate or lag the consolidated graph. Recheck the queue and exact heads
before remote disposition because this report is a point-in-time inventory.
