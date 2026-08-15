//! 应用内内容寻址仓库与 NodeService 基础实现。
//!
//! 本地仓库不依赖外部 Kubo：可 add/cat、逐对象 CID 校验、Pin/Unpin、配额保护、LRU
//! 缓存清理与重启恢复。网络传输适配器可以在此可信仓库之上注册 Provider。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use jimmusic_protocol::{
    cid_v1_for_bytes, cid_v1_for_sha256_digest, ErrorEnvelopeV1, NodeStatusV1, ProviderHealthState,
    ProviderHealthV1, DAG_CBOR_CODEC, SCHEMA_V1,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::storage::{AtomicJsonStore, StorageError};

pub const RAW_CODEC: u64 = 0x55;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NodeConfig {
    pub storage_limit_bytes: u64,
    pub cache_limit_bytes: u64,
    pub max_concurrent_transfers: u16,
    pub upload_limit_bytes_per_second: Option<u64>,
    pub download_limit_bytes_per_second: Option<u64>,
    pub metered_network_allowed: bool,
    /// 当前网络类别声明：`wifi` / `cellular` / `ethernet` / `unknown`；
    /// `None` 表示尚未声明（视为非计量网络，NOD-006）。
    #[serde(default)]
    pub network_class: Option<String>,
    /// 收藏曲目时自动协助 Pin 其内容 CID（用户显式开启，DST-009）。
    #[serde(default)]
    pub assist_pin_favorites: bool,
    /// 发布成功后自动复刻各 rendition 的内容 CID（用户显式开启，DST-010）。
    #[serde(default)]
    pub auto_replicate_published: bool,
    /// 显式配置的第三方 Kubo 兼容 Pin 服务端点（DST-009）。
    #[serde(default)]
    pub pin_services: Vec<String>,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            storage_limit_bytes: 20 * 1024 * 1024 * 1024,
            cache_limit_bytes: 2 * 1024 * 1024 * 1024,
            max_concurrent_transfers: 3,
            upload_limit_bytes_per_second: None,
            download_limit_bytes_per_second: None,
            metered_network_allowed: false,
            network_class: None,
            assist_pin_favorites: false,
            auto_replicate_published: false,
            pin_services: Vec::new(),
        }
    }
}

/// 允许的网络类别值。
pub const NETWORK_CLASSES: [&str; 4] = ["wifi", "cellular", "ethernet", "unknown"];

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ObjectRecord {
    codec: u64,
    byte_length: u64,
    pinned: bool,
    persistent: bool,
    last_accessed_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NodeRepositoryState {
    schema_version: u16,
    peer_id: String,
    running: bool,
    config: NodeConfig,
    objects: BTreeMap<String, ObjectRecord>,
    providers: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, thiserror::Error)]
pub enum NodeError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("object `{0}` is not present in the local repository")]
    NotFound(String),
    #[error("CID integrity check failed: expected {expected}, got {actual}")]
    Integrity { expected: String, actual: String },
    #[error("repository quota exceeded ({requested} requested, {available} available)")]
    Quota { requested: u64, available: u64 },
    #[error("invalid CID path component")]
    InvalidCid,
    #[error("invalid node configuration: {0}")]
    InvalidConfig(String),
    #[error("object IO failed: {0}")]
    Io(#[from] std::io::Error),
}

pub struct NodeService {
    objects_dir: PathBuf,
    state: AtomicJsonStore<NodeRepositoryState>,
    write_lock: Mutex<()>,
    bytes_up: AtomicU64,
    bytes_down: AtomicU64,
}

impl NodeService {
    pub fn open(root: impl Into<PathBuf>, peer_id: impl Into<String>) -> Result<Self, NodeError> {
        let root = root.into();
        let objects_dir = root.join("objects");
        std::fs::create_dir_all(&objects_dir)?;
        let state = AtomicJsonStore::open(
            root.join("node-state.json"),
            NodeRepositoryState {
                schema_version: SCHEMA_V1,
                peer_id: peer_id.into(),
                running: false,
                config: NodeConfig::default(),
                objects: BTreeMap::new(),
                providers: BTreeMap::new(),
            },
        )?;
        Ok(Self {
            objects_dir,
            state,
            write_lock: Mutex::new(()),
            bytes_up: AtomicU64::new(0),
            bytes_down: AtomicU64::new(0),
        })
    }

