# Nuncio progress estimates

September 14: laptop feedback reopened IMAP initial-sync reliability and startup
latency. Concurrent arrivals and flag notifications now have reproduced defects
and passing targeted fixes; full offline/hosted/artifact qualification is still
in progress. The qualified public build remains 7792643. Native credentials and
live Google/Synology acceptance remain separately deferred; mock results do not
close those checks. Native apps remain future work.

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
| 12 Synology IMAP/SMTP mail support | 95% | 1–2 | Concurrent mailbox-change fixes pass targeted tests; full qualification and laptop retest remain. |
| 13 Export, backup, restore and repair | 100% | 0 | Implementation and offline checks passed; live provider compatibility is separately pending. |
| 14 Adversarial checks and resource bounds | 95% | 0.5–1 | New catch-up/crash checks pass; full gate running. Synthetic 66,524-entry cleanup takes 2.55 s; native pause unproven. |
| 15 Local packages, API contract and CI | 95% | 0.5–1 | New source still needs signed checkpoint, actual hosted CI and qualified testing artifacts. |
| 16 Provider acceptance and final report | 50% | 3–6* | Named Google/Synology/native-keystore acceptance remains explicitly deferred; worksheet now includes concrete native checks. |
| 17 Original-workspace dependency remediation | 100% | 0 | Rustls 0.23.45 in all three locks; original 790 tests, six fresh local advisory checks and all three actual hosted advisory jobs pass. |
| 18 Development security scanning | 100% | 0 | Actual CI detected the new Rustls advisory and blocked delivery. All four CodeQL jobs passed; findings remain classified, not dismissed. |
| 19 API publication design | 100% | 0 | SemVer/gRPC docs/client/compatibility proposal delivered. Publication pipeline implementation and remote publication are future scope, not claimed complete. |
| 20 Post-engine product roadmap | 100% | 0 | Senior PO/PM milestone plan delivered; its future implementation is outside this goal. |
| 21 Stable testing installer | 100% | 0 | Permanent installer paths delivered at f14b02a; public fresh/repeat/migration/update checks passed. |
| 22 Guided account setup | 100% | 0 | Guided setup, individual-account help and stable commands delivered; live-provider acceptance remains separate. |
| 23 Google application registration | 0% | 0.5–1* | Step-by-step Google walkthrough supplied. Actual Cloud registration remains pending; current CLI can use a private local Desktop JSON. |
| 24 Visible daemon logs and levels | 95% | 0.25–0.5 | Granular key lookup/deletion/recovery phases pass targeted checks; delivery and native retest pending. |
| 25 CLI help and readable output | 98% | 0.25–0.5 | Safe typo suggestions and unavailable-snapshot guidance pass targeted subprocess checks; delivery pending. |

Percentages describe the named deliverable and its checks; they are engineering
estimates, not live-provider sign-off. Prior proposed API publication and native
roadmap work are planning deliverables, not implemented future products.

Hourly status email `1a0a17628e4c5104` sent September 14 at 14:47:50 Central to james@kof22.com. SENT, recipient, exact plain/HTML bodies and 251798-byte chart metadata verified. It reports passing targeted fixes, full verification still running, qualified public source still 7792643, and native/live gaps. The 25-task chart reopens IMAP/reliability/delivery/logging/help follow-ups; 2–4 active hours estimated, overlapping task estimates and CI waits excluded. Next hourly report due 15:47:50 Central while active; no inactive-session scheduler.
