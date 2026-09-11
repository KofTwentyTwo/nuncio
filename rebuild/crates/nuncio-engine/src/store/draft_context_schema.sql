ALTER TABLE drafts ADD COLUMN context_json TEXT CHECK(context_json IS NULL OR length(CAST(context_json AS BLOB))<=65536);
ALTER TABLE draft_attachments ADD COLUMN parameters_json TEXT NOT NULL DEFAULT '{}' CHECK(length(CAST(parameters_json AS BLOB))<=65536);
ALTER TABLE draft_uploads ADD COLUMN parameters_json TEXT NOT NULL DEFAULT '{}' CHECK(length(CAST(parameters_json AS BLOB))<=65536);
INSERT INTO schema_migrations VALUES (9);
PRAGMA user_version=9;
