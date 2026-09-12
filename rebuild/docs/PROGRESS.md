# Delivery progress estimates

As of September11,2026 22:57Central. These are engineering estimates for the
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
| 15 Local packages, API contract and CI | 90% | 1–4 | Repeatable local archives and all local gates passed; actual hosted Linux/macOS CI is running. |
| 16 Provider acceptance and final report | 50% | 4–8* | 1–2 h final report/CI evidence; 3–6 h live checks remain deferred/unapproved. |

Offline remaining:2–6hours. Live acceptance:3–6hours after separate named-account
authorization (currently deferred). Total active effort:5–12hours; Task16's4–8
comprises1–2offline and3–6live. Do not convert deferred acceptance into passed.
Calendar role work and full regression are now complete. Release preparation
now has the full34-command local gate and repeatable archives verified. Signed
checkpoint7f81b73 is pushed; actual hosted CI34671490682 is running, with the macOS
mock-contract job passed. Finish hosted checks and final acceptance evidence.

James requested this chart/table in every hourly email. Refresh estimates against
current SESSION-STATE/TODO/VERIFICATION evidence before sending; explain material
changes or newly discovered defects. Current renderer and MIME/example artifacts:
`test-results/status-emails/render-progress-2257.py` and `2026-09-11-2257*`.
The renderer is a local reporting aid using existing Pillow; it is outside the
production app. Update its snapshot data/text/time for each report, then render
and inspect the chart. Send embedded PNG plus HTML table/plain-text fallback.
