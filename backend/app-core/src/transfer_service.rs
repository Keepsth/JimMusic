//! 持久化传输任务状态机与 CID 验证提交。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use jimmusic_protocol::{
    ErrorEnvelopeV1, NetworkPolicyV1, TransferKind, TransferState, TransferTaskV1, SCHEMA_V1,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::node_service::{NodeError, NodeService};
use crate::storage::{AtomicJsonStore, StorageError};
use crate::{Event, EventBus};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TransferRepositoryState {
    schema_version: u16,
    tasks: BTreeMap<String, TransferTaskV1>,
    idempotency: BTreeMap<String, String>,
}

/// 应用网络类别策略后的效果：被自动暂停与被自动恢复（重新排队）的任务。
#[derive(Debug, Clone, Default)]
pub struct NetworkClassEffect {
    pub paused: Vec<TransferTaskV1>,
    pub resumed: Vec<TransferTaskV1>,
}

/// 网络策略自动暂停的结构化原因（NOD-006）。
fn network_pause_error(cellular: bool, wifi_only: bool, metered_allowed: bool) -> ErrorEnvelopeV1 {
    let code = if wifi_only {
        "paused_wifi_only"
    } else {
        "paused_metered_network"
    };
    let message = if cellular && wifi_only {
        "current network is cellular but the task requires Wi-Fi".to_string()
    } else if cellular && !metered_allowed {
        "current network is cellular and metered-network use is not allowed".to_string()
    } else {
        "transfer paused by network policy".to_string()
    };
    ErrorEnvelopeV1 {
        schema_version: SCHEMA_V1,
        code: code.into(),
        message,
        subsystem: "transfer".into(),
        operation: "network_policy".into(),
        retryable: false,
        unsupported_reason: None,
        details: BTreeMap::new(),
        request_id: None,
        causes: Vec::new(),
    }
}

/// 蜂窝额度超限的结构化暂停原因（DST-010）。
fn quota_error(limit: u64) -> ErrorEnvelopeV1 {
    ErrorEnvelopeV1 {
        schema_version: SCHEMA_V1,
        code: "paused_cellular_quota".into(),
        message: format!("cellular quota of {limit} bytes for this task was exceeded"),
        subsystem: "transfer".into(),
        operation: "network_policy".into(),
        retryable: false,
        unsupported_reason: None,
        details: BTreeMap::new(),
        request_id: None,
        causes: Vec::new(),
    }
}

/// 任务是否因每任务蜂窝额度耗尽而受限（仅蜂窝网络下生效）。
pub fn cellular_quota_exceeded(task: &TransferTaskV1, class: Option<&str>) -> bool {
    class == Some("cellular")
        && task
            .network_policy
            .cellular_limit_bytes
            .is_some_and(|limit| task.cellular_bytes_used >= limit)
}

#[derive(Debug, thiserror::Error)]
pub enum TransferError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("transfer task `{0}` does not exist")]
    NotFound(String),
    #[error("invalid transfer transition {from:?} -> {to:?}")]
    InvalidTransition {
        from: TransferState,
        to: TransferState,
    },
    #[error("transfer task is already terminal")]
    Terminal,
    #[error("transfer priority must be between -100 and 100")]
    InvalidPriority,
    #[error(transparent)]
    Node(#[from] NodeError),
}

pub struct TransferService {
    store: AtomicJsonStore<TransferRepositoryState>,
    id_counter: AtomicU64,
    events: EventBus,
}

impl TransferService {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, TransferError> {
        Self::open_with_bus(path, EventBus::default())
    }

    pub fn open_with_bus(
        path: impl Into<PathBuf>,
        events: EventBus,
    ) -> Result<Self, TransferError> {
        let path = path.into();
        let store = AtomicJsonStore::open(
            &path,
            TransferRepositoryState {
                schema_version: SCHEMA_V1,
                tasks: BTreeMap::new(),
                idempotency: BTreeMap::new(),
            },
        )?;
        // NFR-014/API-007：拒绝更新版本写入的状态（降级保护），保留原文件。
        crate::storage::reject_future_schema_version(
            store.snapshot().schema_version,
            SCHEMA_V1,
            &path,
        )?;
        // 杀进程恢复：中间态退回 queued，保留已完成字节和重试计数。
        store.transact(|state| {
            for task in state.tasks.values_mut() {
                if matches!(
                    task.state,
                    TransferState::Resolving
                        | TransferState::Transferring
                        | TransferState::Verifying
                        | TransferState::Committing
                ) {
                    task.state = TransferState::Queued;
                    task.retry_count = task.retry_count.saturating_add(1);
                    task.updated_at = now();
                }
            }
            Ok(())
        })?;
        Ok(Self {
            store,
            id_counter: AtomicU64::new(0),
            events,
        })
    }

