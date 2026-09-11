CREATE TABLE drafts (
    account_id TEXT NOT NULL REFERENCES accounts(id),
    id TEXT NOT NULL,
    version INTEGER NOT NULL CHECK(typeof(version)='integer' AND version>0),
    subject TEXT NOT NULL CHECK(length(subject)<=8192),
    content_json TEXT NOT NULL CHECK(length(CAST(content_json AS BLOB))<=8388608),
    created_at_ms INTEGER NOT NULL CHECK(created_at_ms>=0),
    updated_at_ms INTEGER NOT NULL CHECK(updated_at_ms>=created_at_ms),
    PRIMARY KEY(account_id,id)
);
CREATE INDEX drafts_order ON drafts(account_id,updated_at_ms DESC,id);
CREATE TABLE draft_attachments (
    account_id TEXT NOT NULL,
    draft_id TEXT NOT NULL,
    id TEXT NOT NULL,
    position INTEGER NOT NULL CHECK(position>=0 AND position<256),
    blob_id TEXT NOT NULL,
    filename TEXT,
    mime_type TEXT NOT NULL,
    content_id TEXT,
    disposition TEXT NOT NULL CHECK(disposition IN ('attachment','inline')),
    PRIMARY KEY(account_id,draft_id,id),
    UNIQUE(account_id,draft_id,position),
    FOREIGN KEY(account_id,draft_id) REFERENCES drafts(account_id,id) ON DELETE CASCADE,
    FOREIGN KEY(account_id,blob_id) REFERENCES blobs(account_id,id)
);
INSERT INTO schema_migrations VALUES (7);
PRAGMA user_version=7;
