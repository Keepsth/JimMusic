//! 控制面写操作的持久幂等结果缓存。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use app_core::storage::{AtomicJsonStore, StorageError};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    fingerprint: String,
    response: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct State {
    schema_version: u16,
    entries: BTreeMap<String, Entry>,
}

#[derive(Debug, thiserror::Error)]
pub enum IdempotencyError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("idempotency key was already used with a different request")]
    Conflict,
    #[error("idempotency response serialization failed: {0}")]
    Serialization(String),
    #[error("operation failed: {0}")]
    Operation(String),
}

pub struct IdempotencyService {
    store: AtomicJsonStore<State>,
    operation_lock: Mutex<()>,
    http_operation_lock: tokio::sync::Mutex<()>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpReplay {
    pub status: u16,
    pub body: serde_json::Value,
}

impl IdempotencyService {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, IdempotencyError> {
        Ok(Self {
            store: AtomicJsonStore::open(
                path,
                State {
                    schema_version: 1,
                    entries: BTreeMap::new(),
                },
            )?,
            operation_lock: Mutex::new(()),
            http_operation_lock: tokio::sync::Mutex::new(()),
        })
    }

    pub async fn lock_http(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.http_operation_lock.lock().await
    }

    pub fn lookup_http(
        &self,
        scope: &str,
        request_id: &str,
        fingerprint: &str,
    ) -> Result<Option<HttpReplay>, IdempotencyError> {
        let key = format!("http\0{scope}\0{request_id}");
        let snapshot = self.store.snapshot();
        let Some(existing) = snapshot.entries.get(&key) else {
            return Ok(None);
        };
        if existing.fingerprint != fingerprint {
            return Err(IdempotencyError::Conflict);
        }
        serde_json::from_value(existing.response.clone())
            .map(Some)
            .map_err(|error| IdempotencyError::Serialization(error.to_string()))
    }

    pub fn store_http(
        &self,
        scope: &str,
        request_id: &str,
        fingerprint: &str,
        response: HttpReplay,
    ) -> Result<(), IdempotencyError> {
        let key = format!("http\0{scope}\0{request_id}");
        let response = serde_json::to_value(response)
            .map_err(|error| IdempotencyError::Serialization(error.to_string()))?;
        self.store.transact(|state| {
            state.entries.insert(
                key,
                Entry {
                    fingerprint: fingerprint.into(),
                    response,
                },
            );
            trim_entries(state);
            Ok(())
        })?;
        Ok(())
    }

    pub fn execute<T: Serialize + DeserializeOwned>(
        &self,
        scope: &str,
        request_id: &str,
        fingerprint: &str,
        operation: impl FnOnce() -> Result<T, String>,
    ) -> Result<(T, bool), IdempotencyError> {
        let _guard = self
            .operation_lock
            .lock()
            .expect("idempotency operation lock poisoned");
        let key = format!("{scope}\0{request_id}");
        if let Some(existing) = self.store.snapshot().entries.get(&key) {
            if existing.fingerprint != fingerprint {
                return Err(IdempotencyError::Conflict);
            }
            let value = serde_json::from_value(existing.response.clone())
                .map_err(|error| IdempotencyError::Serialization(error.to_string()))?;
            return Ok((value, true));
        }
        let value = operation().map_err(IdempotencyError::Operation)?;
        let response = serde_json::to_value(&value)
            .map_err(|error| IdempotencyError::Serialization(error.to_string()))?;
        self.store.transact(|state| {
            state.entries.insert(
                key,
                Entry {
                    fingerprint: fingerprint.into(),
                    response,
                },
            );
            trim_entries(state);
            Ok(())
        })?;
        Ok((value, false))
    }
}

fn trim_entries(state: &mut State) {
    if state.entries.len() > 20_000 {
        let remove = state.entries.len() - 20_000;
        let keys: Vec<_> = state.entries.keys().take(remove).cloned().collect();
        for key in keys {
            state.entries.remove(&key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_returns_original_response_and_rejects_key_reuse() {
        let dir = tempfile::tempdir().unwrap();
        let service = IdempotencyService::open(dir.path().join("keys.json")).unwrap();
        let (first, replay) = service
            .execute("publish", "request", "a", || Ok::<_, String>(7u64))
            .unwrap();
        assert_eq!(first, 7);
        assert!(!replay);
        let (second, replay) = service
            .execute("publish", "request", "a", || Ok::<_, String>(9u64))
            .unwrap();
        assert_eq!(second, 7);
        assert!(replay);
        assert!(matches!(
            service.execute("publish", "request", "b", || Ok::<_, String>(9u64)),
            Err(IdempotencyError::Conflict)
        ));
    }
}
