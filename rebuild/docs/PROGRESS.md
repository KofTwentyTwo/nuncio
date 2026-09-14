# Nuncio progress estimates

September 14: IMAP correction bb70f95 is signed/pushed and qualified through the full 35-command offline gate, all 17 hosted jobs, reproducible local packages and actual public fresh/update installs. Initial-sync catch-up, granular startup timings and CLI hints are delivered. The exact laptop trigger and four-minute native startup cause still need a retest. Google registration and named live/native acceptance remain pending; native apps are future work.

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
| 12 Synology IMAP/SMTP mail support | 95% | 0.5–1* | Offline mailbox-change and crash checks passed; laptop confirmation remains. |
| 13 Export, backup, restore and repair | 100% | 0 | Implementation and offline checks passed; live provider compatibility is separately pending. |
| 14 Adversarial checks and resource bounds | 100% | 0 | Full offline gate, crash recovery and resource checks passed. |
| 15 Local packages, API contract and CI | 100% | 0 | Signed checkpoint, all 17 hosted jobs, local archives and public fresh/update checks passed. |
| 16 Provider acceptance and final report | 50% | 3–6* | Named Google/Synology/native-keystore acceptance remains explicitly deferred; worksheet now includes concrete native checks. |
| 17 Original-workspace dependency remediation | 100% | 0 | Rustls 0.23.45 in all three locks; original 790 tests, six fresh local advisory checks and all three actual hosted advisory jobs pass. |
| 18 Development security scanning | 100% | 0 | Actual CI detected the new Rustls advisory and blocked delivery. All four CodeQL jobs passed; findings remain classified, not dismissed. |
| 19 API publication design | 100% | 0 | SemVer/gRPC docs/client/compatibility proposal delivered. Publication pipeline implementation and remote publication are future scope, not claimed complete. |
| 20 Post-engine product roadmap | 100% | 0 | Senior PO/PM milestone plan delivered; its future implementation is outside this goal. |
| 21 Stable testing installer | 100% | 0 | Permanent installer paths delivered at f14b02a; public fresh/repeat/migration/update checks passed. |
| 22 Guided account setup | 100% | 0 | Guided setup, individual-account help and stable commands delivered; live-provider acceptance remains separate. |
| 23 Google application registration | 0% | 0.5–1* | Step-by-step Google walkthrough supplied. Actual Cloud registration remains pending; current CLI can use a private local Desktop JSON. |
| 24 Visible daemon logs and levels | 95% | 0.5–1* | Granular logs delivered; native four-minute startup cause needs laptop timings. |
| 25 CLI help and readable output | 100% | 0 | Readable output, all help pages, safe typo suggestions and unavailable-cache guidance verified. |

Percentages describe the named deliverable and its checks; they are estimates, not live-provider sign-off. Task hours overlap. An asterisk means access or laptop feedback is required; additional native fixes cannot yet be estimated. API publication and the post-engine roadmap are completed planning deliverables, not implemented future products.

Hourly chart/status email 1a0a1ad32db03bf2 was verified SENT at 15:47:57 Central (248985-byte chart). The qualified install/retest instructions and updated chart were verified in email 1a0a1b012cf5aead at 15:51:05 Central (248281 bytes). Recipient james@kof22.com, exact plain/HTML bodies and chart metadata checked. Next hourly report is due 16:47:57 Central while active; no inactive-session scheduler exists. See [the current evidence](VERIFICATION.md).
