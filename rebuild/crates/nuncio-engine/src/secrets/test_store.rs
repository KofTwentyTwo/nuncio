use super::{SecretError, SecretStore};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Write},
    path::PathBuf,
    sync::Mutex,
};
use zeroize::Zeroizing;

/// Synthetic credentials for one isolated subprocess-test profile, never a production keystore.
pub struct FileTestStore {
    path: PathBuf,
    gate: Mutex<()>,
}

impl FileTestStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            gate: Mutex::new(()),
        }
    }

    fn read(&self) -> Result<BTreeMap<String, String>, SecretError> {
        match File::open(&self.path) {
            Ok(file) => {
                let mut bytes = Zeroizing::new(Vec::new());
                file.take(1_048_577)
                    .read_to_end(&mut bytes)
                    .map_err(|_| SecretError)?;
                if bytes.len() > 1_048_576 {
                    return Err(SecretError);
                }
                serde_json::from_slice(&bytes).map_err(|_| SecretError)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(_) => Err(SecretError),
        }
    }

    fn write(&self, values: &BTreeMap<String, String>) -> Result<(), SecretError> {
        let parent = self.path.parent().ok_or(SecretError)?;
        let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|_| SecretError)?;
        let bytes = Zeroizing::new(serde_json::to_vec(values).map_err(|_| SecretError)?);
        file.write_all(&bytes).map_err(|_| SecretError)?;
        file.as_file().sync_all().map_err(|_| SecretError)?;
        file.persist(&self.path).map_err(|_| SecretError)?;
        Ok(())
    }
}

impl SecretStore for FileTestStore {
    fn is_synthetic(&self) -> bool {
        true
    }
    fn get(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError> {
        let _guard = self.gate.lock().map_err(|_| SecretError)?;
        self.read()?
            .get(name)
            .map(|value| {
                hex::decode(value)
                    .map(Zeroizing::new)
                    .map_err(|_| SecretError)
            })
            .transpose()
    }
    fn put(&self, name: &str, value: &[u8]) -> Result<(), SecretError> {
        let _guard = self.gate.lock().map_err(|_| SecretError)?;
        let mut values = self.read()?;
        values.insert(name.into(), hex::encode(value));
        self.write(&values)
    }
    fn delete(&self, name: &str) -> Result<(), SecretError> {
        let _guard = self.gate.lock().map_err(|_| SecretError)?;
        let mut values = self.read()?;
        values.remove(name);
        self.write(&values)
    }
}
