CREATE TABLE imap_mailboxes (
 account_id TEXT NOT NULL,
 id TEXT NOT NULL,
 name TEXT NOT NULL,
 state_json TEXT NOT NULL CHECK(json_valid(state_json)),
 retired INTEGER NOT NULL CHECK(retired IN (0,1)),
 PRIMARY KEY(account_id,id),
 FOREIGN KEY(account_id,id) REFERENCES collections(account_id,id)
);
CREATE UNIQUE INDEX imap_active_mailbox_name ON imap_mailboxes(account_id,name) WHERE retired=0;
CREATE TABLE staged_imap_mailboxes (
 account_id TEXT NOT NULL,
 run_id TEXT NOT NULL,
 id TEXT NOT NULL,
 name TEXT NOT NULL,
 state_json TEXT NOT NULL CHECK(json_valid(state_json)),
 PRIMARY KEY(account_id,run_id,id),
 UNIQUE(account_id,run_id,name),
 FOREIGN KEY(account_id,run_id) REFERENCES sync_runs(account_id,id) ON DELETE CASCADE
);
CREATE TABLE imap_placements (
 account_id TEXT NOT NULL,
 message_id TEXT NOT NULL,
 mailbox_id TEXT NOT NULL,
 uid_validity INTEGER NOT NULL CHECK(uid_validity BETWEEN 1 AND 4294967295),
 uid INTEGER NOT NULL CHECK(uid BETWEEN 1 AND 4294967295),
 flags_json TEXT NOT NULL CHECK(json_valid(flags_json)),
 PRIMARY KEY(account_id,message_id),
 UNIQUE(account_id,mailbox_id,uid_validity,uid),
 FOREIGN KEY(account_id,message_id) REFERENCES messages(account_id,id) ON DELETE CASCADE,
 FOREIGN KEY(account_id,mailbox_id) REFERENCES imap_mailboxes(account_id,id)
);
INSERT INTO schema_migrations VALUES(14);
PRAGMA user_version=14;
