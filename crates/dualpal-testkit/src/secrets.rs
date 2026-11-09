//! Secret stores for tests.

pub use dualpal_core::connections::MemorySecrets;

use dualpal_core::connections::SecretStore;
use dualpal_core::{Error, Result};

/// A secret store where every call fails, to prove that a failure surfaces
/// instead of falling back to plaintext (CON-008).
#[derive(Default)]
pub struct FailingSecrets;

impl SecretStore for FailingSecrets {
    fn get(&self, _key: &str) -> Result<Option<String>> {
        Err(Error::Invalid("the secret store is unavailable".into()))
    }

    fn set(&self, _key: &str, _secret: &str) -> Result<()> {
        Err(Error::Invalid("the secret store is unavailable".into()))
    }

    fn delete(&self, _key: &str) -> Result<()> {
        Err(Error::Invalid("the secret store is unavailable".into()))
    }
}
