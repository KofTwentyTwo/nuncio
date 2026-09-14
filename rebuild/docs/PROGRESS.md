# Nuncio progress estimates

September14,2026: daemon activity logging is implemented and its focused Google
and independent IMAP/SMTP checks pass. The full35-command regression passed; hosted/testing delivery is
in progress under task24. Rows01–23 retain prior qualified work; percentages are
engineering estimates, and live/native acceptance remains separate.

| Major task | Complete | Active hours left | Evidence / remaining work |
|---|---:|---:|---|
| 01 Workspace, encrypted lifecycle and status | 100% | 0 | Full current offline regression passed; live acceptance remains separate. New guided-setup download qualification is tracked under task22. |
| 02 Independent stateful Google mock | 100% | 0 | Full current offline regression passed; live acceptance remains separate. New guided-setup download qualification is tracked under task22. |
| 03 System and subprocess E2E harnesses | 100% | 0 | Full current offline regression passed; live acceptance remains separate. New guided-setup download qualification is tracked under task22. |
| 04 Google OAuth and account lifecycle | 100% | 0 | Full current offline regression passed; live acceptance remains separate. New guided-setup download qualification is tracked under task22. |
| 05 Gmail initial sync, MIME and attachments | 100% | 0 | Full current offline regression passed; live acceptance remains separate. New guided-setup download qualification is tracked under task22. |
| 06 Gmail incremental sync and reconciliation | 100% | 0 | Full current offline regression passed; live acceptance remains separate. New guided-setup download qualification is tracked under task22. |
| 07 Calendar sync and agenda | 100% | 0 | Full current offline regression passed; live acceptance remains separate. New guided-setup download qualification is tracked under task22. |
| 08 Background sync and change streaming | 100% | 0 | Full current offline regression passed; live acceptance remains separate. New guided-setup download qualification is tracked under task22. |
| 09 Durable drafts and operation journal | 100% | 0 | Full current offline regression passed; live acceptance remains separate. New guided-setup download qualification is tracked under task22. |
| 10 Gmail send and mail mutations | 100% | 0 | Full current offline regression passed; live acceptance remains separate. New guided-setup download qualification is tracked under task22. |
| 11 Calendar writes, RSVP and free/busy | 100% | 0 | Full current offline regression passed; live acceptance remains separate. New guided-setup download qualification is tracked under task22. |
| 12 Synology IMAP/SMTP mail support | 100% | 0 | Full current offline regression passed; live acceptance remains separate. New guided-setup download qualification is tracked under task22. |
| 13 Export, backup, restore and repair | 100% | 0 | Full current offline regression passed; live acceptance remains separate. New guided-setup download qualification is tracked under task22. |
| 14 Adversarial checks and resource bounds | 100% | 0 | Full current offline regression passed; live acceptance remains separate. New guided-setup download qualification is tracked under task22. |
| 15 Local packages, API contract and CI | 100% | 0 | Earlier full delivery passed; current guided-setup package and CI qualification tracked under22. |
| 16 Provider acceptance and final report | 50% | 3–6* | Named Google/Synology/native-keystore acceptance remains explicitly deferred; worksheet now includes concrete native checks. |
| 17 Original-workspace dependency remediation | 100% | 0 | All11PRs reviewed/incorporated or superseded; original790tests and fresh-index audit pass. Final yank/label correction signed/pushed; all3current hosted advisory jobs pass. |
| 18 Development security scanning | 100% | 0 | All7hosted security jobs passed at78d4d9e. Earlier findings remain classified; no claim of changed default branch settings. |
| 19 API publication design | 100% | 0 | SemVer/gRPC docs/client/compatibility proposal delivered. Publication pipeline implementation and remote publication are future scope, not claimed complete. |
| 20 Post-engine product roadmap | 100% | 0 | Senior PO/PM milestone plan delivered; its future implementation is outside this goal. |
| 21 Curl-to-Bash testing installer | 100% | 0 | Public pipeline and temporary installation verified; James reports laptop quick start worked. |
| 22 Guided account setup | 100% | 0 | Implementation, full offline checks, reproducible packages, all 17 hosted jobs and public installation passed. |
| 23 Google application registration | 0% | 0.5–1* | Laptop walkthrough prepared; registration creation remains pending. The current CLI accepts a private local Desktop JSON without waiting for a shared build. |
| 24 Visible daemon logs and levels | 95% | 0.25–0.75 | Full35-command offline gate passed, including log privacy and independent remote effects; signed push and actual hosted/current artifact qualification remain. |

Guided setup delivery and the concise setup email are verified.
One-time Google app registration:0.5–1 active hour plus external delays. Deferred
live/native acceptance:3–6hours when authorized. No future native-app/API-publication
implementation has been added to this goal.

Latest hourly chart email: message1a09d5480258dced, September13 at19:32Central.
Updated laptop instructions were separately sent September14 at09:22Central
(message1a0a04c78dba5481). Hourly reporting applies during active execution;
no inactive-session scheduler is established.
