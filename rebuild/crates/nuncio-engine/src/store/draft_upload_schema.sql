CREATE TABLE draft_uploads (
    account_id TEXT NOT NULL,
    id TEXT NOT NULL,
    draft_id TEXT NOT NULL,
    expected_version INTEGER NOT NULL CHECK(expected_version>0),
    filename TEXT,
    mime_type TEXT NOT NULL,
    content_id TEXT,
    disposition TEXT NOT NULL CHECK(disposition IN ('attachment','inline')),
    byte_length INTEGER NOT NULL CHECK(byte_length>=0 AND byte_length<=67108864),
    sha256 TEXT NOT NULL CHECK(length(sha256)=64),
    received INTEGER NOT NULL DEFAULT 0 CHECK(received>=0 AND received<=byte_length),
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY(account_id,id),
    FOREIGN KEY(account_id,draft_id) REFERENCES drafts(account_id,id) ON DELETE CASCADE
);
CREATE TABLE draft_upload_chunks (
    account_id TEXT NOT NULL,
    upload_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK(ordinal>=0),
    data BLOB NOT NULL CHECK(length(data)>0 AND length(data)<=262144),
    PRIMARY KEY(account_id,upload_id,ordinal),
    FOREIGN KEY(account_id,upload_id) REFERENCES draft_uploads(account_id,id) ON DELETE CASCADE
);
INSERT INTO schema_migrations VALUES (8);
PRAGMA user_version=8;
