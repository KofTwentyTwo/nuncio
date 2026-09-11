CREATE TABLE imap_transfer_progress (
 account_id TEXT NOT NULL,
 operation_id TEXT NOT NULL,
 phase TEXT NOT NULL CHECK(phase IN ('started','copied','deleting_source','expunging_source','source_removed')),
 wire_mode TEXT NOT NULL CHECK(wire_mode IN ('copy','move')),
 copy_json TEXT CHECK(copy_json IS NULL OR json_valid(copy_json)),
 PRIMARY KEY(account_id,operation_id),
 FOREIGN KEY(account_id,operation_id) REFERENCES mail_change_payloads(account_id,operation_id),
 CHECK((phase='started' AND copy_json IS NULL) OR (phase<>'started' AND copy_json IS NOT NULL))
);
INSERT INTO schema_migrations VALUES(15);
PRAGMA user_version=15;
