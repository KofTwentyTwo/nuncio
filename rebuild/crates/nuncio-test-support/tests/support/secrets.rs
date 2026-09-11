use nuncio_engine::secrets::{test_store::FileTestStore, SecretError, SecretStore};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};
use zeroize::Zeroizing;

pub struct TestSecrets {
    inner: FileTestStore,
    pub fail_put: AtomicBool,
    pub fail_delete: AtomicBool,
}
impl TestSecrets {
    pub fn new(path: PathBuf) -> Self {
        Self {
            inner: FileTestStore::new(path),
            fail_put: AtomicBool::new(false),
            fail_delete: AtomicBool::new(false),
        }
    }
}
impl SecretStore for TestSecrets {
    fn is_synthetic(&self) -> bool {
        true
    }
    fn get(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError> {
        self.inner.get(name)
    }
    fn put(&self, name: &str, value: &[u8]) -> Result<(), SecretError> {
        if name.contains("/account/") && self.fail_put.swap(false, Ordering::SeqCst) {
            return Err(SecretError);
        }
        self.inner.put(name, value)
    }
    fn delete(&self, name: &str) -> Result<(), SecretError> {
        if name.contains("/account/") && self.fail_delete.swap(false, Ordering::SeqCst) {
            return Err(SecretError);
        }
        self.inner.delete(name)
    }
}
