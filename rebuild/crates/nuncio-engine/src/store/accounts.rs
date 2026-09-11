use super::{AccountRecord, ConnectedAccount, StoreError, StoredAccount};
use rusqlite::{params, Connection, OptionalExtension};

pub(super) fn get(connection: &Connection, id: &str) -> Result<Option<StoredAccount>, StoreError> {
    connection
        .query_row(
            "SELECT id,provider,address,subject,state,credential_ref FROM accounts WHERE id=?1",
            [id],
            |row| {
                Ok(StoredAccount {
                    account: AccountRecord {
                        id: row.get(0)?,
                        provider: row.get(1)?,
                        address: row.get(2)?,
                    },
                    subject: row.get(3)?,
                    state: row.get(4)?,
                    credential_ref: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(StoreError::from)
}

pub(super) fn prepare_credential(
    connection: &Connection,
    reference: &str,
) -> Result<(), StoreError> {
    if reference.is_empty() || reference.len() > 512 || reference.chars().any(char::is_control) {
        return Err(StoreError::InvalidAccount);
    }
    connection.execute(
        "INSERT INTO credential_cleanup(reference) VALUES (?1)",
        [reference],
    )?;
    Ok(())
}
pub(super) fn cleanup(connection: &Connection) -> Result<Vec<String>, StoreError> {
    let mut statement =
        connection.prepare("SELECT reference FROM credential_cleanup ORDER BY reference")?;
    let rows = statement.query_map([], |row| row.get(0))?;
    rows.collect::<Result<_, _>>().map_err(StoreError::from)
}
pub(super) fn finish_cleanup(connection: &Connection, reference: &str) -> Result<(), StoreError> {
    connection.execute(
        "DELETE FROM credential_cleanup WHERE reference=?1",
        [reference],
    )?;
    Ok(())
}
pub(super) fn connect(
    connection: &mut Connection,
    account: ConnectedAccount,
) -> Result<(), StoreError> {
    connect_provider(connection, account, None)
}
pub(super) fn connect_provider(
    connection: &mut Connection,
    account: ConnectedAccount,
    imap: Option<(
        crate::domain::imap_account::ImapAccountConfig,
        crate::domain::imap_account::ImapCapabilities,
    )>,
) -> Result<(), StoreError> {
    let provider = if imap.is_some() { "imap" } else { "google" };
    if uuid::Uuid::parse_str(&account.id).is_err()
        || account.subject.is_empty()
        || account.subject.len() > 255
        || !account.subject.bytes().all(|b| b.is_ascii_graphic())
        || account.address.is_empty()
        || account.address.len() > 320
        || account.address.chars().any(char::is_control)
    {
        return Err(StoreError::InvalidAccount);
    }
    let transaction = connection.transaction()?;
    let prepared: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM credential_cleanup WHERE reference=?1)",
        [&account.credential_ref],
        |row| row.get(0),
    )?;
    if !prepared {
        return Err(StoreError::InvalidAccount);
    }
    let old = get(&transaction, &account.id)?;
    if let Some(old) = &old {
        if old.account.provider != provider || old.subject.as_deref() != Some(&account.subject) {
            return Err(StoreError::InvalidAccount);
        }
        if let Some(reference) = &old.credential_ref {
            transaction.execute(
                "INSERT OR IGNORE INTO credential_cleanup(reference) VALUES (?1)",
                [reference],
            )?;
        }
    }
    transaction.execute("INSERT INTO accounts(id,provider,address,subject,state,credential_ref) VALUES (?1,?5,?2,?3,'connected',?4)
        ON CONFLICT(id) DO UPDATE SET address=excluded.address,state='connected',credential_ref=excluded.credential_ref",
        params![account.id,account.address,account.subject,account.credential_ref,provider])?;
    if let Some((config, capabilities)) = imap {
        transaction.execute("INSERT INTO imap_accounts(account_id,config,capabilities) VALUES(?1,?2,?3) ON CONFLICT(account_id) DO UPDATE SET config=excluded.config,capabilities=excluded.capabilities",params![account.id,serde_json::to_string(&config).map_err(|_|StoreError::InvalidAccount)?,serde_json::to_string(&capabilities).map_err(|_|StoreError::InvalidAccount)?])?;
    }
    transaction.execute(
        "DELETE FROM credential_cleanup WHERE reference=?1",
        [&account.credential_ref],
    )?;
    change(&transaction, &account.id, "account_connected")?;
    super::schedules::ensure(&transaction, &account.id)?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn set_state(
    connection: &mut Connection,
    id: &str,
    state: &str,
) -> Result<(), StoreError> {
    if !matches!(state, "disconnected" | "needs_auth") {
        return Err(StoreError::InvalidAccount);
    }
    let transaction = connection.transaction()?;
    let old = get(&transaction, id)?.ok_or(StoreError::InvalidAccount)?;
    if old.state == state && old.credential_ref.is_none() {
        return Ok(());
    }
    if let Some(reference) = old.credential_ref {
        transaction.execute(
            "INSERT OR IGNORE INTO credential_cleanup(reference) VALUES (?1)",
            [reference],
        )?;
    }
    transaction.execute(
        "UPDATE accounts SET state=?1,credential_ref=NULL WHERE id=?2",
        params![state, id],
    )?;
    change(&transaction, id, "account_paused")?;
    transaction.commit()?;
    Ok(())
}

fn change(connection: &Connection, id: &str, kind: &str) -> Result<(), StoreError> {
    super::changes::record(connection, Some(id), kind, None)
}
