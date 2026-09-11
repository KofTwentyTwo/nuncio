ALTER TABLE smtp_submissions ADD COLUMN sent_floor INTEGER CHECK(sent_floor IS NULL OR sent_floor BETWEEN 1 AND 4294967295);
INSERT INTO schema_migrations VALUES(18);
PRAGMA user_version=18;