    pub fn create(
        &self,
        request_id: &str,
        kind: TransferKind,
        target_cid: String,
        destination: Option<String>,
        policy: NetworkPolicyV1,
    ) -> Result<TransferTaskV1, TransferError> {
        self.create_with_priority(request_id, kind, target_cid, destination, policy, 0)
    }

    pub fn create_with_priority(
        &self,
        request_id: &str,
        kind: TransferKind,
        target_cid: String,
        destination: Option<String>,
        policy: NetworkPolicyV1,
        priority: i16,
    ) -> Result<TransferTaskV1, TransferError> {
        if !(-100..=100).contains(&priority) {
            return Err(TransferError::InvalidPriority);
        }
        if let Some(existing) = self
            .store
            .snapshot()
            .idempotency
            .get(request_id)
            .and_then(|task_id| self.store.snapshot().tasks.get(task_id).cloned())
        {
            return Ok(existing);
        }
        let task_id = task_id(
            request_id,
            &target_cid,
            self.id_counter.fetch_add(1, Ordering::Relaxed),
        );
        let timestamp = now();
        let task = TransferTaskV1 {
            schema_version: SCHEMA_V1,
            task_id: task_id.clone(),
            kind,
            target_cid,
            state: TransferState::Queued,
            priority,
            bytes_total: None,
            bytes_completed: 0,
            speed_bytes_per_second: 0,
            providers: Vec::new(),
            retry_count: 0,
            next_retry_at: None,
            network_policy: policy,
            destination,
            paused_by_network: false,
            cellular_bytes_used: 0,
            error: None,
            created_at: timestamp,
            updated_at: timestamp,
        };
        let created = self.store.transact(|state| {
            if let Some(existing) = state.idempotency.get(request_id) {
                return Ok(state.tasks[existing].clone());
            }
            state.idempotency.insert(request_id.into(), task_id.clone());
            state.tasks.insert(task_id, task.clone());
            Ok(task.clone())
        })?;
        self.publish_task(&created);
        Ok(created)
    }

    pub fn list(&self) -> Vec<TransferTaskV1> {
        let mut tasks: Vec<_> = self.store.snapshot().tasks.into_values().collect();
        tasks.sort_by(|left, right| {
            right
                .priority
                .cmp(&left.priority)
                .then_with(|| left.created_at.cmp(&right.created_at))
                .then_with(|| left.task_id.cmp(&right.task_id))
        });
        tasks
    }

    pub fn next_queued(&self) -> Option<TransferTaskV1> {
        self.list()
            .into_iter()
            .find(|task| task.state == TransferState::Queued)
    }

    pub fn get(&self, task_id: &str) -> Option<TransferTaskV1> {
        self.store.snapshot().tasks.get(task_id).cloned()
    }

    pub fn set_priority(
        &self,
        task_id: &str,
        priority: i16,
    ) -> Result<TransferTaskV1, TransferError> {
        if !(-100..=100).contains(&priority) {
            return Err(TransferError::InvalidPriority);
        }
        let snapshot = self
            .get(task_id)
            .ok_or_else(|| TransferError::NotFound(task_id.into()))?;
        if !matches!(
            snapshot.state,
            TransferState::Queued | TransferState::Paused
        ) {
            return Err(TransferError::InvalidTransition {
                from: snapshot.state,
                to: snapshot.state,
            });
        }
        let task = self.store.transact(|state| {
            let task = state.tasks.get_mut(task_id).expect("checked above");
            task.priority = priority;
            task.updated_at = now();
            Ok(task.clone())
        })?;
        self.publish_task(&task);
        Ok(task)
    }

    pub fn pause(&self, task_id: &str) -> Result<TransferTaskV1, TransferError> {
        self.transition(task_id, TransferState::Paused)?;
        // 用户手动暂停：清除网络策略暂停标记，避免回到 Wi-Fi 时被自动恢复。
        self.set_paused_by_network(task_id, false)
    }

    pub fn resume(&self, task_id: &str) -> Result<TransferTaskV1, TransferError> {
        self.transition(task_id, TransferState::Queued)?;
        self.set_paused_by_network(task_id, false)
    }

    pub fn cancel(&self, task_id: &str) -> Result<TransferTaskV1, TransferError> {
        self.transition(task_id, TransferState::Cancelled)
    }

