CREATE TABLE operations (
    account_id TEXT NOT NULL REFERENCES accounts(id),
    id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('send','mail_change','calendar_change','imap_change')),
    resource_id TEXT NOT NULL,
    fingerprint TEXT NOT NULL CHECK(length(fingerprint)=64),
    request_json TEXT NOT NULL CHECK(length(CAST(request_json AS BLOB))<=2097152),
    desired_json TEXT NOT NULL CHECK(length(CAST(desired_json AS BLOB))<=2097152),
    state TEXT NOT NULL CHECK(state IN ('queued','running','applied','retry_wait','conflict','uncertain','failed','cancelled')),
    version INTEGER NOT NULL DEFAULT 1 CHECK(typeof(version)='integer' AND version>0),
    created_at_ms INTEGER NOT NULL CHECK(created_at_ms>=0),
    updated_at_ms INTEGER NOT NULL CHECK(updated_at_ms>=created_at_ms),
    next_attempt_at_ms INTEGER,
    needs_reconciliation INTEGER NOT NULL DEFAULT 0 CHECK(needs_reconciliation IN (0,1)),
    error_code TEXT CHECK(length(error_code)<=80),
    disposition TEXT CHECK(disposition IN ('abandoned','resend_requested','manual_confirmed')),
    PRIMARY KEY(account_id,id)
);
CREATE UNIQUE INDEX operations_request_identity ON operations(account_id,request_id);
CREATE INDEX operations_order ON operations(account_id,created_at_ms DESC,id);
CREATE TABLE send_payloads (
    account_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    draft_id TEXT NOT NULL,
    draft_version INTEGER NOT NULL CHECK(draft_version>0),
    sender TEXT NOT NULL,
    recipients_json TEXT NOT NULL,
    message_id TEXT NOT NULL,
    thread_id TEXT,
    wire_blob_id TEXT NOT NULL,
    sent_blob_id TEXT,
    PRIMARY KEY(account_id,operation_id),
    FOREIGN KEY(account_id,operation_id) REFERENCES operations(account_id,id),
    FOREIGN KEY(account_id,wire_blob_id) REFERENCES blobs(account_id,id),
    FOREIGN KEY(account_id,sent_blob_id) REFERENCES blobs(account_id,id)
);
CREATE TABLE operation_attempts (
    account_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK(ordinal>0),
    kind TEXT NOT NULL CHECK(kind IN ('dispatch','reconcile')),
    started_at_ms INTEGER NOT NULL CHECK(started_at_ms>=0),
    finished_at_ms INTEGER CHECK(finished_at_ms>=started_at_ms),
    outcome TEXT CHECK(outcome IN ('applied','rejected','repeatable','conflict','uncertain')),
    error_code TEXT CHECK(length(error_code)<=80),
    PRIMARY KEY(account_id,operation_id,ordinal),
    FOREIGN KEY(account_id,operation_id) REFERENCES operations(account_id,id)
);
CREATE UNIQUE INDEX operation_active_attempt ON operation_attempts(account_id) WHERE finished_at_ms IS NULL;
CREATE TABLE operation_receipts (
    account_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    attempt_ordinal INTEGER NOT NULL,
    sequence INTEGER NOT NULL CHECK(sequence>0),
    observed_at_ms INTEGER NOT NULL,
    receipt_json TEXT NOT NULL CHECK(length(CAST(receipt_json AS BLOB))<=65536),
    PRIMARY KEY(account_id,operation_id,attempt_ordinal,sequence),
    FOREIGN KEY(account_id,operation_id,attempt_ordinal) REFERENCES operation_attempts(account_id,operation_id,ordinal)
);
CREATE TABLE operation_resolutions (
    account_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    sequence INTEGER NOT NULL CHECK(sequence>0),
    decided_at_ms INTEGER NOT NULL,
    decision TEXT NOT NULL CHECK(decision IN ('abandon','confirm_applied','resend')),
    evidence_json TEXT NOT NULL CHECK(length(CAST(evidence_json AS BLOB))<=65536),
    replacement_id TEXT,
    PRIMARY KEY(account_id,operation_id,sequence),
    FOREIGN KEY(account_id,operation_id) REFERENCES operations(account_id,id),
    FOREIGN KEY(account_id,replacement_id) REFERENCES operations(account_id,id)
);
INSERT INTO schema_migrations VALUES (10);
PRAGMA user_version=10;
