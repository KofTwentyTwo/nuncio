# Nuncio progress estimates

September 14, 2026, 12:46 Central: CLI help/readable output is signed and pushed;
detailed startup logging is implemented. Actual CLI checkpoint CI passed both
macOS/Ubuntu E2E jobs but rejected a new Rustls advisory. All three locks are
patched; six fresh advisory scans and original 790 tests pass. The full patched
rebuild gate is running, with 41 Google E2E cases passed in its first workspace.
Installer qualification remains pending; f14b02a predates logging and this fix.
Percentages estimate delivery progress; live/native acceptance remains deferred.

| Major task | Complete | Active hours left | Evidence / remaining work |
|---|---:|---:|---|
| 01 Workspace, encrypted lifecycle and status | 100% | 0 | Implementation and offline checks passed; live provider compatibility is separately pending. |
| 02 Independent stateful Google mock | 100% | 0 | Implementation and offline checks passed; live provider compatibility is separately pending. |
| 03 System and subprocess E2E harnesses | 100% | 0 | Implementation and offline checks passed; live provider compatibility is separately pending. |
| 04 Google OAuth and account lifecycle | 100% | 0 | Implementation and offline checks passed; live provider compatibility is separately pending. |
| 05 Gmail initial sync, MIME and attachments | 100% | 0 | Implementation and offline checks passed; live provider compatibility is separately pending. |
| 06 Gmail incremental sync and reconciliation | 100% | 0 | Implementation and offline checks passed; live provider compatibility is separately pending. |
| 07 Calendar sync and agenda | 100% | 0 | Implementation and offline checks passed; live provider compatibility is separately pending. |
| 08 Background sync and change streaming | 100% | 0 | Implementation and offline checks passed; live provider compatibility is separately pending. |
| 09 Durable drafts and operation journal | 100% | 0 | Implementation and offline checks passed; live provider compatibility is separately pending. |
| 10 Gmail send and mail mutations | 100% | 0 | Implementation and offline checks passed; live provider compatibility is separately pending. |
| 11 Calendar writes, RSVP and free/busy | 100% | 0 | Implementation and offline checks passed; live provider compatibility is separately pending. |
| 12 Synology IMAP/SMTP mail support | 100% | 0 | Implementation and offline checks passed; live provider compatibility is separately pending. |
| 13 Export, backup, restore and repair | 100% | 0 | Implementation and offline checks passed; live provider compatibility is separately pending. |
| 14 Adversarial checks and resource bounds | 95% | 0.5–2 | The earlier Ubuntu attachment timeout did not recur in the next actual CI run; its root cause remains unproven. Strict limits and diagnostics remain. |
| 15 Local packages, API contract and CI | 100% | 0 | Implementation and offline checks passed; live provider compatibility is separately pending. |
| 16 Provider acceptance and final report | 50% | 3–6* | Named Google/Synology/native-keystore acceptance remains explicitly deferred; worksheet now includes concrete native checks. |
| 17 Original-workspace dependency remediation | 95% | 0.25–0.5 | New RUSTSEC-2026-0285 is patched across all three locks. Six fresh advisory checks and original790 tests pass; final delivery checks remain. |
| 18 Development security scanning | 100% | 0 | Actual CI detected the new Rustls advisory and blocked delivery. All four CodeQL jobs passed; findings remain classified, not dismissed. |
| 19 API publication design | 100% | 0 | SemVer/gRPC docs/client/compatibility proposal delivered. Publication pipeline implementation and remote publication are future scope, not claimed complete. |
| 20 Post-engine product roadmap | 100% | 0 | Senior PO/PM milestone plan delivered; its future implementation is outside this goal. |
| 21 Stable testing installer | 100% | 0 | Permanent installer paths delivered at f14b02a; public fresh/repeat/migration/update checks passed. |
| 22 Guided account setup | 100% | 0 | Guided setup, individual-account help and stable commands delivered; live-provider acceptance remains separate. |
| 23 Google application registration | 0% | 0.5–1* | Step-by-step Google walkthrough supplied. Actual Cloud registration remains pending; current CLI can use a private local Desktop JSON. |
| 24 Visible daemon logs and levels | 90% | 0.75–1.5 | Detailed startup phases added at INFO, with DEBUG migration detail. All 41 Google E2E cases pass in the patched first workspace; full verification and delivery remain. |
| 25 CLI help and readable output | 95% | 0.25–0.5 | CLI 35-command gate and eight final checks passed; signed/pushed 92e9394 passed hosted macOS/Ubuntu E2E. New security finding blocks the download. |

Guided setup delivery and the concise setup email are verified.
One-time Google app registration:0.5–1 active hour plus external delays. Deferred
live/native acceptance:3–6hours when authorized. No future native-app/API-publication
implementation has been added to this goal.

Latest hourly chart email: `1a0a1075028ddac0`, September 14 at 12:46:46 Central.
Recipient, subject, SENT label, exact plain/HTML and 251183-byte 25-task chart
metadata were verified. Logging/startup 90%, CLI 95%, dependency follow-up 95%.
Estimated remaining offline work: 1.75–4.5 active hours, including a reserve for
the prior unexplained attachment timeout; CI waiting and external approvals are
excluded. Next report is due 13:46:46 Central during active execution; no
inactive-session scheduler exists.