    pub fn start(&self) -> Result<(), NodeError> {
        self.state.transact(|state| {
            state.running = true;
            Ok(())
        })?;
        Ok(())
    }

    pub fn stop(&self) -> Result<(), NodeError> {
        self.state.transact(|state| {
            state.running = false;
            Ok(())
        })?;
        Ok(())
    }

    pub fn config(&self) -> NodeConfig {
        self.state.snapshot().config
    }

    pub fn set_config(&self, config: NodeConfig) -> Result<(), NodeError> {
        validate_config(&config)?;
        self.state.transact(|state| {
            state.config = config;
            Ok(())
        })?;
        self.enforce_cache_quota()?;
        Ok(())
    }

    pub fn add_raw(&self, bytes: &[u8], pin: bool) -> Result<String, NodeError> {
        let cid = cid_v1_for_bytes(RAW_CODEC, bytes);
        self.put_verified(&cid, RAW_CODEC, bytes, pin, pin)?;
        Ok(cid)
    }

    pub fn add_dag_cbor(&self, bytes: &[u8], pin: bool) -> Result<String, NodeError> {
        let cid = cid_v1_for_bytes(DAG_CBOR_CODEC, bytes);
        self.put_verified(&cid, DAG_CBOR_CODEC, bytes, pin, pin)?;
        Ok(cid)
    }

    pub fn put_verified(
        &self,
        expected_cid: &str,
        codec: u64,
        bytes: &[u8],
        pin: bool,
        persistent: bool,
    ) -> Result<(), NodeError> {
        validate_cid_component(expected_cid)?;
        let actual = cid_v1_for_bytes(codec, bytes);
        if actual != expected_cid {
            return Err(NodeError::Integrity {
                expected: expected_cid.to_string(),
                actual,
            });
        }
        let _guard = self.write_lock.lock().expect("node write lock poisoned");
        let snapshot = self.state.snapshot();
        let current_bytes: u64 = snapshot
            .objects
            .values()
            .map(|record| record.byte_length)
            .sum();
        let existing = snapshot
            .objects
            .get(expected_cid)
            .map(|record| record.byte_length)
            .unwrap_or(0);
        let projected = current_bytes
            .saturating_sub(existing)
            .saturating_add(bytes.len() as u64);
        if projected > snapshot.config.storage_limit_bytes {
            return Err(NodeError::Quota {
                requested: bytes.len() as u64,
                available: snapshot
                    .config
                    .storage_limit_bytes
                    .saturating_sub(current_bytes),
            });
        }

        let path = self.object_path(expected_cid)?;
        let temporary = path.with_extension("part");
        {
            use std::io::Write;
            let mut file = std::fs::File::create(&temporary)?;
            file.write_all(bytes)?;
            file.sync_all()?;
        }
        std::fs::rename(&temporary, &path)?;
        self.state.transact(|state| {
            state.objects.insert(
                expected_cid.to_string(),
                ObjectRecord {
                    codec,
                    byte_length: bytes.len() as u64,
                    pinned: pin,
                    persistent,
                    last_accessed_at: now(),
                },
            );
            Ok(())
        })?;
        drop(_guard);
        self.enforce_cache_quota()?;
        Ok(())
    }

