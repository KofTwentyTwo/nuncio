CREATE TABLE imap_trash_origins (
 account_id TEXT NOT NULL REFERENCES accounts(id),
 provider_id TEXT NOT NULL CHECK(length(provider_id) BETWEEN 1 AND 2048),
 origin_json TEXT NOT NULL CHECK(length(origin_json)<=8192 AND json_valid(origin_json)),
 PRIMARY KEY(account_id,provider_id)
);
INSERT INTO schema_migrations VALUES(16);
PRAGMA user_version=16;
