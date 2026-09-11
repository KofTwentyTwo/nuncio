CREATE TABLE sync_settings (
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    poll_interval_ms INTEGER NOT NULL CHECK(poll_interval_ms BETWEEN 10 AND 86400000)
);
INSERT INTO sync_settings VALUES(1,60000);
CREATE TABLE sync_schedules (
    account_id TEXT NOT NULL REFERENCES accounts(id),
    scope TEXT NOT NULL CHECK(scope IN ('gmail','calendar','imap')),
    last_run_id TEXT,
    last_success_at_ms INTEGER,
    next_attempt_at_ms INTEGER NOT NULL DEFAULT 0,
    provider_retry_after_ms INTEGER,
    consecutive_failures INTEGER NOT NULL DEFAULT 0 CHECK(consecutive_failures BETWEEN 0 AND 1024),
    error_code TEXT,
    PRIMARY KEY(account_id,scope),
    FOREIGN KEY(account_id,last_run_id) REFERENCES sync_runs(account_id,id)
);
INSERT INTO sync_schedules(account_id,scope)
SELECT a.id,s.scope FROM accounts a JOIN (SELECT 'gmail' AS scope UNION ALL SELECT 'calendar' UNION ALL SELECT 'imap') s
ON (a.provider='google' AND s.scope IN ('gmail','calendar')) OR (a.provider='imap' AND s.scope='imap');
INSERT INTO schema_migrations VALUES(6);
PRAGMA user_version=6;