    /// 从文件流式校验并原子提交对象，内存占用与对象大小无关。
    pub fn put_verified_file(
        &self,
        expected_cid: &str,
        codec: u64,
        source: &Path,
        pin: bool,
        persistent: bool,
    ) -> Result<u64, NodeError> {
        use std::io::Read;

        validate_cid_component(expected_cid)?;
        let mut input = std::fs::File::open(source)?;
        let byte_length = input.metadata()?.len();
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 128 * 1024];
        loop {
            let read = input.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        let actual = cid_v1_for_sha256_digest(codec, hasher.finalize().into());
        if actual != expected_cid {
            return Err(NodeError::Integrity {
                expected: expected_cid.to_string(),
                actual,
            });
        }

        let _guard = self.write_lock.lock().expect("node write lock poisoned");
        let snapshot = self.state.snapshot();
        let current_bytes: u64 = snapshot
            .objects
            .values()
            .map(|record| record.byte_length)
            .sum();
        let existing = snapshot
            .objects
            .get(expected_cid)
            .map(|record| record.byte_length)
            .unwrap_or(0);
        let projected = current_bytes
            .saturating_sub(existing)
            .saturating_add(byte_length);
        if projected > snapshot.config.storage_limit_bytes {
            return Err(NodeError::Quota {
                requested: byte_length,
                available: snapshot
                    .config
                    .storage_limit_bytes
                    .saturating_sub(current_bytes),
            });
        }

        let path = self.object_path(expected_cid)?;
        let temporary = path.with_extension("part");
        std::fs::copy(source, &temporary)?;
        std::fs::File::options()
            .write(true)
            .open(&temporary)?
            .sync_all()?;
        std::fs::rename(&temporary, &path)?;
        self.state.transact(|state| {
            state.objects.insert(
                expected_cid.to_string(),
                ObjectRecord {
                    codec,
                    byte_length,
                    pinned: pin,
                    persistent,
                    last_accessed_at: now(),
                },
            );
            Ok(())
        })?;
        drop(_guard);
        self.enforce_cache_quota()?;
        Ok(byte_length)
    }

    pub fn cat(&self, cid: &str) -> Result<Vec<u8>, NodeError> {
        let record = self
            .state
            .snapshot()
            .objects
            .get(cid)
            .cloned()
            .ok_or_else(|| NodeError::NotFound(cid.to_string()))?;
        let path = self.object_path(cid)?;
        let bytes = std::fs::read(path)?;
        let actual = cid_v1_for_bytes(record.codec, &bytes);
        if actual != cid {
            return Err(NodeError::Integrity {
                expected: cid.to_string(),
                actual,
            });
        }
        self.bytes_down
            .fetch_add(bytes.len() as u64, Ordering::Relaxed);
        self.state.transact(|state| {
            if let Some(record) = state.objects.get_mut(cid) {
                record.last_accessed_at = now();
            }
            Ok(())
        })?;
        Ok(bytes)
    }

    pub fn pin(&self, cid: &str) -> Result<(), NodeError> {
        if !self.state.snapshot().objects.contains_key(cid) {
            return Err(NodeError::NotFound(cid.to_string()));
        }
        self.state.transact(|state| {
            let record = state.objects.get_mut(cid).expect("checked above");
            record.pinned = true;
            record.persistent = true;
            Ok(())
        })?;
        Ok(())
    }

    pub fn unpin(&self, cid: &str) -> Result<(), NodeError> {
        if !self.state.snapshot().objects.contains_key(cid) {
            return Err(NodeError::NotFound(cid.to_string()));
        }
        self.state.transact(|state| {
            let record = state.objects.get_mut(cid).expect("checked above");
            record.pinned = false;
            Ok(())
        })?;
        Ok(())
    }

    pub fn list_pins(&self) -> Vec<String> {
        self.state
            .snapshot()
            .objects
            .into_iter()
            .filter_map(|(cid, record)| record.pinned.then_some(cid))
            .collect()
    }

    pub fn register_provider(&self, cid: &str, provider: String) -> Result<(), NodeError> {
        self.state.transact(|state| {
            let providers = state.providers.entry(cid.to_string()).or_default();
            if !providers.contains(&provider) {
                providers.push(provider);
            }
            Ok(())
        })?;
        Ok(())
    }

    pub fn provider_health(&self, cid: &str) -> ProviderHealthV1 {
        let state = self.state.snapshot();
        let local = state.objects.get(cid);
        let providers = state.providers.get(cid).map_or(0, Vec::len) as u32;
        ProviderHealthV1 {
            schema_version: SCHEMA_V1,
            cid: cid.to_string(),
            observed_providers: providers + u32::from(local.is_some()),
            last_success_at: local.map(|record| record.last_accessed_at),
            latency_ms: local.map(|_| 0),
            local_pin: local.is_some_and(|record| record.pinned),
            configured_pin_services: state.config.pin_services.clone(),
            health: if local.is_some() || providers > 0 {
                ProviderHealthState::Healthy
            } else {
                ProviderHealthState::Unavailable
            },
        }
    }

