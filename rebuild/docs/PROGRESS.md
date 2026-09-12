# Delivery progress estimates

As of September12,2026 00:44Central. These are engineering estimates for the
approved sixteen implementation tasks, not measured percentages, R01–R16 sign-off,
or a delivery promise. 100% means implementation and applicable offline checks
passed; final integrated regression and live-provider acceptance remain separate.
Hours mean remaining active engineering/test effort, excluding external waiting
and unknown defects. Shared remaining work is counted once. Native apps excluded.

| Task | Estimated complete | Remaining hours | Remaining work / evidence |
|---|---:|---:|---|
| 01 Workspace, encrypted lifecycle and status | 100% | 0 | Implemented; relevant offline checks passed. |
| 02 Independent stateful Google mock | 100% | 0 | Independent contract coverage includes limited writers and private-event masking. |
| 03 System and subprocess E2E harnesses | 100% | 0 | Full34-command local gate passed with host/container egress controls and actual subprocess tests. |
| 04 Google OAuth and account lifecycle | 100% | 0 | Implemented and offline verified; live OAuth remains Task 16. |
| 05 Gmail initial sync, MIME and attachments | 100% | 0 | Implemented; pagination and byte-fidelity checks passed. |
| 06 Gmail incremental sync and reconciliation | 100% | 0 | Implemented; restart/cursor/reconciliation checks passed. |
| 07 Calendar sync and agenda | 100% | 0 | Implemented; canonical/occurrence sync checked offline. |
| 08 Background sync and change streaming | 100% | 0 | Implemented; scheduling, backoff and cancellation checked. |
| 09 Durable drafts and operation journal | 100% | 0 | Implemented; durable intent and operation history checked. |
| 10 Gmail send and mail mutations | 100% | 0 | Implemented; independent send/label effects checked. |
| 11 Calendar writes, RSVP and free/busy | 100% | 0 | Limited-writer writes and private-event denial passed mock, API and actual CLI checks. |
| 12 Synology IMAP/SMTP mail support | 100% | 0 | Local Dovecot/Mailpit checks passed; live MailPlus is Task 16. |
| 13 Export, backup, restore and repair | 100% | 0 | Implemented; migration and subprocess crash recovery checked. |
| 14 Adversarial checks and resource bounds | 100% | 0 | Security, resource, migration and recovery suites passed in both configurations and named runs. |
| 15 Local packages, API contract and CI | 90% | 1–4 | Final refreshed local archive and all local gates passed; four hosted macOS jobs passed; Linux runner-disconnection fix passes locally and awaits hosted verification. |
| 16 Provider acceptance and final report | 50% | 4–8* | 1–2 h final report/CI evidence; 3–6 h live checks remain deferred/unapproved. |

Offline remaining:2–6hours. Live acceptance:3–6hours after separate named-account
authorization (currently deferred). Total active effort:5–12hours; Task16's4–8
comprises1–2offline and3–6live. Do not convert deferred acceptance into passed.
Calendar role work and the full local regression are complete. Final guide and
package refresh45789 passed two identical archives/22checks each, selected8996085
artifact. Signed checkpoint2e4d30e is pushed; hosted34672641999 has all4macOS jobs
passed and6Ubuntu jobs failed with runner communication loss. The same-UID
filter defect is independently reproduced; cgroup-scoped correction passes local
controller continuity, denial and detached-child cleanup tests. The next checkpoint
will run it on hosted Linux. Existing remaining-work range is retained because the
demonstrated correction is already locally verified; unknown new defects excluded.

James requested this chart/table in every hourly email. Refresh estimates against
current SESSION-STATE/TODO/VERIFICATION evidence before sending; explain material
changes or newly discovered defects. Current renderer and MIME/example artifacts:
`test-results/status-emails/render-progress-2357.py` and `2026-09-11-2357*`.
The renderer is a local reporting aid using existing Pillow; it is outside the
production app. Update its snapshot data/text/time for each report, then render
and inspect the chart. Send embedded PNG plus HTML table/plain-text fallback.
