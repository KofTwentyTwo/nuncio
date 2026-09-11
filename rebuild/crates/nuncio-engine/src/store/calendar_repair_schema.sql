-- Repair can discover new calendars without exposing placeholder live rows.
CREATE TABLE staged_calendar_repair_ids (
    account_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    calendar_id TEXT NOT NULL,
    PRIMARY KEY(account_id,run_id,provider_id),
    UNIQUE(account_id,run_id,calendar_id),
    FOREIGN KEY(account_id,run_id,provider_id)
        REFERENCES staged_calendar_catalog(account_id,run_id,provider_id) ON DELETE CASCADE
);
CREATE TABLE staged_calendar_repair_events (
    account_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    calendar_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('canonical','occurrence')),
    provider_id TEXT NOT NULL,
    object_json TEXT NOT NULL CHECK(json_valid(object_json)),
    status TEXT NOT NULL CHECK(status IN ('confirmed','tentative','cancelled')),
    etag TEXT,
    start_ms INTEGER,
    end_ms INTEGER,
    PRIMARY KEY(account_id,run_id,calendar_id,kind,provider_id),
    FOREIGN KEY(account_id,run_id,calendar_id)
        REFERENCES staged_calendar_repair_ids(account_id,run_id,calendar_id) ON DELETE CASCADE
);
INSERT INTO schema_migrations VALUES(20);
PRAGMA user_version=20;
