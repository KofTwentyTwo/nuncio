CREATE TABLE imap_accounts (
    account_id TEXT PRIMARY KEY NOT NULL REFERENCES accounts(id),
    config TEXT NOT NULL CHECK(json_valid(config)),
    capabilities TEXT NOT NULL CHECK(json_valid(capabilities))
);
INSERT INTO schema_migrations VALUES (13);
PRAGMA user_version = 13;