    pub fn retry(&self, task_id: &str) -> Result<TransferTaskV1, TransferError> {
        let snapshot = self
            .get(task_id)
            .ok_or_else(|| TransferError::NotFound(task_id.into()))?;
        if snapshot.state != TransferState::Failed {
            return Err(TransferError::InvalidTransition {
                from: snapshot.state,
                to: TransferState::Queued,
            });
        }
        let task = self.store.transact(|state| {
            let task = state.tasks.get_mut(task_id).expect("checked above");
            task.state = TransferState::Queued;
            task.retry_count = task.retry_count.saturating_add(1);
            task.next_retry_at = None;
            task.error = None;
            task.speed_bytes_per_second = 0;
            task.paused_by_network = false;
            task.updated_at = now();
            Ok(task.clone())
        })?;
        self.publish_task(&task);
        Ok(task)
    }

    fn set_paused_by_network(
        &self,
        task_id: &str,
        value: bool,
    ) -> Result<TransferTaskV1, TransferError> {
        let task = self.store.transact(|state| {
            let task = state
                .tasks
                .get_mut(task_id)
                .ok_or_else(|| StorageError::Corrupt {
                    path: PathBuf::from("transfers"),
                    reason: format!("missing task `{task_id}`"),
                })?;
            task.paused_by_network = value;
            task.updated_at = now();
            Ok(task.clone())
        })?;
        self.publish_task(&task);
        Ok(task)
    }

    /// 网络类别策略（NOD-006）：按当前网络类别与计量开关暂停/恢复任务。
    ///
    /// - `class == "cellular"` 且未允许计量网络：暂停所有进行中任务；
    /// - `class == "cellular"`：`wifi_only` 任务无条件暂停（它们要求 Wi-Fi）；
    /// - 回到允许的类别：只自动恢复 `paused_by_network` 的任务，用户手动
    ///   暂停的任务保持暂停；
    /// - 未知/未声明类别视为非计量网络（宽松），UI 应引导用户声明。
    pub fn apply_network_class(
        &self,
        class: Option<&str>,
        metered_allowed: bool,
        timestamp: i64,
    ) -> Result<NetworkClassEffect, TransferError> {
        let cellular = class == Some("cellular");
        let metered_restricted = cellular && !metered_allowed;
        let tasks = self.list();
        let mut paused = Vec::new();
        let mut resumed = Vec::new();
        for task in tasks {
            if matches!(
                task.state,
                TransferState::Completed
                    | TransferState::Cancelled
                    | TransferState::IntegrityFailed
            ) {
                continue;
            }
            let restricted = metered_restricted
                || (cellular && task.network_policy.wifi_only)
                || cellular_quota_exceeded(&task, class);
            if restricted {
                if task.state == TransferState::Paused && task.paused_by_network {
                    continue; // 已按网络策略暂停，保持
                }
                if task.state == TransferState::Paused {
                    continue; // 用户手动暂停，不覆盖
                }
                let updated = self.store.transact(|state| {
                    let stored = state.tasks.get_mut(&task.task_id).ok_or_else(|| {
                        StorageError::Corrupt {
                            path: PathBuf::from("transfers"),
                            reason: format!("missing task `{}`", task.task_id),
                        }
                    })?;
                    stored.state = TransferState::Paused;
                    stored.paused_by_network = true;
                    stored.error = Some(
                        task.network_policy
                            .cellular_limit_bytes
                            .filter(|_| cellular_quota_exceeded(&task, class))
                            .map(quota_error)
                            .unwrap_or_else(|| {
                                network_pause_error(
                                    cellular,
                                    task.network_policy.wifi_only,
                                    metered_allowed,
                                )
                            }),
                    );
                    stored.updated_at = timestamp;
                    Ok(stored.clone())
                })?;
                self.publish_task(&updated);
                paused.push(updated);
            } else if task.paused_by_network && task.state == TransferState::Paused {
                let updated = self.store.transact(|state| {
                    let stored = state.tasks.get_mut(&task.task_id).ok_or_else(|| {
                        StorageError::Corrupt {
                            path: PathBuf::from("transfers"),
                            reason: format!("missing task `{}`", task.task_id),
                        }
                    })?;
                    stored.state = TransferState::Queued;
                    stored.paused_by_network = false;
                    stored.error = None;
                    stored.updated_at = timestamp;
                    Ok(stored.clone())
                })?;
                self.publish_task(&updated);
                resumed.push(updated);
            }
        }
        Ok(NetworkClassEffect { paused, resumed })
    }

