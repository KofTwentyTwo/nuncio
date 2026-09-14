# IMAP initial-sync reliability follow-up

Within the approved engine/API/CLI rebuild, fix initial-sync failure when a mailbox changes during download. Retain atomic publication, strict protocol validation, account/UIDVALIDITY isolation, and independent remote-effect evidence. Use at most three mailbox passes; later passes reuse only this run’s immutable bodies, refresh metadata and discard staged expunged placements. Identity changes and continued churn fail explicitly. Valid unsolicited flag notifications trigger reconciliation, including without CONDSTORE; malformed requested data still fails. Mid-command transport loss remains a failure, with the last published snapshot preserved.

1. Reproduce concurrent arrival failure against independent Dovecot — done (RED 101).
2. Implement bounded catch-up and strict flag-notification handling — complete offline gate passes.
3. Arrivals, flags, expunges, repeated churn, UID epoch changes and actual daemon crashes pass; provider effects and raw bytes were independently compared.
4. Time credential/recovery phases; measure large synthetic recovery. The 66,524-entry cleanup took 2.548 seconds locally; the laptop’s 240-second native recovery pause remains unproven.
5. Full offline gate passed (35 commands, 359 tests per workspace configuration); signed/pushed bb70f95, all 17 actual CI jobs and production/testing artifacts now verified. Update operating instructions and request laptop retest only after qualified delivery. No live provider or native Keychain action by the agent.

Implementation: `providers/imap/{sync,read}.rs`, `store/imap_mailboxes/cached.rs`, startup phases and CLI rendering. Regressions: `imap_read_system/catchup.rs`, `support/imap_catchup_e2e.rs`, `support/logging_e2e.rs`, `support/cli_human_e2e.rs`, `sync_recovery_resources.rs`. Exact commands and retained RED/GREEN evidence: `test-results/startup-delay/`. No schema, wire API shape, dependency or credential-format change.
