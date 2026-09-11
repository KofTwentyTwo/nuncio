CREATE TABLE calendars (
    account_id TEXT NOT NULL REFERENCES accounts(id),
    id TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    summary TEXT,
    time_zone TEXT,
    access_role TEXT NOT NULL,
    is_primary INTEGER NOT NULL DEFAULT 0 CHECK(is_primary IN (0,1)),
    retired INTEGER NOT NULL DEFAULT 0 CHECK(retired IN (0,1)),
    provider_json TEXT NOT NULL CHECK(json_valid(provider_json)),
    canonical_revision INTEGER NOT NULL DEFAULT 0 CHECK(canonical_revision>=0),
    PRIMARY KEY(account_id,id),
    UNIQUE(account_id,provider_id)
);
-- A generated occurrence can later acquire a canonical exception resource.
-- Its local identity remains stable across those representations and windows.
CREATE TABLE calendar_event_ids (
    account_id TEXT NOT NULL,
    calendar_id TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    id TEXT NOT NULL,
    PRIMARY KEY(account_id,calendar_id,provider_id),
    UNIQUE(account_id,calendar_id,id),
    FOREIGN KEY(account_id,calendar_id) REFERENCES calendars(account_id,id)
);
CREATE TABLE calendar_objects (
    account_id TEXT NOT NULL,
    calendar_id TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    object_json TEXT NOT NULL CHECK(json_valid(object_json)),
    status TEXT NOT NULL CHECK(status IN ('confirmed','tentative','cancelled')),
    etag TEXT,
    start_ms INTEGER,
    end_ms INTEGER,
    PRIMARY KEY(account_id,calendar_id,provider_id),
    FOREIGN KEY(account_id,calendar_id,provider_id) REFERENCES calendar_event_ids(account_id,calendar_id,provider_id)
);
CREATE TABLE calendar_occurrences (
    account_id TEXT NOT NULL,
    calendar_id TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    object_json TEXT NOT NULL CHECK(json_valid(object_json)),
    status TEXT NOT NULL CHECK(status IN ('confirmed','tentative','cancelled')),
    etag TEXT,
    start_ms INTEGER,
    end_ms INTEGER,
    PRIMARY KEY(account_id,calendar_id,provider_id),
    FOREIGN KEY(account_id,calendar_id,provider_id) REFERENCES calendar_event_ids(account_id,calendar_id,provider_id)
);
CREATE INDEX calendar_occurrence_time ON calendar_occurrences(account_id,calendar_id,start_ms,provider_id);
CREATE TABLE agenda_coverage (
    account_id TEXT NOT NULL,
    calendar_id TEXT NOT NULL,
    from_date TEXT NOT NULL,
    to_date TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('current','stale')),
    canonical_revision INTEGER NOT NULL CHECK(canonical_revision>=0),
    refreshed_at_ms INTEGER NOT NULL,
    generation TEXT NOT NULL,
    PRIMARY KEY(account_id,calendar_id),
    FOREIGN KEY(account_id,calendar_id) REFERENCES calendars(account_id,id),
    FOREIGN KEY(account_id,generation) REFERENCES sync_runs(account_id,id)
);
CREATE TABLE staged_calendar_catalog (
    account_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    summary TEXT,
    time_zone TEXT,
    access_role TEXT NOT NULL,
    is_primary INTEGER NOT NULL CHECK(is_primary IN (0,1)),
    provider_json TEXT NOT NULL CHECK(json_valid(provider_json)),
    PRIMARY KEY(account_id,run_id,provider_id),
    FOREIGN KEY(account_id,run_id) REFERENCES sync_runs(account_id,id) ON DELETE CASCADE
);
CREATE TABLE staged_calendar_events (
    account_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    calendar_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('canonical','occurrence')),
    provider_id TEXT NOT NULL,
    object_json TEXT NOT NULL CHECK(json_valid(object_json)),
    status TEXT NOT NULL CHECK(status IN ('confirmed','tentative','cancelled')),
    etag TEXT,
    start_ms INTEGER,
    end_ms INTEGER,
    PRIMARY KEY(account_id,run_id,calendar_id,kind,provider_id),
    FOREIGN KEY(account_id,run_id) REFERENCES sync_runs(account_id,id) ON DELETE CASCADE,
    FOREIGN KEY(account_id,calendar_id) REFERENCES calendars(account_id,id)
);
INSERT INTO schema_migrations VALUES (4);
PRAGMA user_version = 4;
