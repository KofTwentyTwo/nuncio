# Delivery progress estimates

As of September11,2026 20:07Central. These are engineering estimates for the
approved sixteen implementation tasks, not measured percentages, R01–R16 sign-off,
or a delivery promise. 100% means implementation and applicable offline checks
passed; final integrated regression and live-provider acceptance remain separate.
Hours mean remaining active engineering/test effort, excluding external waiting
and unknown defects. Shared remaining work is counted once. Native apps excluded.

| Task | Estimated complete | Remaining hours | Remaining work / evidence |
|---|---:|---:|---|
| 01 Workspace, encrypted lifecycle and status | 100% | 0 | Implemented; relevant offline checks passed. |
| 02 Independent stateful Google mock | 95% | 1–2 | Add limited-writer/private-event contract coverage. |
| 03 System and subprocess E2E harnesses | 90% | 2–4 | Explicit external-egress denial and its verification. |
| 04 Google OAuth and account lifecycle | 100% | 0 | Implemented and offline verified; live OAuth remains Task 16. |
| 05 Gmail initial sync, MIME and attachments | 100% | 0 | Implemented; pagination and byte-fidelity checks passed. |
| 06 Gmail incremental sync and reconciliation | 100% | 0 | Implemented; restart/cursor/reconciliation checks passed. |
| 07 Calendar sync and agenda | 100% | 0 | Implemented; canonical/occurrence sync checked offline. |
| 08 Background sync and change streaming | 100% | 0 | Implemented; scheduling, backoff and cancellation checked. |
| 09 Durable drafts and operation journal | 100% | 0 | Implemented; durable intent and operation history checked. |
| 10 Gmail send and mail mutations | 100% | 0 | Implemented; independent send/label effects checked. |
| 11 Calendar writes, RSVP and free/busy | 95% | 1–3 | Limited-writer adapter support and system/CLI verification. |
| 12 Synology IMAP/SMTP mail support | 100% | 0 | Local Dovecot/Mailpit checks passed; live MailPlus is Task 16. |
| 13 Export, backup, restore and repair | 100% | 0 | Implemented; migration and subprocess crash recovery checked. |
| 14 Adversarial checks and resource bounds | 85% | 3–6 | Finish remaining findings and the full offline verification run. |
| 15 Local packages, API contract and CI | 25% | 6–12 | Descriptor/client smoke, dependency review, archives, CI, docs. |
| 16 Provider acceptance and final report | 15% | 5–10* | 2–4h offline report/matrix; 3–6h live checks, deferred/unapproved. |

Offline remaining:15–31hours. Live acceptance:3–6hours after separate named-account
authorization (currently deferred). Total active effort:18–37hours; Task16's5–10
comprises2–4offline and3–6live. Do not convert deferred acceptance into passed.

James requested this chart/table in every hourly email. Refresh estimates against
current SESSION-STATE/TODO/VERIFICATION evidence before sending; explain material
changes or newly discovered defects. Current renderer and MIME/example artifacts:
`test-results/status-emails/render-progress.py` and `2026-09-11-1957-revised*`.
The renderer is a local reporting aid using existing Pillow; it is outside the
production app. Update its snapshot data/text/time for each report, then render
and inspect the chart. Send embedded PNG plus HTML table/plain-text fallback.
