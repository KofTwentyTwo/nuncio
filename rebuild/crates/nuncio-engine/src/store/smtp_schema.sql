CREATE TABLE smtp_submissions (
 account_id TEXT NOT NULL,
 operation_id TEXT NOT NULL,
 intent_json TEXT NOT NULL CHECK(json_valid(intent_json)),
 step TEXT NOT NULL DEFAULT 'prepared' CHECK(step IN ('prepared','started','accepted','appending','copied')),
 placement_json TEXT CHECK(placement_json IS NULL OR json_valid(placement_json)),
 PRIMARY KEY(account_id,operation_id),
 FOREIGN KEY(account_id,operation_id) REFERENCES send_payloads(account_id,operation_id),
 CHECK((step='copied' AND placement_json IS NOT NULL) OR (step<>'copied' AND placement_json IS NULL))
);
INSERT INTO schema_migrations VALUES(17);
PRAGMA user_version=17;