    pub fn status(&self) -> NodeStatusV1 {
        let state = self.state.snapshot();
        let repository_bytes: u64 = state
            .objects
            .values()
            .map(|record| record.byte_length)
            .sum();
        let pinned_bytes: u64 = state
            .objects
            .values()
            .filter(|record| record.pinned)
            .map(|record| record.byte_length)
            .sum();
        NodeStatusV1 {
            schema_version: SCHEMA_V1,
            peer_id: state.peer_id,
            lifecycle_state: if state.running { "running" } else { "stopped" }.into(),
            transports: vec!["local-cas".into()],
            listen_addresses: Vec::new(),
            peers: Vec::new(),
            connected_peers: 0,
            routing_status: "local_only".into(),
            repository_bytes,
            cache_bytes: repository_bytes.saturating_sub(pinned_bytes),
            pinned_bytes,
            bytes_up: self.bytes_up.load(Ordering::Relaxed),
            bytes_down: self.bytes_down.load(Ordering::Relaxed),
            limitations: vec!["p2p transport adapter is not active".into()],
            last_error: None,
        }
    }

    pub fn integrity_error(cid: &str, actual: &str) -> ErrorEnvelopeV1 {
        ErrorEnvelopeV1 {
            schema_version: SCHEMA_V1,
            code: "integrity_failed".into(),
            message: "content does not match its CID".into(),
            subsystem: "node".into(),
            operation: "cat".into(),
            retryable: false,
            unsupported_reason: None,
            details: BTreeMap::from([
                ("expected_cid".into(), cid.into()),
                ("actual_cid".into(), actual.into()),
            ]),
            request_id: None,
            causes: Vec::new(),
        }
    }

    fn enforce_cache_quota(&self) -> Result<(), NodeError> {
        let _guard = self.write_lock.lock().expect("node write lock poisoned");
        loop {
            let snapshot = self.state.snapshot();
            let cache_bytes: u64 = snapshot
                .objects
                .values()
                .filter(|record| !record.pinned && !record.persistent)
                .map(|record| record.byte_length)
                .sum();
            if cache_bytes <= snapshot.config.cache_limit_bytes {
                return Ok(());
            }
            let Some((cid, _)) = snapshot
                .objects
                .iter()
                .filter(|(_, record)| !record.pinned && !record.persistent)
                .min_by_key(|(_, record)| record.last_accessed_at)
            else {
                return Ok(());
            };
            let cid = cid.clone();
            let path = self.object_path(&cid)?;
            if path.exists() {
                std::fs::remove_file(path)?;
            }
            self.state.transact(|state| {
                state.objects.remove(&cid);
                Ok(())
            })?;
        }
    }

    fn object_path(&self, cid: &str) -> Result<PathBuf, NodeError> {
        validate_cid_component(cid)?;
        Ok(self.objects_dir.join(cid))
    }
}

fn validate_config(config: &NodeConfig) -> Result<(), NodeError> {
    if config.storage_limit_bytes == 0 {
        return Err(NodeError::InvalidConfig(
            "storage_limit_bytes must be greater than zero".into(),
        ));
    }
    if config.cache_limit_bytes > config.storage_limit_bytes {
        return Err(NodeError::InvalidConfig(
            "cache_limit_bytes must not exceed storage_limit_bytes".into(),
        ));
    }
    if !(1..=64).contains(&config.max_concurrent_transfers) {
        return Err(NodeError::InvalidConfig(
            "max_concurrent_transfers must be between 1 and 64".into(),
        ));
    }
    if config
        .upload_limit_bytes_per_second
        .is_some_and(|limit| limit == 0)
        || config
            .download_limit_bytes_per_second
            .is_some_and(|limit| limit == 0)
    {
        return Err(NodeError::InvalidConfig(
            "bandwidth limits must be greater than zero when configured".into(),
        ));
    }
    if let Some(class) = &config.network_class {
        if !NETWORK_CLASSES.contains(&class.as_str()) {
            return Err(NodeError::InvalidConfig(format!(
                "network_class must be one of {NETWORK_CLASSES:?} or null"
            )));
        }
    }
    if config.pin_services.len() > 16 {
        return Err(NodeError::InvalidConfig(
            "at most 16 third-party pin services may be configured".into(),
        ));
    }
    for service in &config.pin_services {
        if service.len() > 2000 {
            return Err(NodeError::InvalidConfig(
                "pin service endpoint exceeds 2000 characters".into(),
            ));
        }
        let Ok(uri) = url::Url::parse(service) else {
            return Err(NodeError::InvalidConfig(format!(
                "pin service `{service}` is not a valid URL"
            )));
        };
        if !matches!(uri.scheme(), "http" | "https")
            || uri.host_str().is_none()
            || !uri.username().is_empty()
            || uri.password().is_some()
        {
            return Err(NodeError::InvalidConfig(format!(
                "pin service `{service}` must be an http(s) URL without credentials"
            )));
        }
    }
    Ok(())
}