    /// DST-010：累加蜂窝网络下已下载字节；超出每任务额度时把任务
    /// 暂停并给出结构化原因（回到 Wi-Fi 后由网络策略恢复）。
    pub fn add_cellular_bytes(
        &self,
        task_id: &str,
        bytes: u64,
    ) -> Result<TransferTaskV1, TransferError> {
        let task = self.store.transact(|state| {
            let task = state
                .tasks
                .get_mut(task_id)
                .ok_or_else(|| StorageError::Corrupt {
                    path: PathBuf::from("transfers"),
                    reason: format!("missing task `{task_id}`"),
                })?;
            if is_terminal(task.state) || task.state == TransferState::Paused {
                return Ok(task.clone());
            }
            task.cellular_bytes_used = task.cellular_bytes_used.saturating_add(bytes);
            if let Some(limit) = task.network_policy.cellular_limit_bytes {
                if task.cellular_bytes_used >= limit {
                    task.state = TransferState::Paused;
                    task.paused_by_network = true;
                    task.error = Some(quota_error(limit));
                }
            }
            task.updated_at = now();
            Ok(task.clone())
        })?;
        self.publish_task(&task);
        Ok(task)
    }

    /// 按当前类别判定任务是否允许排队执行（供创建/恢复入口使用）。
    pub fn network_restricted(
        &self,
        policy: &NetworkPolicyV1,
        class: Option<&str>,
        metered_allowed: bool,
    ) -> bool {
        let cellular = class == Some("cellular");
        (cellular && !metered_allowed) || (cellular && policy.wifi_only)
    }

    pub fn mark_resolving(&self, task_id: &str) -> Result<TransferTaskV1, TransferError> {
        self.transition(task_id, TransferState::Resolving)
    }

    pub fn record_progress(
        &self,
        task_id: &str,
        completed: u64,
        total: Option<u64>,
        speed: u64,
        providers: Vec<String>,
    ) -> Result<TransferTaskV1, TransferError> {
        let task = self
            .store
            .transact(|state| {
                let task = state
                    .tasks
                    .get_mut(task_id)
                    .ok_or_else(|| StorageError::Corrupt {
                        path: PathBuf::from("transfers"),
                        reason: format!("missing task `{task_id}`"),
                    })?;
                if is_terminal(task.state) {
                    return Err(StorageError::Corrupt {
                        path: PathBuf::from("transfers"),
                        reason: "cannot update terminal task".into(),
                    });
                }
                task.state = TransferState::Transferring;
                task.bytes_completed = completed;
                task.bytes_total = total;
                task.speed_bytes_per_second = speed;
                task.providers = providers;
                task.updated_at = now();
                Ok(task.clone())
            })
            .map_err(map_storage_error)?;
        self.publish_task(&task);
        Ok(task)
    }

    /// 校验完整内容并提交到可信内容仓库；校验失败绝不进入 completed。
    pub fn verify_and_commit(
        &self,
        task_id: &str,
        codec: u64,
        bytes: &[u8],
        node: &NodeService,
        pin: bool,
        persistent: bool,
    ) -> Result<TransferTaskV1, TransferError> {
        let task = self
            .get(task_id)
            .ok_or_else(|| TransferError::NotFound(task_id.into()))?;
        self.transition(task_id, TransferState::Verifying)?;
        if let Err(error) = node.put_verified(&task.target_cid, codec, bytes, pin, persistent) {
            let state = if matches!(error, NodeError::Integrity { .. }) {
                TransferState::IntegrityFailed
            } else {
                TransferState::Failed
            };
            let envelope = ErrorEnvelopeV1 {
                schema_version: SCHEMA_V1,
                code: if state == TransferState::IntegrityFailed {
                    "integrity_failed"
                } else {
                    "commit_failed"
                }
                .into(),
                message: error.to_string(),
                subsystem: "transfer".into(),
                operation: "verify_and_commit".into(),
                retryable: state == TransferState::Failed,
                unsupported_reason: None,
                details: BTreeMap::new(),
                request_id: None,
                causes: vec!["node repository rejected the object".into()],
            };
            let _ = self.fail(task_id, state, envelope);
            return Err(TransferError::Node(error));
        }
        self.transition(task_id, TransferState::Committing)?;
        let committed = self
            .store
            .transact(|state| {
                let task = state.tasks.get_mut(task_id).expect("task checked above");
                task.state = TransferState::Completed;
                task.bytes_completed = bytes.len() as u64;
                task.bytes_total = Some(bytes.len() as u64);
                task.speed_bytes_per_second = 0;
                task.error = None;
                task.updated_at = now();
                Ok(task.clone())
            })
            .map_err(TransferError::from)?;
        self.publish_task(&committed);
        Ok(committed)
    }

