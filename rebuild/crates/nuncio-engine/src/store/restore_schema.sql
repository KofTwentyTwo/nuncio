CREATE TABLE restored_operations (
    account_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    backup_sha256 TEXT NOT NULL CHECK(length(backup_sha256)=64),
    source_state TEXT NOT NULL CHECK(source_state IN ('queued','running','retry_wait','conflict','uncertain')),
    source_version INTEGER NOT NULL CHECK(typeof(source_version)='integer' AND source_version>0),
    restored_at_ms INTEGER NOT NULL CHECK(restored_at_ms>=0),
    PRIMARY KEY(account_id,operation_id,backup_sha256),
    FOREIGN KEY(account_id,operation_id) REFERENCES operations(account_id,id)
);
INSERT INTO schema_migrations VALUES (19);
PRAGMA user_version=19;
