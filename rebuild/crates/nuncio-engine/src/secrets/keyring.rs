use super::{SecretError, SecretStore};
use zeroize::Zeroizing;

pub struct OsKeyring;

impl SecretStore for OsKeyring {
    fn get(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError> {
        let entry = keyring::Entry::new("mx.nuncio.rebuild", name).map_err(|_| SecretError)?;
        match entry.get_password() {
            Ok(password) => {
                let password = Zeroizing::new(password);
                hex::decode(password.as_str())
                    .map(Zeroizing::new)
                    .map(Some)
                    .map_err(|_| SecretError)
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(SecretError),
        }
    }

    fn put(&self, name: &str, value: &[u8]) -> Result<(), SecretError> {
        let entry = keyring::Entry::new("mx.nuncio.rebuild", name).map_err(|_| SecretError)?;
        let password = Zeroizing::new(hex::encode(value));
        entry
            .set_password(password.as_str())
            .map_err(|_| SecretError)
    }

    fn delete(&self, name: &str) -> Result<(), SecretError> {
        let entry = keyring::Entry::new("mx.nuncio.rebuild", name).map_err(|_| SecretError)?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(SecretError),
        }
    }
}