fn validate_cid_component(cid: &str) -> Result<(), NodeError> {
    if cid.len() < 8
        || cid.len() > 256
        || !cid.starts_with('b')
        || !cid
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    {
        return Err(NodeError::InvalidCid);
    }
    Ok(())
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
    fn add_cat_pin_and_reopen_are_persistent() {
        let dir = tempfile::tempdir().unwrap();
        let node = NodeService::open(dir.path(), "peer-a").unwrap();
        node.start().unwrap();
        let cid = node.add_raw(b"hello", true).unwrap();
        assert_eq!(node.cat(&cid).unwrap(), b"hello");
        assert_eq!(node.list_pins(), vec![cid.clone()]);
        drop(node);

        let reopened = NodeService::open(dir.path(), "ignored-new-peer").unwrap();
        assert_eq!(reopened.status().peer_id, "peer-a");
        assert_eq!(reopened.cat(&cid).unwrap(), b"hello");
        assert!(reopened.provider_health(&cid).local_pin);
    }

    #[test]
    fn tampered_object_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let node = NodeService::open(dir.path(), "peer-a").unwrap();
        let cid = node.add_raw(b"original", false).unwrap();
        std::fs::write(node.objects_dir.join(&cid), b"tampered").unwrap();
        assert!(matches!(node.cat(&cid), Err(NodeError::Integrity { .. })));
    }

    #[test]
    fn cache_quota_never_evicts_pins() {
        let dir = tempfile::tempdir().unwrap();
        let node = NodeService::open(dir.path(), "peer-a").unwrap();
        let pinned = node.add_raw(b"pinned", true).unwrap();
        let cached = node.add_raw(b"cached", false).unwrap();
        let mut config = node.config();
        config.cache_limit_bytes = 0;
        node.set_config(config).unwrap();
        assert_eq!(node.cat(&pinned).unwrap(), b"pinned");
        assert!(matches!(node.cat(&cached), Err(NodeError::NotFound(_))));
    }

    #[test]
    fn wrong_expected_cid_never_commits() {
        let dir = tempfile::tempdir().unwrap();
        let node = NodeService::open(dir.path(), "peer-a").unwrap();
        let wrong = cid_v1_for_bytes(RAW_CODEC, b"other");
        assert!(matches!(
            node.put_verified(&wrong, RAW_CODEC, b"payload", false, false),
            Err(NodeError::Integrity { .. })
        ));
        assert!(!node.objects_dir.join(wrong).exists());
    }

    #[test]
    fn large_file_is_stream_verified_and_persisted() {
        let dir = tempfile::tempdir().unwrap();
        let node = NodeService::open(dir.path().join("node"), "peer").unwrap();
        let source = dir.path().join("large.bin");
        let bytes = vec![0x5a; 2 * 1024 * 1024];
        std::fs::write(&source, &bytes).unwrap();
        let cid = cid_v1_for_bytes(RAW_CODEC, &bytes);
        assert_eq!(
            node.put_verified_file(&cid, RAW_CODEC, &source, true, true)
                .unwrap(),
            bytes.len() as u64
        );
        assert_eq!(node.cat(&cid).unwrap(), bytes);
    }
}