    /// 对下载到临时文件的内容进行流式 CID 校验并提交，避免大对象整体进入内存。
    pub fn verify_and_commit_file(
        &self,
        task_id: &str,
        codec: u64,
        path: &std::path::Path,
        node: &NodeService,
        pin: bool,
        persistent: bool,
    ) -> Result<TransferTaskV1, TransferError> {
        let task = self
            .get(task_id)
            .ok_or_else(|| TransferError::NotFound(task_id.into()))?;
        self.transition(task_id, TransferState::Verifying)?;
        let byte_length =
            match node.put_verified_file(&task.target_cid, codec, path, pin, persistent) {
                Ok(length) => length,
                Err(error) => {
                    let state = if matches!(error, NodeError::Integrity { .. }) {
                        TransferState::IntegrityFailed
                    } else {
                        TransferState::Failed
                    };
                    let envelope = ErrorEnvelopeV1 {
                        schema_version: SCHEMA_V1,
                        code: if state == TransferState::IntegrityFailed {
                            "integrity_failed"
                        } else {
                            "commit_failed"
                        }
                        .into(),
                        message: error.to_string(),
                        subsystem: "transfer".into(),
                        operation: "verify_and_commit_file".into(),
                        retryable: state == TransferState::Failed,
                        unsupported_reason: None,
                        details: BTreeMap::new(),
                        request_id: None,
                        causes: vec!["node repository rejected the streamed object".into()],
                    };
                    let _ = self.fail(task_id, state, envelope);
                    return Err(TransferError::Node(error));
                }
            };
        self.transition(task_id, TransferState::Committing)?;
        let committed = self
            .store
            .transact(|state| {
                let task = state.tasks.get_mut(task_id).expect("task checked above");
                task.state = TransferState::Completed;
                task.bytes_completed = byte_length;
                task.bytes_total = Some(byte_length);
                task.speed_bytes_per_second = 0;
                task.error = None;
                task.updated_at = now();
                Ok(task.clone())
            })
            .map_err(TransferError::from)?;
        self.publish_task(&committed);
        Ok(committed)
    }

    pub fn fail(
        &self,
        task_id: &str,
        terminal_state: TransferState,
        error: ErrorEnvelopeV1,
    ) -> Result<TransferTaskV1, TransferError> {
        if !matches!(
            terminal_state,
            TransferState::Failed | TransferState::IntegrityFailed
        ) {
            return Err(TransferError::InvalidTransition {
                from: self
                    .get(task_id)
                    .map(|task| task.state)
                    .unwrap_or(TransferState::Queued),
                to: terminal_state,
            });
        }
        let task = self
            .store
            .transact(|state| {
                let task = state
                    .tasks
                    .get_mut(task_id)
                    .ok_or_else(|| StorageError::Corrupt {
                        path: PathBuf::from("transfers"),
                        reason: format!("missing task `{task_id}`"),
                    })?;
                task.state = terminal_state;
                task.error = Some(error);
                task.updated_at = now();
                Ok(task.clone())
            })
            .map_err(map_storage_error)?;
        self.publish_task(&task);
        Ok(task)
    }

    fn transition(
        &self,
        task_id: &str,
        target: TransferState,
    ) -> Result<TransferTaskV1, TransferError> {
        let snapshot = self
            .get(task_id)
            .ok_or_else(|| TransferError::NotFound(task_id.into()))?;
        if !transition_allowed(snapshot.state, target) {
            return Err(if is_terminal(snapshot.state) {
                TransferError::Terminal
            } else {
                TransferError::InvalidTransition {
                    from: snapshot.state,
                    to: target,
                }
            });
        }
        let task = self
            .store
            .transact(|state| {
                let task = state.tasks.get_mut(task_id).expect("task checked above");
                task.state = target;
                task.updated_at = now();
                if target == TransferState::Queued {
                    task.error = None;
                }
                Ok(task.clone())
            })
            .map_err(TransferError::from)?;
        self.publish_task(&task);
        Ok(task)
    }

    fn publish_task(&self, task: &TransferTaskV1) {
        self.events.publish(Event::TransferChanged {
            task_id: task.task_id.clone(),
            state: format!("{:?}", task.state).to_ascii_lowercase(),
            bytes_completed: task.bytes_completed,
        });
    }
}

