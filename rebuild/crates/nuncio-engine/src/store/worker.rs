use super::{
    accounts as lifecycle, migrations, AccountRecord, Reply, Request, StoreError, StoreOpenOptions,
    StoreStatus,
};
use fs4::fs_std::FileExt;
use rusqlite::{params, Connection, OpenFlags};
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::sync::mpsc;
use zeroize::Zeroizing;

pub(super) fn run(
    directory: PathBuf,
    key: Zeroizing<Vec<u8>>,
    mut receiver: mpsc::Receiver<Request>,
    ready: Reply<()>,
    changed: tokio::sync::watch::Sender<u64>,
    options: StoreOpenOptions,
) {
    let (mut connection, lock) = match open_with_options(&directory, &key, &options) {
        Ok(value) => value,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    drop(key);
    if let Ok(status) = status(&connection) {
        changed.send_replace(status.revision);
    }
    if ready.send(Ok(())).is_err() {
        return;
    }
    while let Some(request) = receiver.blocking_recv() {
        match request {
            Request::Execute(job) => job(&mut connection),
            Request::Status(reply) => {
                let _ = reply.send(status(&connection));
            }
            Request::AddAccount(account, reply) => {
                let _ = reply.send(add_account(&mut connection, account));
            }
            Request::Accounts(reply) => {
                let _ = reply.send(accounts(&connection));
            }
            Request::Account(id, reply) => {
                let _ = reply.send(lifecycle::get(&connection, &id));
            }
            Request::ConnectGoogle(account, reply) => {
                let _ = reply.send(lifecycle::connect(&mut connection, account));
            }
            Request::SetAccountState(id, state, reply) => {
                let _ = reply.send(lifecycle::set_state(&mut connection, &id, &state));
            }
            Request::PrepareCredential(reference, reply) => {
                let _ = reply.send(lifecycle::prepare_credential(&connection, &reference));
            }
            Request::CredentialCleanup(reply) => {
                let _ = reply.send(lifecycle::cleanup(&connection));
            }
            Request::FinishCredentialCleanup(reference, reply) => {
                let _ = reply.send(lifecycle::finish_cleanup(&connection, &reference));
            }
            Request::Close(reply) => {
                let result = connection
                    .close()
                    .map_err(|(_, error)| StoreError::from(error));
                drop(lock);
                let _ = reply.send(result);
                return;
            }
        }
        if let Ok(revision) = connection.query_row(
            "SELECT revision FROM store_meta WHERE singleton=1",
            [],
            |r| {
                let value: i64 = r.get(0)?;
                u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(0, value))
            },
        ) {
            changed.send_if_modified(|current| {
                if *current == revision {
                    false
                } else {
                    *current = revision;
                    true
                }
            });
        }
    }
}

pub(super) fn open(directory: &Path, key: &[u8]) -> Result<(Connection, File), StoreError> {
    open_with_options(directory, key, &StoreOpenOptions::default())
}

fn open_with_options(
    directory: &Path,
    key: &[u8],
    store_options: &StoreOpenOptions,
) -> Result<(Connection, File), StoreError> {
    private_directory(directory)?;
    let lock_path = directory.join("store.lock");
    regular_path(&lock_path)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = options.open(lock_path)?;
    if !lock.try_lock_exclusive()? {
        return Err(StoreError::Locked);
    }
    let path = directory.join("store.db");
    regular_path(&path)?;
    if !path.exists() {
        let mut create = OpenOptions::new();
        create.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            create.mode(0o600);
        }
        create.open(&path)?;
    }
    let mut connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    let raw_key = Zeroizing::new(format!("x'{}'", hex::encode(key)));
    connection.pragma_update(None, "key", raw_key.as_str())?;
    let cipher: String = connection
        .pragma_query_value(None, "cipher_version", |row| row.get(0))
        .map_err(|_| StoreError::CipherUnavailable)?;
    if cipher.is_empty() {
        return Err(StoreError::CipherUnavailable);
    }
    connection
        .query_row("SELECT count(*) FROM sqlite_master", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|_| StoreError::KeyOrCorrupt)?;
    migrations::check_version(&connection)?;
    connection.pragma_update(None, "cipher_memory_security", true)?;
    connection.pragma_update(None, "temp_store", "MEMORY")?;
    connection.pragma_update(None, "foreign_keys", true)?;
    connection.busy_timeout(Duration::from_secs(5))?;
    let journal: String =
        connection.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))?;
    if journal != "wal" {
        return Err(StoreError::Unavailable);
    }
    connection.pragma_update(None, "synchronous", "FULL")?;
    migrations::migrate(&mut connection, store_options)?;
    Ok((connection, lock))
}

pub(crate) fn private_directory(path: &Path) -> Result<(), StoreError> {
    if path.exists() {
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(StoreError::InvalidPath);
        }
    } else {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(path)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn regular_path(path: &Path) -> Result<(), StoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => {
            Err(StoreError::InvalidPath)
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn status(connection: &Connection) -> Result<StoreStatus, StoreError> {
    let account_count: i64 =
        connection.query_row("SELECT count(*) FROM accounts", [], |row| row.get(0))?;
    let revision: i64 = connection.query_row(
        "SELECT revision FROM store_meta WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    Ok(StoreStatus {
        schema_version: migrations::VERSION,
        account_count: account_count
            .try_into()
            .map_err(|_| StoreError::KeyOrCorrupt)?,
        revision: revision.try_into().map_err(|_| StoreError::KeyOrCorrupt)?,
    })
}

fn add_account(connection: &mut Connection, account: AccountRecord) -> Result<(), StoreError> {
    if uuid::Uuid::parse_str(&account.id).is_err()
        || !matches!(account.provider.as_str(), "google" | "imap")
        || account.address.is_empty()
        || account.address.len() > 320
        || account.address.chars().any(char::is_control)
    {
        return Err(StoreError::InvalidAccount);
    }
    let transaction = connection.transaction()?;
    transaction.execute(
        "INSERT INTO accounts(id, provider, address) VALUES (?1, ?2, ?3)",
        params![account.id, account.provider, account.address],
    )?;
    super::changes::record(&transaction, Some(&account.id), "account_added", None)?;
    super::schedules::ensure(&transaction, &account.id)?;
    transaction.commit()?;
    Ok(())
}

fn accounts(connection: &Connection) -> Result<Vec<AccountRecord>, StoreError> {
    let mut statement =
        connection.prepare("SELECT id, provider, address FROM accounts ORDER BY id")?;
    let rows = statement.query_map([], |row| {
        Ok(AccountRecord {
            id: row.get(0)?,
            provider: row.get(1)?,
            address: row.get(2)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(StoreError::from)
}
