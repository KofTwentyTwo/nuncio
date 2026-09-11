CREATE TABLE blobs (
    account_id TEXT NOT NULL REFERENCES accounts(id),
    id TEXT NOT NULL,
    sha256 TEXT NOT NULL CHECK(length(sha256)=64),
    byte_length INTEGER NOT NULL CHECK(byte_length>=0 AND byte_length<=67108864),
    PRIMARY KEY(account_id,id),
    UNIQUE(account_id,sha256)
);
CREATE TABLE blob_chunks (
    account_id TEXT NOT NULL,
    blob_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK(ordinal>=0),
    data BLOB NOT NULL CHECK(length(data)<=262144),
    PRIMARY KEY(account_id,blob_id,ordinal),
    FOREIGN KEY(account_id,blob_id) REFERENCES blobs(account_id,id) ON DELETE CASCADE
);
CREATE TABLE collections (
    account_id TEXT NOT NULL REFERENCES accounts(id),
    id TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    name TEXT,
    kind TEXT NOT NULL DEFAULT 'unknown',
    retired INTEGER NOT NULL DEFAULT 0 CHECK(retired IN (0,1)),
    PRIMARY KEY(account_id,id),
    UNIQUE(account_id,provider_id)
);
CREATE TABLE messages (
    account_id TEXT NOT NULL REFERENCES accounts(id),
    id TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    thread_id TEXT,
    history_id TEXT,
    internal_date_ms INTEGER,
    subject TEXT,
    provider_json TEXT NOT NULL DEFAULT '{}',
    raw_blob_id TEXT,
    text_blob_id TEXT,
    html_blob_id TEXT,
    body_availability TEXT NOT NULL CHECK(body_availability IN ('available','missing','too_large','unparsed')),
    PRIMARY KEY(account_id,id),
    UNIQUE(account_id,provider_id),
    FOREIGN KEY(account_id,raw_blob_id) REFERENCES blobs(account_id,id),
    FOREIGN KEY(account_id,text_blob_id) REFERENCES blobs(account_id,id),
    FOREIGN KEY(account_id,html_blob_id) REFERENCES blobs(account_id,id)
);
CREATE INDEX messages_order ON messages(account_id,internal_date_ms DESC,id);
CREATE TABLE memberships (
    account_id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    collection_id TEXT NOT NULL,
    PRIMARY KEY(account_id,message_id,collection_id),
    FOREIGN KEY(account_id,message_id) REFERENCES messages(account_id,id) ON DELETE CASCADE,
    FOREIGN KEY(account_id,collection_id) REFERENCES collections(account_id,id) ON DELETE CASCADE
);
CREATE INDEX collection_members ON memberships(account_id,collection_id,message_id);
CREATE TABLE message_headers (
    account_id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK(ordinal>=0),
    name TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY(account_id,message_id,ordinal),
    FOREIGN KEY(account_id,message_id) REFERENCES messages(account_id,id) ON DELETE CASCADE
);
CREATE TABLE attachments (
    account_id TEXT NOT NULL,
    id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    part_index INTEGER NOT NULL CHECK(part_index>=0),
    filename TEXT,
    mime_type TEXT NOT NULL,
    content_id TEXT,
    blob_id TEXT NOT NULL,
    PRIMARY KEY(account_id,id),
    UNIQUE(account_id,message_id,part_index),
    FOREIGN KEY(account_id,message_id) REFERENCES messages(account_id,id) ON DELETE CASCADE,
    FOREIGN KEY(account_id,blob_id) REFERENCES blobs(account_id,id)
);
CREATE VIRTUAL TABLE message_search USING fts5(account_id UNINDEXED,message_id UNINDEXED,subject,body);

CREATE TABLE sync_runs (
    account_id TEXT NOT NULL REFERENCES accounts(id),
    id TEXT NOT NULL,
    scope TEXT NOT NULL,
    mode TEXT NOT NULL CHECK(mode IN ('full','delta','fetch')),
    state TEXT NOT NULL CHECK(state IN ('queued','running','succeeded','failed','cancelled')),
    started_at_ms INTEGER NOT NULL,
    finished_at_ms INTEGER,
    start_cursor TEXT,
    page_token TEXT,
    error_code TEXT,
    processed INTEGER NOT NULL DEFAULT 0 CHECK(processed>=0),
    PRIMARY KEY(account_id,id)
);
CREATE INDEX sync_runs_state ON sync_runs(account_id,scope,state);
CREATE TABLE sync_scopes (
    account_id TEXT NOT NULL REFERENCES accounts(id),
    scope TEXT NOT NULL,
    cursor_kind TEXT NOT NULL,
    cursor TEXT NOT NULL,
    query_fingerprint TEXT NOT NULL,
    active_generation TEXT NOT NULL,
    synchronized_at_ms INTEGER NOT NULL,
    PRIMARY KEY(account_id,scope),
    FOREIGN KEY(account_id,active_generation) REFERENCES sync_runs(account_id,id)
);
CREATE TABLE staged_collections (
    account_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    name TEXT,
    kind TEXT NOT NULL,
    PRIMARY KEY(account_id,run_id,provider_id),
    FOREIGN KEY(account_id,run_id) REFERENCES sync_runs(account_id,id) ON DELETE CASCADE
);
CREATE TABLE staged_messages (
    account_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    thread_id TEXT,
    history_id TEXT,
    internal_date_ms INTEGER,
    subject TEXT,
    provider_json TEXT NOT NULL DEFAULT '{}',
    raw_blob_id TEXT,
    text_blob_id TEXT,
    html_blob_id TEXT,
    search_text TEXT,
    body_availability TEXT NOT NULL CHECK(body_availability IN ('available','missing','too_large','unparsed')),
    deleted INTEGER NOT NULL DEFAULT 0 CHECK(deleted IN (0,1)),
    PRIMARY KEY(account_id,run_id,provider_id),
    FOREIGN KEY(account_id,run_id) REFERENCES sync_runs(account_id,id) ON DELETE CASCADE,
    FOREIGN KEY(account_id,raw_blob_id) REFERENCES blobs(account_id,id),
    FOREIGN KEY(account_id,text_blob_id) REFERENCES blobs(account_id,id),
    FOREIGN KEY(account_id,html_blob_id) REFERENCES blobs(account_id,id)
);
CREATE TABLE staged_headers (
    account_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL,
    name TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY(account_id,run_id,provider_id,ordinal),
    FOREIGN KEY(account_id,run_id,provider_id) REFERENCES staged_messages(account_id,run_id,provider_id) ON DELETE CASCADE
);
CREATE TABLE staged_memberships (
    account_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    label_id TEXT NOT NULL,
    PRIMARY KEY(account_id,run_id,provider_id,label_id),
    FOREIGN KEY(account_id,run_id,provider_id) REFERENCES staged_messages(account_id,run_id,provider_id) ON DELETE CASCADE
);
CREATE TABLE staged_attachments (
    account_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    part_index INTEGER NOT NULL,
    filename TEXT,
    mime_type TEXT NOT NULL,
    content_id TEXT,
    blob_id TEXT NOT NULL,
    PRIMARY KEY(account_id,run_id,provider_id,part_index),
    FOREIGN KEY(account_id,run_id,provider_id) REFERENCES staged_messages(account_id,run_id,provider_id) ON DELETE CASCADE,
    FOREIGN KEY(account_id,blob_id) REFERENCES blobs(account_id,id)
);
INSERT INTO schema_migrations VALUES (3);
PRAGMA user_version = 3;