fn transition_allowed(from: TransferState, to: TransferState) -> bool {
    use TransferState::*;
    matches!(
        (from, to),
        (Queued, Resolving)
            | (Queued, Paused)
            | (Queued, Cancelled)
            | (Resolving, Transferring)
            | (Resolving, Paused)
            | (Resolving, Cancelled)
            | (Transferring, Transferring)
            | (Transferring, Paused)
            | (Transferring, Cancelled)
            | (Transferring, Verifying)
            | (Paused, Queued)
            | (Paused, Cancelled)
            | (Failed, Queued)
            | (Verifying, Committing)
            | (Verifying, Failed)
            | (Verifying, IntegrityFailed)
            | (Committing, Completed)
            | (Committing, Failed)
    )
}

fn is_terminal(state: TransferState) -> bool {
    matches!(
        state,
        TransferState::Completed | TransferState::Cancelled | TransferState::IntegrityFailed
    )
}

fn map_storage_error(error: StorageError) -> TransferError {
    let text = error.to_string();
    if text.contains("missing task") {
        TransferError::NotFound(text)
    } else if text.contains("terminal task") {
        TransferError::Terminal
    } else {
        TransferError::Storage(error)
    }
}

fn task_id(request_id: &str, cid: &str, counter: u64) -> String {
    let mut hasher = Sha256::new();
    hasher.update(request_id.as_bytes());
    hasher.update([0]);
    hasher.update(cid.as_bytes());
    hasher.update(counter.to_le_bytes());
    format!("tr_{}", &hex::encode(hasher.finalize())[..24])
}

