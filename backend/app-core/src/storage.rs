//! 事务式 JSON 状态存储。
//!
//! 每次提交先写同目录临时文件、`sync_all`，再以原子 rename 替换主快照；旧主快照保留
//! 为 `.bak` 恢复点。解析或迁移失败不会清空用户数据，而是以只读状态返回错误。

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::de::DeserializeOwned;
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("storage IO failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("state at {path} is invalid; store remains read-only: {reason}")]
    Corrupt { path: PathBuf, reason: String },
    #[error("store is read-only after a previous load or commit failure")]
    ReadOnly,
}

pub struct AtomicJsonStore<T> {
    path: PathBuf,
    state: Mutex<StoreState<T>>,
}

struct StoreState<T> {
    value: T,
    read_only: bool,
}

impl<T> AtomicJsonStore<T>
where
    T: Clone + Serialize + DeserializeOwned,
{
    pub fn open(path: impl Into<PathBuf>, default: T) -> Result<Self, StorageError> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| StorageError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let value = if path.exists() {
            read_json(&path)?
        } else {
            default
        };
        Ok(Self {
            path,
            state: Mutex::new(StoreState {
                value,
                read_only: false,
            }),
        })
    }

    pub fn snapshot(&self) -> T {
        self.state
            .lock()
            .expect("atomic store lock poisoned")
            .value
            .clone()
    }

    pub fn is_read_only(&self) -> bool {
        self.state
            .lock()
            .expect("atomic store lock poisoned")
            .read_only
    }

    /// 在内存副本上执行变更，只有落盘成功才替换活动状态。
    pub fn transact<R>(
        &self,
        update: impl FnOnce(&mut T) -> Result<R, StorageError>,
    ) -> Result<R, StorageError> {
        let mut state = self.state.lock().expect("atomic store lock poisoned");
        if state.read_only {
            return Err(StorageError::ReadOnly);
        }
        let mut candidate = state.value.clone();
        let result = update(&mut candidate)?;
        if let Err(error) = write_json_atomic(&self.path, &candidate) {
            state.read_only = true;
            return Err(error);
        }
        state.value = candidate;
        Ok(result)
    }
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, StorageError> {
    let mut file = File::open(path).map_err(|source| StorageError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|source| StorageError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    serde_json::from_slice(&bytes).map_err(|error| StorageError::Corrupt {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), StorageError> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| StorageError::Corrupt {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("json");
    let temporary = path.with_extension(format!("{extension}.tmp"));
    let backup = path.with_extension(format!("{extension}.bak"));

    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|source| StorageError::Io {
            path: temporary.clone(),
            source,
        })?;
    file.write_all(&bytes).map_err(|source| StorageError::Io {
        path: temporary.clone(),
        source,
    })?;
    file.sync_all().map_err(|source| StorageError::Io {
        path: temporary.clone(),
        source,
    })?;

    if path.exists() {
        // Backup is a recovery aid, not the commit point. A failure here leaves the live file.
        std::fs::copy(path, &backup).map_err(|source| StorageError::Io {
            path: backup,
            source,
        })?;
    }
    std::fs::rename(&temporary, path).map_err(|source| StorageError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if let Some(parent) = path.parent() {
        if let Ok(directory) = File::open(parent) {
            let _ = directory.sync_all();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    struct State {
        schema_version: u16,
        counter: u64,
    }

    #[test]
    fn transaction_is_persisted_and_reopened() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let store = AtomicJsonStore::open(
            &path,
            State {
                schema_version: 1,
                counter: 0,
            },
        )
        .unwrap();
        store
            .transact(|state| {
                state.counter = 7;
                Ok(())
            })
            .unwrap();
        let reopened = AtomicJsonStore::open(
            &path,
            State {
                schema_version: 1,
                counter: 0,
            },
        )
        .unwrap();
        assert_eq!(reopened.snapshot().counter, 7);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn failed_update_does_not_replace_state() {
        let dir = tempfile::tempdir().unwrap();
        let store = AtomicJsonStore::open(
            dir.path().join("state.json"),
            State {
                schema_version: 1,
                counter: 1,
            },
        )
        .unwrap();
        let result = store.transact(|state| {
            state.counter = 9;
            Err::<(), _>(StorageError::ReadOnly)
        });
        assert!(result.is_err());
        assert_eq!(store.snapshot().counter, 1);
    }

    #[test]
    fn corrupt_state_is_reported_without_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(&path, b"not-json").unwrap();
        let result = AtomicJsonStore::open(
            &path,
            State {
                schema_version: 1,
                counter: 0,
            },
        );
        assert!(matches!(result, Err(StorageError::Corrupt { .. })));
        assert_eq!(std::fs::read(&path).unwrap(), b"not-json");
    }
}
