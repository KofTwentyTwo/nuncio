# Nuncio Rebuild — Execution Goal

**Status:** Ready to adopt. Writing this goal has not started implementation or an active execution goal.

Build the complete personal engine and CLI in the [implementation plan](superpowers/plans/2026-09-10-google-first-rebuild.md), using its [specification](superpowers/specs/2026-09-10-google-first-rebuild.md). Google Gmail and Calendar come first; Synology MailPlus email through IMAP/SMTP follows. Native applications come later.

The first checkpoint is a usable Google reader. Full completion includes drafts and sending, mail changes, calendar changes, durable operations, background synchronization, encrypted storage, export/backup/recovery, local release artifacts, and provider acceptance. The stateful mock Gmail provider is mandatory; it also covers Google OAuth and Calendar so system and subprocess E2E tests exercise the real engine through the complete path.

## Copy this to start implementation

```text
Create an active goal to fully implement the Nuncio rebuild described in:

- docs/superpowers/specs/2026-09-10-google-first-rebuild.md
- docs/superpowers/plans/2026-09-10-google-first-rebuild.md

Read both documents, the September 10 engineering audit, repository guidance,
and any existing rebuild session state before making changes. Do not set a
token budget unless I give you one.

I approve the proposed scope and engineering defaults in those documents:
engine and CLI first; Google Gmail and Calendar, then Synology MailPlus
IMAP/SMTP; Rust, SQLCipher-backed SQLite, a versioned authenticated local gRPC
API, and a separate rebuild/ workspace. Native apps are outside this goal.

Implement every task and satisfy requirements R01 through R16. Build the
independent, stateful mock Google provider before the production Google
adapter. Use it for separate system tests and actual daemon/CLI subprocess
E2E tests. Validate remote send/copy/notification effects independently from
local results, especially after crashes and lost acknowledgements. Include
strict IMAP/SMTP testing against local independent server implementations.

I authorize the local feature branch or isolated worktree, new workspace,
manifests, dependencies, schema, API, local test services, CI workflow files,
documentation, and implementation fixes required by this plan. No issue is
required for this personal build. Preserve existing source, original data,
and unrelated work. Do not commit, push, merge, publish, install into my
normal environment, or change remote settings without my authorization.

Work through the plan autonomously, implementing and verifying each task.
Use inline execution unless I request delegation. Continue past intermediate
milestones; a scaffold, mock provider, or read-only client is not completion.
Resolve failures within scope and run the relevant checks. Do not weaken
requirements, mock validation, or assertions to manufacture a passing result.

Keep rebuild/docs/SESSION-STATE.md, rebuild/docs/TODO.md, and
rebuild/docs/VERIFICATION.md current so another session can resume exactly.
Record commands, exit statuses, test evidence, implementation locations,
remaining defects, and the next concrete action. Continue from that state
after context resets. Do not restart the planning process unnecessarily.

Ask early for any account setup or access that only I can supply, using a
secure credential-entry path. All automated tests must remain offline from
real providers. Prepare a concrete manual acceptance worksheet and obtain my
explicit authorization for named live accounts, recipients, calendars, and
actions before accessing them or sending mail/invitations. Continue all
independent implementation and offline verification while that is pending.

Do not mark the goal complete until the full scope works through the engine,
API, and CLI; all required offline checks pass; local production artifacts
are verified; and the authorized Google and Synology acceptance checks have
passed. If external access is missing, report the completed work and precise
pending condition honestly and follow the goal tool's blocked-state rules.

Finish with the requirement-to-evidence matrix, actual test results, local
artifact paths, operating/recovery instructions, provider compatibility
limits, and any remaining risks. Never claim that passing mocks proves live
provider compatibility or that local CI-equivalent checks prove remote CI ran.
```

## Completion checklist

- [ ] Google account setup, Gmail and Calendar reads/writes, offline search/agenda, attachments, and background refresh work through the real CLI.
- [ ] Synology IMAP/SMTP mail works with verified TLS, placements, UID recovery, and correct Sent-copy behavior.
- [ ] Local drafts and operation intent survive restarts, resync, and backup/restore. Unknown remote outcomes cannot trigger blind duplicate sends.
- [ ] Independent mock Google conformance, real system tests, subprocess E2E, IMAP/SMTP compatibility, failure, security, and release checks pass with no ignored required tests.
- [ ] Versioned API and local production artifacts are usable; test controls and credentials are excluded from shipped builds.
- [ ] Named disposable Google and Synology accounts pass separately authorized manual acceptance, with evidence and limitations recorded.
- [ ] Every requirement R01–R16 has implementation and verification evidence. No required work remains.

The proposed implementation details become accepted when you issue the execution prompt. Until then, this is a reviewable plan, and the existing root roadmap and application remain unchanged.
