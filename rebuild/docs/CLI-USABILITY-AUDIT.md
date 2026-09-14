# CLI usability audit and implementation

September 14, 2026. James reported that `mail list` hides the missing account
argument, many commands have blank descriptions, and normal output is JSON.
Use readable terminal output by default and preserve the explicit `--json`
contract, exit codes, byte exports, and JSONL change stream. This is an inline
follow-up to the accepted engine/CLI scope; no new provider or API is needed.

- [x] Reproduce missing-argument, incomplete-command, and unhelpful runtime errors.
- [x] Audit every help page and visible option; add descriptions, ID lookup guidance,
      safe examples, file-format instructions, and local/remote effects.
- [x] Add safe human output and actionable errors; keep machine JSON, uncertainty
      receipts, pagination, secret redaction and terminal control escaping intact.
- [x] Exercise the actual CLI offline; pass the full gate, signed checkpoint/push,
      actual hosted jobs and both public fresh/update installations at 7792643.

Initial findings: parser failures discard Clap's useful required-argument context;
the error renderer prints both a generic sentence and a JSON envelope even without
`--json`; numerous mail/calendar/system/draft/operation commands and options have
no explanation. Some file parsers also collapse missing, malformed and unsupported
input into the same message. Audit evidence belongs in `test-results/cli-usability/`.
The older Ubuntu attachment timeout remains unexplained. Current strict local
and hosted resource checks pass; its failing evidence and diagnostics remain.

The implemented audit covers 71 help pages, 531 visible option entries (including
shared global options) and 192 actual missing/unknown-argument invocations. All
192 passed without creating a profile, contacting a provider, printing rejected
values, or changing the JSON error contract. All 31 blank descriptions are filled.
The full frozen-source offline gate passed all 35 commands and 345 tests per
workspace configuration. Final account/backup help and known-RPC-message guidance
passed eight supplementary checks, including CLI 22 normal/23 harness tests,
both Clippy configurations and the complete production-binary help audit.
Exact three-file source delta: final-leaf-summary.json. Signed/pushed 92e9394
passed both hosted macOS/Ubuntu E2E jobs; new Rustls advisory failures blocked
delivery. All three locks are patched in the startup-logging follow-up. Its full
35-command verification, all 17 hosted jobs and actual public fresh/update
qualification passed at 7792643. Both installed packages pass the 71/531/192 audit.

| Finding | Change / evidence |
|---|---|
| `mail list` hid the required account | Human stderr names `--account <ACCOUNT_ID>`, prints usage, and explains `account list` plus profile selection. |
| Normal results/errors were JSON | Labeled fields and numbered results are now default; UTC timestamps, explicit empty results and indented mail text remain readable. `--json` preserves machine envelopes. |
| Write request IDs were undocumented UUIDs | Help explains UUIDs; malformed IDs fail locally before connecting. The new real send walkthrough exposed this omission. |
| Parser errors could become unsafe if printed verbatim | Guidance is built from declared commands/options; rejected values are never echoed. Control-character/token canaries are tested. |
| Action-file errors gave no next step | Mail/calendar/draft/recovery-decision errors name their schema/limits and help page; help includes independently reviewed JSON examples. |
| Google setup error implied a new build was mandatory | The message now points to private Desktop JSON via `--client-config` and the one-time setup guide. |
| Provider auth/TLS failures looked like local daemon problems | Final review adds allowlisted recovery guidance for known API errors, retaining error codes/exits and redaction of arbitrary server details. |
| Existing security checks assumed both modes were JSON | The hostile-mail E2E now checks readable mode and lossless explicit JSON separately, preserving byte exports, path protection and independent remote-state checks. |

Source: `crates/nuncio-cli/src/{args,help_text,usage,human,output}.rs` and input
boundaries. Regressions: CLI output tests, `support/cli_human_e2e.rs`, and the
existing Google/IMAP/security subprocess suites. Human output is for people;
automation must use `--json`. `system watch` remains JSONL in both modes.
