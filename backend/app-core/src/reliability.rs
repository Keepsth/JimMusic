//! Privacy-preserving local session reliability ledger.
//!
//! A session marker is committed at startup and cleared only during graceful
//! shutdown. The next startup counts a leftover marker as an unclean session.
//! Only aggregate counts are exposed; no media, identity, or network data is
//! collected or uploaded.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::storage::{AtomicJsonStore, StorageError};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ActiveSession {
    id: String,
    started_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReliabilityState {
    schema_version: u16,
    total_sessions: u64,
    clean_sessions: u64,
    unclean_sessions: u64,
    #[serde(default)]
    active_session: Option<ActiveSession>,
    #[serde(default)]
    last_clean_shutdown_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ReliabilityReport {
    pub total_sessions: u64,
    pub clean_sessions: u64,
    pub unclean_sessions: u64,
    pub current_session_active: bool,
    pub crash_free_session_rate: Option<f64>,
    pub last_clean_shutdown_at: Option<i64>,
    pub local_only: bool,
}

pub struct ReliabilityService {
    store: AtomicJsonStore<ReliabilityState>,
    session_id: String,
}

impl ReliabilityService {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, StorageError> {
        let path = path.into();
        let store = AtomicJsonStore::open(
            &path,
            ReliabilityState {
                schema_version: 1,
                total_sessions: 0,
                clean_sessions: 0,
                unclean_sessions: 0,
                active_session: None,
                last_clean_shutdown_at: None,
            },
        )?;
        // NFR-014/API-007：拒绝更新版本写入的状态（降级保护），保留原文件。
        crate::storage::reject_future_schema_version(store.snapshot().schema_version, 1, &path)?;
        let session_id = new_session_id()?;
        store.transact(|state| {
            if state.active_session.take().is_some() {
                state.unclean_sessions = state.unclean_sessions.saturating_add(1);
            }
            state.total_sessions = state.total_sessions.saturating_add(1);
            state.active_session = Some(ActiveSession {
                id: session_id.clone(),
                started_at: now(),
            });
            Ok(())
        })?;
        Ok(Self { store, session_id })
    }

    pub fn finish_clean(&self) -> Result<(), StorageError> {
        self.store.transact(|state| {
            if state
                .active_session
                .as_ref()
                .is_some_and(|session| session.id == self.session_id)
            {
                state.active_session = None;
                state.clean_sessions = state.clean_sessions.saturating_add(1);
                state.last_clean_shutdown_at = Some(now());
            }
            Ok(())
        })
    }

    pub fn report(&self) -> ReliabilityReport {
        let state = self.store.snapshot();
        let completed = state.clean_sessions.saturating_add(state.unclean_sessions);
        ReliabilityReport {
            total_sessions: state.total_sessions,
            clean_sessions: state.clean_sessions,
            unclean_sessions: state.unclean_sessions,
            current_session_active: state.active_session.is_some(),
            crash_free_session_rate: (completed > 0)
                .then_some(state.clean_sessions as f64 / completed as f64),
            last_clean_shutdown_at: state.last_clean_shutdown_at,
            local_only: true,
        }
    }
}

fn new_session_id() -> Result<String, StorageError> {
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|source| StorageError::Io {
        path: PathBuf::from("reliability-session-rng"),
        source: std::io::Error::other(source),
    })?;
    Ok(hex::encode(random))
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(i64::MAX as u64) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_and_unclean_sessions_are_distinguished_across_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reliability.json");
        let first = ReliabilityService::open(&path).unwrap();
        assert!(first.report().current_session_active);
        first.finish_clean().unwrap();
        drop(first);

        let second = ReliabilityService::open(&path).unwrap();
        assert_eq!(second.report().clean_sessions, 1);
        drop(second); // simulated crash: marker remains

        let third = ReliabilityService::open(&path).unwrap();
        let report = third.report();
        assert_eq!(report.total_sessions, 3);
        assert_eq!(report.clean_sessions, 1);
        assert_eq!(report.unclean_sessions, 1);
        assert_eq!(report.crash_free_session_rate, Some(0.5));
    }
}
