use rusqlite::{params, types::Value, Connection};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};

pub const ACCOUNT: &str = "21266fe5-c38f-48c8-82b5-6b85ea54f11c";
pub const IMAP: &str = "d692c4b5-0dd3-4e7b-83f1-8b1c4b46d250";
pub const DRAFT: &str = "ef719f36-d6df-4326-9d47-02936b02a643";
pub const OPERATION: &str = "d54e097d-705e-4704-8584-bc2679a1b971";
pub const REQUEST: &str = "9342e6b1-bbbf-4016-8585-18ef7c6e8599";
pub const MAILBOX: &str = "898c8753-451b-48ed-a3a0-f9437c22a57b";
pub const KEY: [u8; 32] = [0x54; 32];
pub const PDF: &[u8] = include_bytes!("../../../../tests/fixtures/recovery/queued-draft.pdf");
pub const WIRE: &[u8] = include_bytes!("../../../../tests/fixtures/migrations/legacy-message.eml");

#[derive(Deserialize)]
pub struct History {
    pub last_schema: u32,
    pub migrations: Vec<Migration>,
}
#[derive(Deserialize)]
pub struct Migration {
    pub version: u32,
    pub sha256: String,
    pub sql: String,
}
pub fn history() -> History {
    let history: History = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/migrations/schema-history-through-22.json"
    ))
    .unwrap();
    assert_eq!(history.last_schema, 22);
    assert_eq!(history.migrations.len(), 22);
    for (ordinal, m) in history.migrations.iter().enumerate() {
        assert_eq!(m.version as usize, ordinal + 1);
        assert_eq!(hex::encode(Sha256::digest(m.sql.as_bytes())), m.sha256);
    }
    history
}
pub fn opened(path: &Path) -> Connection {
    let c = Connection::open(path).unwrap();
    c.pragma_update(None, "key", format!("x'{}'", hex::encode(KEY)))
        .unwrap();
    c.pragma_update(None, "foreign_keys", true).unwrap();
    c
}
pub fn fixture(path: &Path, version: u32) {
    let c = opened(path);
    for m in history().migrations.iter().take(version as usize) {
        c.execute_batch(&m.sql).unwrap();
    }
    seed(&c, version);
    assert_eq!(
        c.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        version
    );
    assert_eq!(
        c.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    assert!(c
        .prepare("PRAGMA foreign_key_check")
        .unwrap()
        .query([])
        .unwrap()
        .next()
        .unwrap()
        .is_none());
    c.close().unwrap();
}
pub fn backup_fixture(path: &Path, version: u32, passphrase: &str) -> Snapshot {
    fixture(path, version);
    let c = opened(path);
    if version >= 2 {
        c.execute(
            "UPDATE accounts SET state='connected',credential_ref='legacy-reference' WHERE id=?1",
            [ACCOUNT],
        )
        .unwrap();
        c.execute(
            "INSERT INTO credential_cleanup VALUES('legacy-abandoned-reference')",
            [],
        )
        .unwrap();
    }
    c.execute_batch("PRAGMA application_id=0x4e554e42; CREATE TABLE nuncio_backup_metadata(singleton INTEGER PRIMARY KEY CHECK(singleton=1),format_version INTEGER NOT NULL,schema_version INTEGER NOT NULL,created_at_ms INTEGER NOT NULL,revision INTEGER NOT NULL);").unwrap();
    c.execute(
        "INSERT INTO nuncio_backup_metadata VALUES(1,1,?1,1000,41)",
        [version],
    )
    .unwrap();
    let before = snapshot(&c);
    c.pragma_update(None, "rekey", passphrase).unwrap();
    c.close().unwrap();
    before
}
fn seed(c: &Connection, version: u32) {
    for (account, provider) in [(ACCOUNT, "google"), (IMAP, "imap")] {
        c.execute(
            "INSERT INTO accounts(id,provider,address) VALUES(?1,?2,'legacy@example.test')",
            params![account, provider],
        )
        .unwrap();
        if version >= 3 {
            for (id, bytes) in [("pdf", PDF), ("wire", WIRE)] {
                c.execute(
                    "INSERT INTO blobs VALUES(?1,?2,?3,?4)",
                    params![
                        account,
                        id,
                        hex::encode(Sha256::digest(bytes)),
                        bytes.len() as i64
                    ],
                )
                .unwrap();
                c.execute(
                    "INSERT INTO blob_chunks VALUES(?1,?2,0,?3)",
                    params![account, id, bytes],
                )
                .unwrap();
            }
            c.execute("INSERT INTO collections(account_id,id,provider_id,name,kind) VALUES(?1,?2,'INBOX','INBOX','system')",params![account,MAILBOX]).unwrap();
            c.execute("INSERT INTO messages(account_id,id,provider_id,subject,raw_blob_id,body_availability) VALUES(?1,'message','legacy-message','Historical mail','wire','available')",[account]).unwrap();
            c.execute(
                "INSERT INTO memberships VALUES(?1,'message',?2)",
                params![account, MAILBOX],
            )
            .unwrap();
            c.execute(
                "INSERT INTO message_headers VALUES(?1,'message',0,'Subject','Historical mail')",
                [account],
            )
            .unwrap();
            c.execute("INSERT INTO message_search(account_id,message_id,subject,body) VALUES(?1,'message','Historical mail','Original historical body')",[account]).unwrap();
            c.execute("INSERT INTO attachments(account_id,id,message_id,part_index,mime_type,filename,blob_id) VALUES(?1,'attachment','message',2,'application/pdf','legacy.pdf','pdf')",[account]).unwrap();
        }
        if version >= 7 {
            let content = json!({"to":[{"address":"recipient@example.test"}],"subject":"Historical mail","text":"Original historical body."});
            c.execute("INSERT INTO drafts(account_id,id,version,subject,content_json,created_at_ms,updated_at_ms) VALUES(?1,?2,1,'Historical mail',?3,1000,1000)",params![account,DRAFT,content.to_string()]).unwrap();
            c.execute("INSERT INTO draft_attachments(account_id,draft_id,id,position,blob_id,filename,mime_type,disposition) VALUES(?1,?2,'attachment',0,'pdf','legacy.pdf','application/pdf','attachment')",params![account,DRAFT]).unwrap();
        }
        if version >= 10 {
            let canonical =
                json!({"version":1,"kind":"send","draft_id":DRAFT,"expected_version":null})
                    .to_string();
            let desired=json!({"draft_id":DRAFT,"draft_version":1,"submission":"send","sender":"legacy@example.test","recipients":["recipient@example.test"],"message_id":"<legacy@example.test>","thread_id":null}).to_string();
            c.execute("INSERT INTO operations(account_id,id,request_id,kind,resource_id,fingerprint,request_json,desired_json,state,created_at_ms,updated_at_ms) VALUES(?1,?2,?3,'send',?4,?5,?6,?7,'queued',1000,1000)",params![account,OPERATION,REQUEST,DRAFT,hex::encode(Sha256::digest(canonical.as_bytes())),canonical,desired]).unwrap();
            c.execute("INSERT INTO send_payloads(account_id,operation_id,draft_id,draft_version,sender,recipients_json,message_id,wire_blob_id,sent_blob_id) VALUES(?1,?2,?3,1,'legacy@example.test','[\"recipient@example.test\"]','<legacy@example.test>','wire','wire')",params![account,OPERATION,DRAFT]).unwrap();
        }
    }
    if version >= 4 {
        let object=nuncio_engine::domain::calendar::CalendarObject::from_google(json!({"id":"legacy-event","status":"confirmed","summary":"Historical event","start":{"date":"2026-10-01"},"end":{"date":"2026-10-02"}}),"UTC").unwrap();
        c.execute("INSERT INTO calendars(account_id,id,provider_id,time_zone,access_role,provider_json) VALUES(?1,'calendar','primary','UTC','owner','{}')",[ACCOUNT]).unwrap();
        c.execute(
            "INSERT INTO calendar_event_ids VALUES(?1,'calendar','legacy-event','event')",
            [ACCOUNT],
        )
        .unwrap();
        for table in ["calendar_objects", "calendar_occurrences"] {
            c.execute(&format!("INSERT INTO {table}(account_id,calendar_id,provider_id,object_json,status,start_ms,end_ms) VALUES(?1,'calendar','legacy-event',?2,'confirmed',?3,?4)"),params![ACCOUNT,serde_json::to_string(&object).unwrap(),object.start_ms,object.end_ms]).unwrap();
        }
    }
    if version >= 13 {
        let config = json!({"address":"legacy@example.test","imap":{"host":"127.0.0.1","port":993,"tls":"implicit","username":"legacy@example.test"},"smtp":{"host":"127.0.0.1","port":587,"tls":"start_tls","username":"legacy@example.test"},"sent_policy":"client_append","sent_folder":"Sent","archive_folder":"Archive","trash_folder":"Trash"});
        let caps = json!({"move_messages":true,"uidplus":true,"condstore":true,"qresync":false,"idle":true,"smtp_utf8":false,"eight_bit_mime":true});
        c.execute(
            "INSERT INTO imap_accounts VALUES(?1,?2,?3)",
            params![IMAP, config.to_string(), caps.to_string()],
        )
        .unwrap();
    }
    if version >= 14 {
        let mailbox = json!({"name":"INBOX","delimiter":"/","attributes":[],"uid_validity":9001,"uid_next":2,"highest_mod_seq":10});
        c.execute(
            "INSERT INTO imap_mailboxes VALUES(?1,?2,'INBOX',?3,0)",
            params![IMAP, MAILBOX, mailbox.to_string()],
        )
        .unwrap();
        c.execute(
            "INSERT INTO imap_placements VALUES(?1,'message',?2,9001,1,'[\"\\\\Seen\"]')",
            params![IMAP, MAILBOX],
        )
        .unwrap();
    }
    if version >= 17 {
        let intent = json!({"account_id":IMAP,"endpoint":{"host":"127.0.0.1","port":587,"tls":"start_tls","username":"legacy@example.test"},"sent_policy":"client_append","mailbox_id":MAILBOX,"mailbox":"INBOX","uid_validity":9001,"uid_next":2});
        c.execute("INSERT INTO smtp_submissions(account_id,operation_id,intent_json,step) VALUES(?1,?2,?3,'prepared')",params![IMAP,OPERATION,intent.to_string()]).unwrap();
    }
    c.execute("UPDATE store_meta SET revision=41 WHERE singleton=1", [])
        .unwrap();
    c.execute(
        "INSERT INTO change_log(revision,kind,account_id) VALUES(41,'legacy-fixture',?1)",
        [ACCOUNT],
    )
    .unwrap();
}
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
}
pub type Snapshot = BTreeMap<String, Table>;
pub fn snapshot(c: &Connection) -> Snapshot {
    let names = c.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name NOT LIKE 'message_search_%' AND name<>'schema_migrations' ORDER BY name").unwrap().query_map([],|r|r.get::<_,String>(0)).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
    names
        .into_iter()
        .map(|name| {
            let columns = c
                .prepare(&format!("SELECT * FROM {} LIMIT 0", quoted(&name)))
                .unwrap()
                .column_names()
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>();
            let rows = rows(c, &name, &columns);
            (name, Table { columns, rows })
        })
        .collect()
}
pub fn rows(c: &Connection, name: &str, columns: &[String]) -> Vec<Vec<Value>> {
    let fields = columns
        .iter()
        .map(|c| quoted(c))
        .collect::<Vec<_>>()
        .join(",");
    c.prepare(&format!(
        "SELECT {fields} FROM {} ORDER BY {fields}",
        quoted(name)
    ))
    .unwrap()
    .query_map([], |r| {
        (0..columns.len())
            .map(|i| r.get::<_, Value>(i))
            .collect::<Result<Vec<_>, _>>()
    })
    .unwrap()
    .collect::<Result<Vec<_>, _>>()
    .unwrap()
}
fn quoted(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}
