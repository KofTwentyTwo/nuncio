CREATE TABLE calendar_change_payloads (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    provider_calendar_id TEXT NOT NULL,
    provider_event_id TEXT NOT NULL,
    payload_json TEXT NOT NULL CHECK(json_valid(payload_json) AND length(payload_json)<=4194304),
    UNIQUE(account_id,operation_id),
    FOREIGN KEY(account_id,operation_id) REFERENCES operations(account_id,id)
);
CREATE INDEX calendar_change_order ON calendar_change_payloads(account_id,provider_calendar_id,provider_event_id,sequence);
INSERT INTO schema_migrations VALUES (12);
PRAGMA user_version = 12;
