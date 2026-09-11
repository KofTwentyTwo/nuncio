pub mod keyring;
#[cfg(feature = "test-harness")]
pub mod test_store;

use zeroize::Zeroizing;

#[derive(Debug, thiserror::Error)]
#[error("Secure credential storage is unavailable")]
pub struct SecretError;

pub trait SecretStore: Send + Sync {
    #[cfg(feature = "test-harness")]
    fn is_synthetic(&self) -> bool {
        false
    }
    fn get(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError>;
    fn put(&self, name: &str, value: &[u8]) -> Result<(), SecretError>;
    fn delete(&self, name: &str) -> Result<(), SecretError>;
}
