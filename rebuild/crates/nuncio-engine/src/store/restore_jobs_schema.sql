CREATE TABLE restore_cleanup_jobs (
    profile_id TEXT PRIMARY KEY NOT NULL,
    record_json TEXT NOT NULL CHECK(json_valid(record_json)),
    activating INTEGER NOT NULL DEFAULT 0 CHECK(activating IN (0,1))
);
INSERT INTO schema_migrations VALUES(22);
PRAGMA user_version=22;
