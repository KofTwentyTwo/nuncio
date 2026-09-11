CREATE TABLE mail_change_payloads (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    provider_message_id TEXT NOT NULL,
    payload_json TEXT NOT NULL CHECK(length(CAST(payload_json AS BLOB))<=65536),
    UNIQUE(account_id,operation_id),
    FOREIGN KEY(account_id,operation_id) REFERENCES operations(account_id,id)
);
CREATE INDEX mail_change_sequence ON mail_change_payloads(account_id,provider_message_id,sequence);
INSERT INTO schema_migrations VALUES (11);
PRAGMA user_version=11;
