//! Refresh-token storage. `KeyringTokenStore` is the real backend (OS
//! keychain via the `keyring` crate: Secret Service on Linux, Keychain on
//! macOS, Credential Manager on Windows). The trait seam exists so a
//! headless backend (encrypted file / stronghold) can be swapped in later.

use std::collections::HashMap;
use std::sync::Mutex;

pub const KEYRING_SERVICE: &str = "com.minatolabs.spite";

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{0}")]
    Backend(String),
}

pub trait TokenStore: Send + Sync {
    fn save_refresh_token(&self, upn: &str, token: &str) -> Result<(), StoreError>;
    fn load_refresh_token(&self, upn: &str) -> Result<Option<String>, StoreError>;
    fn delete_refresh_token(&self, upn: &str) -> Result<(), StoreError>;
}

#[derive(Default)]
pub struct KeyringTokenStore;

impl KeyringTokenStore {
    fn entry(upn: &str) -> Result<keyring::Entry, StoreError> {
        keyring::Entry::new(KEYRING_SERVICE, upn).map_err(|e| StoreError::Backend(e.to_string()))
    }
}

impl TokenStore for KeyringTokenStore {
    fn save_refresh_token(&self, upn: &str, token: &str) -> Result<(), StoreError> {
        Self::entry(upn)?
            .set_password(token)
            .map_err(|e| StoreError::Backend(e.to_string()))
    }

    fn load_refresh_token(&self, upn: &str) -> Result<Option<String>, StoreError> {
        match Self::entry(upn)?.get_password() {
            Ok(token) => Ok(Some(token)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(StoreError::Backend(e.to_string())),
        }
    }

    fn delete_refresh_token(&self, upn: &str) -> Result<(), StoreError> {
        match Self::entry(upn)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(StoreError::Backend(e.to_string())),
        }
    }
}

/// In-memory store for tests.
#[derive(Default)]
pub struct MemoryTokenStore(Mutex<HashMap<String, String>>);

impl TokenStore for MemoryTokenStore {
    fn save_refresh_token(&self, upn: &str, token: &str) -> Result<(), StoreError> {
        self.0
            .lock()
            .unwrap()
            .insert(upn.to_string(), token.to_string());
        Ok(())
    }

    fn load_refresh_token(&self, upn: &str) -> Result<Option<String>, StoreError> {
        Ok(self.0.lock().unwrap().get(upn).cloned())
    }

    fn delete_refresh_token(&self, upn: &str) -> Result<(), StoreError> {
        self.0.lock().unwrap().remove(upn);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_rotation() {
        let store = MemoryTokenStore::default();
        let upn = "user@example.com";
        assert_eq!(store.load_refresh_token(upn).unwrap(), None);

        store.save_refresh_token(upn, "rt-1").unwrap();
        assert_eq!(
            store.load_refresh_token(upn).unwrap().as_deref(),
            Some("rt-1")
        );

        // Rotation: a newer token replaces the stored one.
        store.save_refresh_token(upn, "rt-2").unwrap();
        assert_eq!(
            store.load_refresh_token(upn).unwrap().as_deref(),
            Some("rt-2")
        );

        store.delete_refresh_token(upn).unwrap();
        assert_eq!(store.load_refresh_token(upn).unwrap(), None);
        // Deleting a missing entry is not an error.
        store.delete_refresh_token(upn).unwrap();
    }
}