/// 当前 Unix 秒（任务时间戳单位）。
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(i64::MAX as u64) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node_service::RAW_CODEC;
    use jimmusic_protocol::cid_v1_for_bytes;

    fn policy() -> NetworkPolicyV1 {
        NetworkPolicyV1 {
            wifi_only: false,
            cellular_limit_bytes: None,
            max_concurrency: 2,
        }
    }

    #[test]
    fn create_is_idempotent_and_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transfers.json");
        let service = TransferService::open(&path).unwrap();
        let cid = cid_v1_for_bytes(RAW_CODEC, b"hello");
        let first = service
            .create("req-1", TransferKind::Download, cid.clone(), None, policy())
            .unwrap();
        let second = service
            .create("req-1", TransferKind::Download, cid, None, policy())
            .unwrap();
        assert_eq!(first.task_id, second.task_id);
        drop(service);
        assert_eq!(TransferService::open(path).unwrap().list().len(), 1);
    }

    fn policy_wifi_only() -> NetworkPolicyV1 {
        NetworkPolicyV1 {
            wifi_only: true,
            cellular_limit_bytes: None,
            max_concurrency: 2,
        }
    }

    fn quota_policy(limit: u64) -> NetworkPolicyV1 {
        NetworkPolicyV1 {
            wifi_only: false,
            cellular_limit_bytes: Some(limit),
            max_concurrency: 2,
        }
    }

    #[test]
    fn cellular_quota_pauses_task_and_resumes_on_wifi() {
        let dir = tempfile::tempdir().unwrap();
        let service = TransferService::open(dir.path().join("transfers.json")).unwrap();
        let task = service
            .create(
                "req-quota",
                TransferKind::Download,
                cid_v1_for_bytes(RAW_CODEC, b"q"),
                None,
                quota_policy(100),
            )
            .unwrap();

        // 蜂窝网络下累加计量：未超限继续，超限暂停并给出结构化原因。
        // （真实链路中 record_progress 先置 Transferring，再按块计量。）
        service
            .record_progress(&task.task_id, 60, None, 0, vec!["test".into()])
            .unwrap();
        let updated = service.add_cellular_bytes(&task.task_id, 60).unwrap();
        assert_eq!(updated.state, TransferState::Transferring);
        assert_eq!(updated.cellular_bytes_used, 60);
        let updated = service.add_cellular_bytes(&task.task_id, 40).unwrap();
        assert_eq!(updated.state, TransferState::Paused);
        assert!(updated.paused_by_network);
        assert_eq!(updated.error.unwrap().code, "paused_cellular_quota");

        // 蜂窝类别下再次应用策略：保持额度暂停（不重复恢复）。
        let effect = service
            .apply_network_class(Some("cellular"), true, 10)
            .unwrap();
        assert!(effect.resumed.is_empty());
        assert!(effect.paused.is_empty()); // 已暂停，保持
        assert_eq!(
            service.get(&task.task_id).unwrap().state,
            TransferState::Paused
        );

        // 回到 Wi-Fi：自动恢复排队（额度只约束蜂窝网络）。
        let effect = service
            .apply_network_class(Some("wifi"), false, 20)
            .unwrap();
        assert_eq!(effect.resumed.len(), 1);
        assert_eq!(
            service.get(&task.task_id).unwrap().state,
            TransferState::Queued
        );
        assert!(service.get(&task.task_id).unwrap().error.is_none());
    }

    #[test]
    fn cellular_quota_exceeded_requires_cellular_class() {
        let dir = tempfile::tempdir().unwrap();
        let service = TransferService::open(dir.path().join("transfers.json")).unwrap();
        let task = service
            .create(
                "req-quota2",
                TransferKind::Download,
                cid_v1_for_bytes(RAW_CODEC, b"q2"),
                None,
                quota_policy(10),
            )
            .unwrap();
        service.add_cellular_bytes(&task.task_id, 20).unwrap();
        let stored = service.get(&task.task_id).unwrap();
        assert!(cellular_quota_exceeded(&stored, Some("cellular")));
        assert!(!cellular_quota_exceeded(&stored, Some("wifi")));
        assert!(!cellular_quota_exceeded(&stored, None));
    }

    #[test]
    fn network_class_pauses_and_auto_resumes_only_network_pauses() {
        let dir = tempfile::tempdir().unwrap();
        let service = TransferService::open(dir.path().join("transfers.json")).unwrap();
        let wifi_task = service
            .create(
                "req-wifi",
                TransferKind::Download,
                cid_v1_for_bytes(RAW_CODEC, b"w"),
                None,
                policy_wifi_only(),
            )
            .unwrap();
        let normal_task = service
            .create(
                "req-normal",
                TransferKind::Download,
                cid_v1_for_bytes(RAW_CODEC, b"n"),
                None,
                policy(),
            )
            .unwrap();

        // 蜂窝网络 + 不允许计量：全部暂停，原因结构化。
        let effect = service
            .apply_network_class(Some("cellular"), false, 10)
            .unwrap();
        assert_eq!(effect.resumed.len(), 0);
        assert_eq!(effect.paused.len(), 2);
        for task in effect.paused {
            assert!(task.paused_by_network);
            let error = task.error.as_ref().unwrap();
            assert_eq!(error.operation, "network_policy");
            assert!(!error.retryable);
        }
        let wifi_paused = service.get(&wifi_task.task_id).unwrap();
        assert_eq!(wifi_paused.error.unwrap().code, "paused_wifi_only");
        let normal_paused = service.get(&normal_task.task_id).unwrap();
        assert_eq!(normal_paused.error.unwrap().code, "paused_metered_network");

        // 回到 Wi-Fi：仅网络暂停的任务自动恢复。
        let effect = service
            .apply_network_class(Some("wifi"), false, 20)
            .unwrap();
        assert_eq!(effect.paused.len(), 0);
        assert_eq!(effect.resumed.len(), 2);
        for task in effect.resumed {
            assert_eq!(task.state, TransferState::Queued);
            assert!(!task.paused_by_network);
            assert!(task.error.is_none());
        }

        // 用户手动暂停不受自动恢复影响。
        service.pause(&normal_task.task_id).unwrap();
        service
            .apply_network_class(Some("cellular"), false, 30)
            .unwrap();
        let user_paused = service.get(&normal_task.task_id).unwrap();
        assert_eq!(user_paused.state, TransferState::Paused);
        assert!(!user_paused.paused_by_network);
        let effect = service
            .apply_network_class(Some("wifi"), false, 40)
            .unwrap();
        assert_eq!(effect.resumed.len(), 1); // 只有网络暂停的 wifi_task 恢复
        assert!(effect
            .resumed
            .iter()
            .all(|task| task.task_id == wifi_task.task_id));
        assert_eq!(
            service.get(&normal_task.task_id).unwrap().state,
            TransferState::Paused
        );
    }

    #[test]
    fn network_class_cellular_with_metering_allowed_keeps_non_wifi_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let service = TransferService::open(dir.path().join("transfers.json")).unwrap();
        let wifi_task = service
            .create(
                "req-wifi",
                TransferKind::Download,
                cid_v1_for_bytes(RAW_CODEC, b"w"),
                None,
                policy_wifi_only(),
            )
            .unwrap();
        let normal_task = service
            .create(
                "req-normal",
                TransferKind::Download,
                cid_v1_for_bytes(RAW_CODEC, b"n"),
                None,
                policy(),
            )
            .unwrap();

        // 蜂窝 + 允许计量：仅 wifi_only 任务暂停。
        let effect = service
            .apply_network_class(Some("cellular"), true, 10)
            .unwrap();
        assert_eq!(effect.paused.len(), 1);
        assert_eq!(effect.paused[0].task_id, wifi_task.task_id);
        assert_eq!(
            service.get(&normal_task.task_id).unwrap().state,
            TransferState::Queued
        );

        // 未声明类别视为非计量：网络暂停的任务恢复。
        let effect = service.apply_network_class(None, false, 20).unwrap();
        assert_eq!(effect.resumed.len(), 1);
        assert_eq!(effect.resumed[0].task_id, wifi_task.task_id);
    }

    #[test]
    fn pause_resume_cancel_follow_state_machine() {
        let dir = tempfile::tempdir().unwrap();
        let service = TransferService::open(dir.path().join("transfers.json")).unwrap();
        let task = service
            .create(
                "req",
                TransferKind::Fetch,
                cid_v1_for_bytes(RAW_CODEC, b"x"),
                None,
                policy(),
            )
            .unwrap();
        assert_eq!(
            service.pause(&task.task_id).unwrap().state,
            TransferState::Paused
        );
        assert_eq!(
            service.resume(&task.task_id).unwrap().state,
            TransferState::Queued
        );
        assert_eq!(
            service.cancel(&task.task_id).unwrap().state,
            TransferState::Cancelled
        );
        assert!(matches!(
            service.resume(&task.task_id),
            Err(TransferError::Terminal)
        ));
    }

    #[test]
    fn queued_tasks_are_ordered_by_persistent_priority() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transfers.json");
        let service = TransferService::open(&path).unwrap();
        let low = service
            .create_with_priority(
                "low",
                TransferKind::Download,
                cid_v1_for_bytes(RAW_CODEC, b"low"),
                None,
                policy(),
                -10,
            )
            .unwrap();
        let high = service
            .create_with_priority(
                "high",
                TransferKind::Download,
                cid_v1_for_bytes(RAW_CODEC, b"high"),
                None,
                policy(),
                10,
            )
            .unwrap();
        assert_eq!(service.next_queued().unwrap().task_id, high.task_id);
        service.set_priority(&low.task_id, 50).unwrap();
        assert_eq!(service.next_queued().unwrap().task_id, low.task_id);
        drop(service);
        let reopened = TransferService::open(path).unwrap();
        assert_eq!(reopened.next_queued().unwrap().priority, 50);
        assert!(matches!(
            reopened.set_priority(&low.task_id, 101),
            Err(TransferError::InvalidPriority)
        ));
    }

    #[test]
    fn commit_verifies_cid_before_completion() {
        let dir = tempfile::tempdir().unwrap();
        let service = TransferService::open(dir.path().join("transfers.json")).unwrap();
        let node = NodeService::open(dir.path().join("node"), "peer").unwrap();
        let cid = cid_v1_for_bytes(RAW_CODEC, b"good");
        let task = service
            .create("req", TransferKind::Download, cid.clone(), None, policy())
            .unwrap();
        service.mark_resolving(&task.task_id).unwrap();
        service
            .record_progress(&task.task_id, 4, Some(4), 100, vec!["local".into()])
            .unwrap();
        let completed = service
            .verify_and_commit(&task.task_id, RAW_CODEC, b"good", &node, true, true)
            .unwrap();
        assert_eq!(completed.state, TransferState::Completed);
        assert_eq!(node.cat(&cid).unwrap(), b"good");
    }

    #[test]
    fn integrity_failure_is_terminal_and_never_commits() {
        let dir = tempfile::tempdir().unwrap();
        let service = TransferService::open(dir.path().join("transfers.json")).unwrap();
        let node = NodeService::open(dir.path().join("node"), "peer").unwrap();
        let cid = cid_v1_for_bytes(RAW_CODEC, b"expected");
        let task = service
            .create("req", TransferKind::Download, cid.clone(), None, policy())
            .unwrap();
        service.mark_resolving(&task.task_id).unwrap();
        service
            .record_progress(&task.task_id, 3, Some(3), 0, Vec::new())
            .unwrap();
        assert!(service
            .verify_and_commit(&task.task_id, RAW_CODEC, b"bad", &node, false, false)
            .is_err());
        assert_eq!(
            service.get(&task.task_id).unwrap().state,
            TransferState::IntegrityFailed
        );
        assert!(matches!(node.cat(&cid), Err(NodeError::NotFound(_))));
    }
}
