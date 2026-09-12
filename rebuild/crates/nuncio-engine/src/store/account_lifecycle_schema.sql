ALTER TABLE accounts ADD COLUMN display_name TEXT NOT NULL DEFAULT '' CHECK(length(CAST(display_name AS BLOB))<=256);
ALTER TABLE accounts ADD COLUMN version INTEGER NOT NULL DEFAULT 1 CHECK(typeof(version)='integer' AND version>0);
ALTER TABLE accounts ADD COLUMN paused INTEGER NOT NULL DEFAULT 0 CHECK(paused IN (0,1));
ALTER TABLE accounts ADD COLUMN archived INTEGER NOT NULL DEFAULT 0 CHECK(archived IN (0,1));
INSERT INTO schema_migrations VALUES (23);
PRAGMA user_version=23;
