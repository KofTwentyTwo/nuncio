CREATE TABLE operation_reconciliation_requests (
    account_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    expected_version INTEGER NOT NULL CHECK(typeof(expected_version)='integer' AND expected_version>0),
    mode TEXT NOT NULL CHECK(mode IN ('observe','resume_safe')),
    requested_at_ms INTEGER NOT NULL CHECK(requested_at_ms>=0),
    first_attempt_ordinal INTEGER CHECK(first_attempt_ordinal BETWEEN 1 AND 1000),
    PRIMARY KEY(account_id,request_id),
    FOREIGN KEY(account_id,operation_id) REFERENCES operations(account_id,id)
);
CREATE UNIQUE INDEX operation_reconciliation_version ON operation_reconciliation_requests(account_id,operation_id,expected_version);
INSERT INTO schema_migrations VALUES(21);
PRAGMA user_version=21;
