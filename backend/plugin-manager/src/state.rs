//! 插件管理器的共享状态：插件记录、本地仓库缓存、IPFS 客户端。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use app_core::audio_graph::AudioGraphManager;
use app_core::community_service::CommunitySourceService;
use app_core::library_service::LibraryService;
use app_core::node_service::NodeService;
use app_core::p2p_node::{EmbeddedIpfsNode, EmbeddedNodeConfig};
use app_core::publication_service::PublicationService;
use app_core::reliability::ReliabilityService;
use app_core::storage::AtomicJsonStore;
use app_core::transfer_service::TransferService;
use app_core::{EventBus, IpfsClient};
use jimmusic_protocol::{
    AudioEdgeSpecV1, AudioFormatSpecV1, AudioGraphMode, AudioGraphSpecV1, AudioMediaType,
    AudioNodeSpecV1, AudioNodeType, AudioPortSpecV1, CommunitySourceManifestV1, NodeFailurePolicy,
    TransferState, SCHEMA_V1,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 清理 `transfer-parts/` 中的孤儿 part 文件：任务不存在或已终结（无法继续
/// 流式播放）的文件。`keep_task_id` 用于流端点启动时的同步清理，避免误删
/// 当前正在被请求的文件。完整 part 文件由任务成功路径保留，供
/// `/v1/transfers/{id}/stream` 在任务终结后完成尾部交接（DST-007）。
pub fn sweep_transfer_parts(
    repo_dir: &Path,
    transfers: &TransferService,
    keep_task_id: Option<&str>,
) {
    let dir = repo_dir.join("transfer-parts");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let Some(task_id) = name.strip_suffix(".part") else {
            continue;
        };
        if keep_task_id == Some(task_id) {
            continue;
        }
        let keep = transfers.get(task_id).is_some_and(|task| {
            !matches!(
                task.state,
                TransferState::Completed
                    | TransferState::Failed
                    | TransferState::Cancelled
                    | TransferState::IntegrityFailed
            )
        });
        if !keep {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// 插件来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginSource {
    /// 本地仓库。
    Local,
    /// IPFS 网络。
    Ipfs,
    /// HTTP 镜像。
    Http,
}

/// 一条插件记录（元数据 + 来源 + 校验信息）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginRecord {
    /// 插件名。
    pub name: String,
    /// 版本号。
    pub version: String,
    /// 作者。
    pub author: String,
    /// 插件种类（见 plugin_abi::PluginKind）。
    pub kind: String,
    /// 来源。
    pub source: PluginSource,
    /// 下载地址（HTTP）或 CID（IPFS）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// 文件 SHA-256 摘要（十六进制），用于签名/完整性校验。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// 动态库文件本地相对路径（安装后）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lib_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LegacyPluginState {
    schema_version: u16,
    records: BTreeMap<String, PluginRecord>,
    active_output: Option<String>,
}

/// 共享应用状态。
#[derive(Clone)]
pub struct AppState {
    /// 本地仓库缓存目录。
    pub repo_dir: Arc<PathBuf>,
    /// IPFS HTTP 客户端。
    pub ipfs: IpfsClient,
    /// 插件记录表：名字 -> 记录（用 Mutex 包裹以便 axum 无状态处理器共享）。
    legacy: Arc<AtomicJsonStore<LegacyPluginState>>,
    /// API 令牌（可选，用于控制面节点认证）。
    api_token: Arc<Option<String>>,
    /// 当前激活的音频输出插件名（`None` 表示未激活）。
    pub lifecycle: Arc<crate::PluginLifecycleService>,
    pub wasm: Arc<crate::WasmPluginSupervisor>,
    pub node_identity: Arc<crate::NodeIdentity>,
    pub node: Arc<NodeService>,
    embedded_node: Arc<tokio::sync::Mutex<Option<Arc<EmbeddedIpfsNode>>>>,
    pub transfers: Arc<TransferService>,
    pub publications: Arc<PublicationService>,
    pub community: Arc<CommunitySourceService>,
    pub library: Arc<LibraryService>,
    pub reliability: Arc<ReliabilityService>,
    pub audio_graph: Arc<AudioGraphManager<1024>>,
    pub transfer_slots: Arc<tokio::sync::RwLock<Arc<tokio::sync::Semaphore>>>,
    /// Serializes queued-task selection so priority ordering is deterministic.
    pub transfer_scheduler: Arc<tokio::sync::Mutex<()>>,
    /// Prevents duplicate concurrent submissions of the same persisted report.
    pub moderation_scheduler: Arc<tokio::sync::Mutex<()>>,
    pub events: EventBus,
    pub idempotency: Arc<crate::idempotency::IdempotencyService>,
}

impl AppState {
    /// 清理孤儿 transfer part 文件，保留 [keep_task_id] 对应的文件。
    /// 由 `/v1/transfers/{id}/stream` 启动时调用，避免已完成任务残留的
    /// part 文件无限积累。
    pub fn sweep_transfer_parts(&self, keep_task_id: Option<&str>) {
        sweep_transfer_parts(&self.repo_dir, &self.transfers, keep_task_id);
    }

    /// 创建状态并确保仓库目录存在。
    pub fn new(repo_dir: String, ipfs_gateway: String) -> std::io::Result<Self> {
        let repo = PathBuf::from(repo_dir);
        std::fs::create_dir_all(&repo)?;
        let legacy = AtomicJsonStore::open(
            repo.join("legacy-plugins.json"),
            LegacyPluginState {
                schema_version: 1,
                records: BTreeMap::new(),
                active_output: None,
            },
        )
        .map_err(std::io::Error::other)?;
        let lifecycle = crate::PluginLifecycleService::open(repo.join("plugins"))
            .map_err(std::io::Error::other)?;
        let wasm = crate::WasmPluginSupervisor::deny_all().map_err(std::io::Error::other)?;
        let node_identity = crate::node::NodeIdentity::load_or_create(&repo.join("node-key.pb"))?;
        let peer_id = node_identity.peer_id_str();
        let node = NodeService::open(repo.join("node"), peer_id).map_err(std::io::Error::other)?;
        node.start().map_err(std::io::Error::other)?;
        let events = EventBus::new(2_048);
        let transfers = TransferService::open_with_bus(repo.join("transfers.json"), events.clone())
            .map_err(std::io::Error::other)?;
        sweep_transfer_parts(&repo, &transfers, None);
        let publications = PublicationService::open(repo.join("publications.json"))
            .map_err(std::io::Error::other)?;
        let community = CommunitySourceService::open(repo.join("community.json"))
            .map_err(std::io::Error::other)?;
        community
            .ensure_bootstrap_source(
                official_bootstrap_source(),
                OFFICIAL_BOOTSTRAP_PUBLIC_KEY.into(),
                &node,
                100,
            )
            .map_err(std::io::Error::other)?;
        let library =
            LibraryService::open(repo.join("library.json")).map_err(std::io::Error::other)?;
        let reliability = ReliabilityService::open(repo.join("reliability.json"))
            .map_err(std::io::Error::other)?;
        let audio_graph =
            AudioGraphManager::new(default_audio_graph()).map_err(std::io::Error::other)?;
        let transfer_slots = Arc::new(tokio::sync::RwLock::new(Arc::new(
            tokio::sync::Semaphore::new(node.config().max_concurrent_transfers.max(1) as usize),
        )));
        let idempotency =
            crate::idempotency::IdempotencyService::open(repo.join("api-idempotency.json"))
                .map_err(std::io::Error::other)?;
        Ok(Self {
            repo_dir: Arc::new(repo),
            ipfs: IpfsClient::new(ipfs_gateway),
            legacy: Arc::new(legacy),
            api_token: Arc::new(None),
            lifecycle: Arc::new(lifecycle),
            wasm: Arc::new(wasm),
            node_identity: Arc::new(node_identity),
            node: Arc::new(node),
            embedded_node: Arc::new(tokio::sync::Mutex::new(None)),
            transfers: Arc::new(transfers),
            publications: Arc::new(publications),
            community: Arc::new(community),
            library: Arc::new(library),
            reliability: Arc::new(reliability),
            audio_graph: Arc::new(audio_graph),
            transfer_slots,
            transfer_scheduler: Arc::new(tokio::sync::Mutex::new(())),
            moderation_scheduler: Arc::new(tokio::sync::Mutex::new(())),
            events,
            idempotency: Arc::new(idempotency),
        })
    }

    /// Starts the native Bitswap/Kademlia node with the same persisted key
    /// used by node-authentication signatures. Calling this more than once is
    /// harmless.
    pub async fn start_embedded_node(&self) -> std::io::Result<()> {
        let mut guard = self.embedded_node.lock().await;
        if guard.is_some() {
            return Ok(());
        }
        let identity = self.node_identity.private_key_protobuf()?;
        let mut config =
            EmbeddedNodeConfig::native(self.repo_dir.join("ipfs-repository"), identity);
        if let Ok(value) = std::env::var("JIMMUSIC_IPFS_LISTEN") {
            let addresses = comma_separated(&value);
            if !addresses.is_empty() {
                config.listen_addresses = addresses;
            }
        }
        if let Ok(value) = std::env::var("JIMMUSIC_IPFS_BOOTSTRAP") {
            config.bootstrap_addresses = comma_separated(&value);
        }
        if std::env::var("JIMMUSIC_IPFS_MDNS").is_ok_and(|value| value == "0") {
            config.enable_mdns = false;
        }
        let embedded = Arc::new(
            EmbeddedIpfsNode::start(config)
                .await
                .map_err(std::io::Error::other)?,
        );
        if embedded.peer_id() != self.node_identity.peer_id_str() {
            embedded.shutdown().await;
            return Err(std::io::Error::other(
                "embedded IPFS PeerId does not match the persisted node identity",
            ));
        }

        // Restore network availability for objects that the trusted local CAS
        // already marks as pinned. Each block is re-verified by rust-ipfs.
        for cid in self.node.list_pins() {
            let bytes = self.node.cat(&cid).map_err(std::io::Error::other)?;
            embedded
                .put_block(&cid, &bytes, true)
                .await
                .map_err(std::io::Error::other)?;
        }
        *guard = Some(embedded);
        Ok(())
    }

    pub async fn embedded_node(&self) -> Option<Arc<EmbeddedIpfsNode>> {
        self.embedded_node.lock().await.clone()
    }

    pub async fn node_status(&self) -> jimmusic_protocol::NodeStatusV1 {
        let mut status = self.node.status();
        let Some(embedded) = self.embedded_node().await else {
            return status;
        };
        match embedded.status().await {
            Ok(network) => {
                status.peer_id = network.peer_id;
                status.lifecycle_state = "running".into();
                status.transports = std::iter::once("local-cas".to_string())
                    .chain(network.transports)
                    .collect();
                status.listen_addresses = network.listen_addresses;
                status.peers = network.connected_peers;
                status.connected_peers = status.peers.len().min(u32::MAX as usize) as u32;
                status.routing_status = network.routing_status;
                status.bytes_up = status.bytes_up.saturating_add(network.bytes_up);
                status.bytes_down = status.bytes_down.saturating_add(network.bytes_down);
                status.limitations.clear();
                status.last_error = None;
            }
            Err(error) => {
                status.lifecycle_state = "degraded".into();
                status.routing_status = "embedded_node_unavailable".into();
                status.last_error = Some(jimmusic_protocol::ErrorEnvelopeV1 {
                    schema_version: SCHEMA_V1,
                    code: "p2p_status_failed".into(),
                    message: error.to_string(),
                    subsystem: "node".into(),
                    operation: "status".into(),
                    retryable: true,
                    unsupported_reason: None,
                    details: BTreeMap::new(),
                    request_id: None,
                    causes: Vec::new(),
                });
            }
        }
        status
    }

    /// 设置控制面 API 令牌（`None` 表示关闭鉴权）。
    pub fn with_api_token(mut self, token: Option<String>) -> Self {
        self.api_token = Arc::new(token);
        self
    }

    /// 校验给定 `Authorization` 头是否通过节点认证。
    pub fn authorize_header(&self, header: Option<&str>) -> bool {
        crate::auth::authorize_token(
            self.api_token.as_deref(),
            crate::auth::bearer_from_header(header),
        )
    }

    /// 快照所有插件记录（按名字排序）。
    pub fn list_records(&self) -> Vec<PluginRecord> {
        let mut recs: Vec<PluginRecord> = self.legacy.snapshot().records.into_values().collect();
        recs.sort_by(|a, b| a.name.cmp(&b.name));
        recs
    }

    /// 查询单个记录。
    pub fn get_record(&self, name: &str) -> Option<PluginRecord> {
        self.legacy.snapshot().records.get(name).cloned()
    }

    /// 插入/更新记录。
    pub fn upsert_record(&self, record: PluginRecord) {
        let _ = self.legacy.transact(|state| {
            state.records.insert(record.name.clone(), record);
            Ok(())
        });
    }

    /// 删除记录并返回被删除者。
    pub fn remove_record(&self, name: &str) -> Option<PluginRecord> {
        self.legacy
            .transact(|state| Ok(state.records.remove(name)))
            .ok()
            .flatten()
    }

    /// 返回库文件在本地仓库中的绝对路径。
    pub fn lib_abs_path(&self, record: &PluginRecord) -> PathBuf {
        match &record.lib_path {
            Some(rel) => self.repo_dir.join(rel),
            None => self.repo_dir.join(format!("lib{}.so", record.name)),
        }
    }

    /// 列举指定 kind 的插件记录（`None` 表示不过滤）。
    pub fn list_by_kind(&self, kind: Option<&str>) -> Vec<PluginRecord> {
        let mut recs = self.list_records();
        if let Some(kind) = kind {
            recs.retain(|r| r.kind.eq_ignore_ascii_case(kind));
        }
        recs
    }

    /// 设置激活的音频输出插件（校验其为 `output` 类型）。
    pub fn activate_output(&self, name: &str) -> Result<(), String> {
        let rec = self
            .get_record(name)
            .ok_or_else(|| format!("plugin `{name}` not found"))?;
        if !rec.kind.eq_ignore_ascii_case("output") {
            return Err(format!("plugin `{name}` is not an output plugin"));
        }
        self.legacy
            .transact(|state| {
                state.active_output = Some(name.to_string());
                Ok(())
            })
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    /// 当前激活的音频输出插件名。
    pub fn active_output(&self) -> Option<String> {
        self.legacy.snapshot().active_output
    }
}

const OFFICIAL_BOOTSTRAP_PUBLIC_KEY: &str =
    "772c8a442b7db06e166cfbc1ccbcbcde6f3eba76a4e98ef3ffc519502237d6ef";

fn official_bootstrap_source() -> CommunitySourceManifestV1 {
    CommunitySourceManifestV1 {
        schema_version: SCHEMA_V1,
        source_id: "org.jimmusic.bootstrap".into(),
        name: "JimMusic Bootstrap".into(),
        description:
            "Built-in signed bootstrap source. Empty until an official feed is published.".into(),
        languages: vec!["en".into(), "zh-CN".into()],
        maintainer_identity_cid: "bafkreif74jimmusicbootstrapidentity".into(),
        catalog_head: None,
        policy_head: None,
        supported_schemas: vec![SCHEMA_V1],
        report_endpoint: None,
        report_encryption_public_key: None,
        updated_at: 1_787_644_800,
        signature: Some(
            "ecaf27e4913ed6240754992cbc5dce109fbe2a86a7b06c79486d520c377bd16ba405f7f44f10a1b684b08b710b5520772f13c00e26f28c8831ff657fee84260d"
                .into(),
        ),
    }
}

fn default_audio_graph() -> AudioGraphSpecV1 {
    let format = AudioFormatSpecV1 {
        media_type: AudioMediaType::Pcm,
        sample_type: "f32".into(),
        sample_rate: 48_000,
        channels: 2,
        channel_layout: "stereo".into(),
        packing: "planar".into(),
        endian: "not_applicable".into(),
        bit_exact: false,
    };
    AudioGraphSpecV1 {
        schema_version: SCHEMA_V1,
        graph_id: "core-default".into(),
        version: 1,
        created_by: "core".into(),
        nodes: vec![
            AudioNodeSpecV1 {
                node_id: "decoder".into(),
                node_type: AudioNodeType::Decoder,
                plugin_id: "core.decoder".into(),
                plugin_version: "2.0.0".into(),
                inputs: Vec::new(),
                outputs: vec![AudioPortSpecV1 {
                    port_id: "out".into(),
                    media_type: AudioMediaType::Pcm,
                    format: format.clone(),
                }],
                latency_frames: 0,
                tail_frames: 0,
                realtime_safe: true,
                failure_policy: NodeFailurePolicy::Stop,
                state_cid: None,
            },
            AudioNodeSpecV1 {
                node_id: "output".into(),
                node_type: AudioNodeType::Output,
                plugin_id: "core.output-adapter-v1".into(),
                plugin_version: "1.0.0".into(),
                inputs: vec![AudioPortSpecV1 {
                    port_id: "in".into(),
                    media_type: AudioMediaType::Pcm,
                    format,
                }],
                outputs: Vec::new(),
                latency_frames: 512,
                tail_frames: 0,
                realtime_safe: true,
                failure_policy: NodeFailurePolicy::Stop,
                state_cid: None,
            },
        ],
        edges: vec![AudioEdgeSpecV1 {
            from_node: "decoder".into(),
            from_port: "out".into(),
            to_node: "output".into(),
            to_port: "in".into(),
        }],
        output_node: "output".into(),
        mode: AudioGraphMode::Normal,
        allow_format_conversion: true,
        cpu_budget_micros: 5_000,
        memory_budget_bytes: 8 * 1024 * 1024,
        latency_budget_frames: 8_192,
    }
}

/// 计算文件的 SHA-256 十六进制摘要（用于安装后/加载前的完整性校验）。
#[allow(dead_code)] // 供 future 在加载前校验盘上库文件使用
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(sha256_hex(&bytes))
}

/// 计算字节的 SHA-256 十六进制摘要。
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn comma_separated(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_is_stable_and_correct() {
        // 已知 SHA-256("abc") = ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
        let digest = sha256_hex(b"abc");
        assert_eq!(
            digest,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn upsert_and_list() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(
            dir.path().to_string_lossy().into_owned(),
            "http://127.0.0.1:5001".into(),
        )
        .unwrap();
        let rec = PluginRecord {
            name: "test".into(),
            version: "1.0".into(),
            author: "a".into(),
            kind: "decoder".into(),
            source: PluginSource::Local,
            location: None,
            sha256: None,
            lib_path: None,
        };
        state.upsert_record(rec.clone());
        assert_eq!(state.list_records().len(), 1);
        assert!(state.get_record("test").is_some());
        state.remove_record("test");
        assert!(state.get_record("test").is_none());
    }

    #[test]
    fn signed_bootstrap_source_can_be_disabled_or_removed_persistently() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_string_lossy().into_owned();
        let state = AppState::new(path.clone(), "http://127.0.0.1:5001".into()).unwrap();
        let source = state
            .community
            .list_sources()
            .into_iter()
            .find(|source| source.manifest.source_id == "org.jimmusic.bootstrap")
            .unwrap();
        assert!(source.bootstrap);
        state
            .community
            .set_enabled("org.jimmusic.bootstrap", false, false)
            .unwrap();
        drop(state);

        let state = AppState::new(path.clone(), "http://127.0.0.1:5001".into()).unwrap();
        let source = state
            .community
            .list_sources()
            .into_iter()
            .find(|source| source.manifest.source_id == "org.jimmusic.bootstrap")
            .unwrap();
        assert!(!source.catalog_enabled);
        assert!(!source.policy_enabled);
        state
            .community
            .remove_source("org.jimmusic.bootstrap")
            .unwrap();
        drop(state);

        let reopened = AppState::new(path, "http://127.0.0.1:5001".into()).unwrap();
        assert!(reopened
            .community
            .list_sources()
            .iter()
            .all(|source| source.manifest.source_id != "org.jimmusic.bootstrap"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn embedded_node_uses_stable_identity_and_reports_real_transports() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(
            dir.path().to_string_lossy().into_owned(),
            "http://127.0.0.1:1".into(),
        )
        .unwrap();
        state.start_embedded_node().await.unwrap();
        let status = state.node_status().await;
        assert_eq!(status.peer_id, state.node_identity.peer_id_str());
        assert!(status.transports.contains(&"bitswap".to_string()));
        assert!(status.transports.contains(&"quic-v1".to_string()));
        assert!(!status.listen_addresses.is_empty());
        assert!(status.limitations.is_empty());
        state.embedded_node().await.unwrap().shutdown().await;
    }
}
