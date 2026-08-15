//! 事务式插件生命周期、权限、兼容性、回滚、撤销与安全模式。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use app_core::crypto::{verify_ed25519_hex, SignatureError};
use app_core::node_service::RAW_CODEC;
use app_core::storage::{AtomicJsonStore, StorageError};
use jimmusic_protocol::{
    cid_v1_for_bytes, PluginArtifactV1, PluginDependencyV1, PluginLifecycleState, PluginManifestV1,
    PluginPermission, PluginRuntime, Validate, SCHEMA_V1,
};
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

use crate::state::sha256_hex;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PluginTrustChannel {
    OfficialNative,
    CommunitySandbox,
    CommunityNativeAdvanced,
    Bundled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledPluginVersion {
    pub version: String,
    pub manifest_cid: String,
    pub artifact: PluginArtifactV1,
    pub install_path: String,
    pub installed_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginRuntimeRecord {
    pub plugin_id: String,
    pub name: String,
    pub publisher: String,
    pub kind: String,
    pub lifecycle_state: PluginLifecycleState,
    pub trust_channel: PluginTrustChannel,
    pub active_version: Option<String>,
    pub rollback_version: Option<String>,
    pub available_version: Option<String>,
    pub permissions_declared: BTreeSet<PluginPermission>,
    pub permissions_granted: BTreeSet<PluginPermission>,
    #[serde(default)]
    pub dependencies: Vec<PluginDependencyV1>,
    #[serde(default)]
    pub conflicts: BTreeSet<String>,
    pub configuration: serde_json::Value,
    pub configuration_schema_cid: String,
    pub state_schema_version: u16,
    /// PLG-011：升级跨越 state_schema_version 时保留的旧配置（审计/回滚参考）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_configuration: Option<PreviousConfiguration>,
    pub versions: BTreeMap<String, InstalledPluginVersion>,
    pub consecutive_failures: u32,
    pub last_error: Option<String>,
    pub updated_at: i64,
}

/// PLG-011：跨状态 Schema 版本升级时，旧配置按其原 schema 版本封存，
/// 供审计、回滚与人工迁移参考；新版本从新 Schema 默认值开始。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PreviousConfiguration {
    pub state_schema_version: u16,
    pub configuration: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginAuditEvent {
    pub timestamp: i64,
    pub plugin_id: String,
    pub action: String,
    pub result: String,
    pub request_id: Option<String>,
    pub details: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PluginRepositoryState {
    schema_version: u16,
    plugins: BTreeMap<String, PluginRuntimeRecord>,
    idempotency: BTreeMap<String, String>,
    official_publishers: BTreeSet<String>,
    revoked_releases: BTreeSet<String>,
    service_owners: BTreeMap<String, String>,
    safe_mode: bool,
    audit: Vec<PluginAuditEvent>,
    #[serde(default)]
    install_journal: BTreeMap<String, InstallJournalEntryV1>,
}

/// 插件安装中间态日志（PLG-013）：下载/验证/暂存/提交等阶段持久可观测，
/// 崩溃中断的安装重启后标记为 interrupted。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstallJournalEntryV1 {
    pub schema_version: u16,
    pub request_id: String,
    pub plugin_id: String,
    pub version: String,
    pub stage: String,
    pub started_at: i64,
    pub updated_at: i64,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct InstallContext {
    pub request_id: String,
    pub platform: String,
    pub architecture: String,
    pub core_version: String,
    pub public_key: String,
    pub granted_permissions: BTreeSet<PluginPermission>,
    pub allow_community_native: bool,
    /// PLG-011：新状态 Schema 的默认配置（由调用方从 configuration_schema_cid
    /// 解析）；仅当升级跨越 state_schema_version 时使用。
    pub configuration_defaults: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct PluginInstallOutcome {
    pub record: PluginRuntimeRecord,
    pub artifact_path: PathBuf,
    pub idempotent_replay: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum PluginLifecycleError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Signature(#[from] SignatureError),
    #[error("invalid plugin manifest: {0}")]
    InvalidManifest(String),
    #[error("manifest signature is required")]
    MissingSignature,
    #[error("plugin `{0}` is not installed")]
    NotFound(String),
    #[error("no artifact supports platform `{platform}` architecture `{architecture}`")]
    Incompatible {
        platform: String,
        architecture: String,
    },
    #[error("plugin requires core range {minimum}..={maximum}; current core is {current}")]
    CoreVersion {
        minimum: String,
        maximum: String,
        current: String,
    },
    #[error("artifact byte length mismatch")]
    LengthMismatch,
    #[error("artifact digest mismatch")]
    DigestMismatch,
    #[error("artifact CID mismatch")]
    CidMismatch,
    #[error("permissions were not granted: {0:?}")]
    Permissions(BTreeSet<PluginPermission>),
    #[error("required plugin dependency `{0}` is not installed")]
    DependencyMissing(String),
    #[error("plugin dependency `{plugin}` requires `{requirement}`, active version is `{actual}`")]
    DependencyVersion {
        plugin: String,
        requirement: String,
        actual: String,
    },
    #[error("plugin conflicts with installed plugin `{0}`")]
    Conflict(String),
    #[error("community native plugins require explicit advanced authorization")]
    CommunityNativeDenied,
    #[error("plugin entrypoint is not a safe relative path")]
    UnsafeEntrypoint,
    #[error("plugin release is revoked")]
    Revoked,
    #[error("plugin is quarantined; disable it or leave safe mode before enabling")]
    Quarantined,
    #[error("no rollback version is available")]
    NoRollback,
    #[error("configuration must be a JSON object")]
    InvalidConfiguration,
    #[error("state_schema_version downgrade from {from} to {to} is not allowed")]
    StateSchemaDowngrade { from: u16, to: u16 },
    #[error("plugins cannot replace trusted microkernel service `{0}`")]
    TrustedService(String),
    #[error("service `{0}` is already registered by another plugin")]
    ServiceConflict(String),
    #[error("plugin repository IO failed: {0}")]
    Io(#[from] std::io::Error),
}

pub struct PluginLifecycleService {
    versions_dir: PathBuf,
    staging_dir: PathBuf,
    store: AtomicJsonStore<PluginRepositoryState>,
    install_lock: Mutex<()>,
}

impl PluginLifecycleService {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, PluginLifecycleError> {
        let root = root.into();
        let versions_dir = root.join("versions");
        let staging_dir = root.join("staging");
        std::fs::create_dir_all(&versions_dir)?;
        std::fs::create_dir_all(&staging_dir)?;
        let store = AtomicJsonStore::open(
            root.join("lifecycle.json"),
            PluginRepositoryState {
                schema_version: SCHEMA_V1,
                plugins: BTreeMap::new(),
                idempotency: BTreeMap::new(),
                official_publishers: BTreeSet::new(),
                revoked_releases: BTreeSet::new(),
                service_owners: BTreeMap::new(),
                safe_mode: false,
                audit: Vec::new(),
                install_journal: BTreeMap::new(),
            },
        )?;
        // PLG-013：崩溃中断的安装标记为 interrupted（保留观测）。
        store
            .transact(|state| {
                for entry in state.install_journal.values_mut() {
                    if !matches!(entry.stage.as_str(), "failed" | "interrupted") {
                        entry.stage = "interrupted".into();
                        entry.error = Some("install interrupted by process exit".into());
                        entry.updated_at = now();
                    }
                }
                Ok(())
            })
            .map_err(std::io::Error::other)?;
        // 未完成 staging 从未成为活动版本，可安全清理普通文件/目录。
        for entry in std::fs::read_dir(&staging_dir)? {
            let path = entry?.path();
            if path.is_dir() {
                std::fs::remove_dir_all(path)?;
            } else {
                std::fs::remove_file(path)?;
            }
        }
        // 事务可能在“制品原子替换”与“状态提交”之间遭遇进程崩溃。启动时仅保留
        // lifecycle.json 明确引用的版本目录，避免孤儿制品被误加载。
        let referenced: BTreeSet<PathBuf> = store
            .snapshot()
            .plugins
            .values()
            .flat_map(|record| record.versions.values())
            .filter_map(|version| Path::new(&version.install_path).parent())
            .map(Path::to_path_buf)
            .collect();
        for plugin_entry in std::fs::read_dir(&versions_dir)? {
            let plugin_path = plugin_entry?.path();
            if !plugin_path.is_dir() {
                continue;
            }
            for version_entry in std::fs::read_dir(&plugin_path)? {
                let version_path = version_entry?.path();
                if version_path.is_dir() && !referenced.contains(&version_path) {
                    std::fs::remove_dir_all(version_path)?;
                }
            }
        }
        Ok(Self {
            versions_dir,
            staging_dir,
            store,
            install_lock: Mutex::new(()),
        })
    }

    pub fn add_official_publisher(&self, publisher: String) -> Result<(), PluginLifecycleError> {
        self.store.transact(|state| {
            state.official_publishers.insert(publisher);
            Ok(())
        })?;
        Ok(())
    }

    pub fn list(&self) -> Vec<PluginRuntimeRecord> {
        self.store.snapshot().plugins.into_values().collect()
    }

    pub fn get(&self, plugin_id: &str) -> Option<PluginRuntimeRecord> {
        self.store.snapshot().plugins.get(plugin_id).cloned()
    }

    /// 在下载制品前完成所有仅依赖 Manifest/本机状态的判定，并验证 Manifest 签名。
    pub fn preflight(
        &self,
        manifest: &PluginManifestV1,
        context: &InstallContext,
    ) -> Result<(PluginArtifactV1, PluginTrustChannel), PluginLifecycleError> {
        manifest
            .validate()
            .map_err(|error| PluginLifecycleError::InvalidManifest(error.to_string()))?;
        let artifact = manifest
            .compatible_artifact(&context.platform, &context.architecture)
            .cloned()
            .ok_or_else(|| PluginLifecycleError::Incompatible {
                platform: context.platform.clone(),
                architecture: context.architecture.clone(),
            })?;
        check_core_version(
            &context.core_version,
            &manifest.minimum_core_version,
            &manifest.maximum_core_version,
        )?;
        verify_ed25519_hex(
            &context.public_key,
            manifest
                .signature
                .as_deref()
                .ok_or(PluginLifecycleError::MissingSignature)?,
            &manifest
                .unsigned_bytes()
                .map_err(|error| PluginLifecycleError::InvalidManifest(error.to_string()))?,
        )?;
        let manifest_cid = jimmusic_protocol::cid_v1_for(manifest)
            .map_err(|error| PluginLifecycleError::InvalidManifest(error.to_string()))?;
        let snapshot = self.store.snapshot();
        if snapshot.revoked_releases.contains(&manifest_cid) || manifest.revoked_at.is_some() {
            return Err(PluginLifecycleError::Revoked);
        }
        let denied: BTreeSet<_> = manifest
            .permissions
            .difference(&context.granted_permissions)
            .copied()
            .collect();
        if !denied.is_empty() {
            return Err(PluginLifecycleError::Permissions(denied));
        }
        for dependency in &manifest.dependencies {
            let Some(record) = snapshot.plugins.get(&dependency.plugin_id) else {
                if dependency.optional {
                    continue;
                }
                return Err(PluginLifecycleError::DependencyMissing(
                    dependency.plugin_id.clone(),
                ));
            };
            let actual = record.active_version.as_deref().unwrap_or_default();
            if !version_matches(actual, &dependency.version_requirement) {
                if dependency.optional {
                    continue;
                }
                return Err(PluginLifecycleError::DependencyVersion {
                    plugin: dependency.plugin_id.clone(),
                    requirement: dependency.version_requirement.clone(),
                    actual: actual.into(),
                });
            }
        }
        for conflict in &manifest.conflicts {
            if snapshot.plugins.contains_key(conflict) {
                return Err(PluginLifecycleError::Conflict(conflict.clone()));
            }
        }
        if let Some(conflict) = snapshot
            .plugins
            .values()
            .find(|record| record.conflicts.contains(&manifest.plugin_id))
        {
            return Err(PluginLifecycleError::Conflict(conflict.plugin_id.clone()));
        }
        let official = snapshot.official_publishers.contains(&manifest.publisher);
        let trust_channel = match artifact.runtime {
            PluginRuntime::Native if official => PluginTrustChannel::OfficialNative,
            PluginRuntime::Native if context.allow_community_native => {
                PluginTrustChannel::CommunityNativeAdvanced
            }
            PluginRuntime::Native => return Err(PluginLifecycleError::CommunityNativeDenied),
            PluginRuntime::Declarative | PluginRuntime::Wasm | PluginRuntime::Service => {
                PluginTrustChannel::CommunitySandbox
            }
        };
        validate_entrypoint(&artifact.entrypoint)?;
        Ok((artifact, trust_channel))
    }

    /// PLG-013：安装日志（中间态持久化）——开始一次安装。
    pub fn journal_begin(
        &self,
        request_id: &str,
        plugin_id: &str,
        version: &str,
    ) -> Result<(), PluginLifecycleError> {
        let timestamp = now();
        self.store.transact(|state| {
            // 上限 64 条：丢弃最旧条目，避免无限增长。
            if state.install_journal.len() >= 64 && !state.install_journal.contains_key(request_id)
            {
                if let Some(oldest) = state
                    .install_journal
                    .iter()
                    .min_by_key(|(_, entry)| entry.updated_at)
                    .map(|(key, _)| key.clone())
                {
                    state.install_journal.remove(&oldest);
                }
            }
            state.install_journal.insert(
                request_id.into(),
                InstallJournalEntryV1 {
                    schema_version: SCHEMA_V1,
                    request_id: request_id.into(),
                    plugin_id: plugin_id.into(),
                    version: version.into(),
                    stage: "downloading".into(),
                    started_at: timestamp,
                    updated_at: timestamp,
                    error: None,
                },
            );
            Ok(())
        })?;
        Ok(())
    }

    /// PLG-013：更新安装日志阶段。
    pub fn journal_stage(&self, request_id: &str, stage: &str) -> Result<(), PluginLifecycleError> {
        self.store.transact(|state| {
            if let Some(entry) = state.install_journal.get_mut(request_id) {
                entry.stage = stage.into();
                entry.updated_at = now();
            }
            Ok(())
        })?;
        Ok(())
    }

    /// PLG-013：安装失败时记录结构化错误（条目保留供观测）。
    pub fn journal_fail(&self, request_id: &str, error: &str) -> Result<(), PluginLifecycleError> {
        self.store.transact(|state| {
            if let Some(entry) = state.install_journal.get_mut(request_id) {
                entry.stage = "failed".into();
                entry.error = Some(error.chars().take(500).collect());
                entry.updated_at = now();
            }
            Ok(())
        })?;
        Ok(())
    }

    /// PLG-013：安装成功，移除日志。
    pub fn journal_finish(&self, request_id: &str) -> Result<(), PluginLifecycleError> {
        self.store.transact(|state| {
            state.install_journal.remove(request_id);
            Ok(())
        })?;
        Ok(())
    }

    /// PLG-005：发布是否已被撤销（目录详情/升级判定用）。
    pub fn is_release_revoked(&self, manifest_cid: &str) -> bool {
        self.store
            .snapshot()
            .revoked_releases
            .contains(manifest_cid)
    }

    /// PLG-013：当前安装日志（下载/验证/暂存/提交/失败/中断）。
    pub fn install_journal(&self) -> Vec<InstallJournalEntryV1> {
        self.store
            .snapshot()
            .install_journal
            .into_values()
            .collect()
    }

    /// 完整安装事务：兼容性预判 → Manifest 验签 → 权限判定 → 制品 CID/摘要校验 →
    /// staging fsync → 原子激活。任何失败都保留旧活动版本。
    pub fn install(
        &self,
        manifest: PluginManifestV1,
        artifact_bytes: &[u8],
        context: InstallContext,
    ) -> Result<PluginInstallOutcome, PluginLifecycleError> {
        let _guard = self.install_lock.lock().expect("install lock poisoned");
        if let Some(plugin_id) = self.store.snapshot().idempotency.get(&context.request_id) {
            if let Some(record) = self.get(plugin_id) {
                let version = record
                    .active_version
                    .as_ref()
                    .and_then(|version| record.versions.get(version))
                    .ok_or_else(|| PluginLifecycleError::NotFound(plugin_id.clone()))?;
                return Ok(PluginInstallOutcome {
                    artifact_path: PathBuf::from(&version.install_path),
                    record,
                    idempotent_replay: true,
                });
            }
        }
        // PLG-013：安装日志（verifying → staging → committing），失败保留观测。
        let request_id = context.request_id.clone();
        self.journal_begin(&request_id, &manifest.plugin_id, &manifest.version)?;
        self.journal_stage(&request_id, "verifying")?;
        let result = self.install_inner(manifest, artifact_bytes, context);
        match &result {
            Ok(_) => {
                let _ = self.journal_finish(&request_id);
            }
            Err(error) => {
                let _ = self.journal_fail(&request_id, &error.to_string());
            }
        }
        result
    }

    fn install_inner(
        &self,
        manifest: PluginManifestV1,
        artifact_bytes: &[u8],
        context: InstallContext,
    ) -> Result<PluginInstallOutcome, PluginLifecycleError> {
        let (artifact, trust_channel) = self.preflight(&manifest, &context)?;
        // PLG-011：状态 Schema 降级拒绝——与存储层拒绝未来 schema 的策略一致。
        if let Some(existing) = self
            .store
            .snapshot()
            .plugins
            .get(&manifest.plugin_id)
            .cloned()
        {
            if existing.state_schema_version > manifest.state_schema_version {
                return Err(PluginLifecycleError::StateSchemaDowngrade {
                    from: existing.state_schema_version,
                    to: manifest.state_schema_version,
                });
            }
        }
        let manifest_cid = jimmusic_protocol::cid_v1_for(&manifest)
            .map_err(|error| PluginLifecycleError::InvalidManifest(error.to_string()))?;
        if artifact.byte_length != artifact_bytes.len() as u64 {
            return Err(PluginLifecycleError::LengthMismatch);
        }
        if !artifact
            .sha256
            .eq_ignore_ascii_case(&sha256_hex(artifact_bytes))
        {
            return Err(PluginLifecycleError::DigestMismatch);
        }
        if artifact.artifact_cid != cid_v1_for_bytes(RAW_CODEC, artifact_bytes) {
            return Err(PluginLifecycleError::CidMismatch);
        }

        let _ = self.journal_stage(&context.request_id, "staging");
        let stage = self.staging_dir.join(safe_component(&context.request_id));
        if stage.exists() {
            std::fs::remove_dir_all(&stage)?;
        }
        let staged_file = stage.join(&artifact.entrypoint);
        std::fs::create_dir_all(staged_file.parent().expect("entrypoint has parent"))?;
        write_synced(&staged_file, artifact_bytes)?;

        let version_dir = self
            .versions_dir
            .join(safe_component(&manifest.plugin_id))
            .join(safe_component(&manifest.version));
        if let Some(parent) = version_dir.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let backup_dir = self.staging_dir.join(format!(
            "rollback-{}-{}",
            safe_component(&manifest.plugin_id),
            safe_component(&manifest.version)
        ));
        if backup_dir.exists() {
            std::fs::remove_dir_all(&backup_dir)?;
        }
        if version_dir.exists() {
            std::fs::rename(&version_dir, &backup_dir)?;
        }
        if let Err(error) = std::fs::rename(&stage, &version_dir) {
            if backup_dir.exists() {
                let _ = std::fs::rename(&backup_dir, &version_dir);
            }
            return Err(error.into());
        }
        let install_path = version_dir.join(&artifact.entrypoint);
        let timestamp = now();
        let installed_version = InstalledPluginVersion {
            version: manifest.version.clone(),
            manifest_cid,
            artifact,
            install_path: install_path.to_string_lossy().into_owned(),
            installed_at: timestamp,
        };

        let _ = self.journal_stage(&context.request_id, "committing");
        let transaction = self.store.transact(|state| {
            let existing = state.plugins.get(&manifest.plugin_id).cloned();
            let mut versions = existing
                .as_ref()
                .map(|record| record.versions.clone())
                .unwrap_or_default();
            versions.insert(manifest.version.clone(), installed_version.clone());
            let rollback_version = existing.as_ref().and_then(|record| {
                if record.active_version.as_deref() == Some(manifest.version.as_str()) {
                    record.rollback_version.clone()
                } else {
                    record.active_version.clone()
                }
            });
            // PLG-011：状态 Schema 迁移策略——
            // 同版本完整迁移配置；跨版本封存旧配置并从新 Schema 默认值开始。
            let previous_configuration = existing
                .as_ref()
                .filter(|record| record.state_schema_version != manifest.state_schema_version)
                .map(|record| PreviousConfiguration {
                    state_schema_version: record.state_schema_version,
                    configuration: record.configuration.clone(),
                });
            let configuration = match (&existing, &previous_configuration) {
                (Some(record), None) => record.configuration.clone(),
                (_, Some(_)) => context.configuration_defaults.clone(),
                (None, None) => serde_json::json!({}),
            };
            let record = PluginRuntimeRecord {
                plugin_id: manifest.plugin_id.clone(),
                name: manifest.name.clone(),
                publisher: manifest.publisher.clone(),
                kind: manifest.plugin_kind.clone(),
                lifecycle_state: PluginLifecycleState::Installed,
                trust_channel,
                active_version: Some(manifest.version.clone()),
                rollback_version,
                available_version: None,
                permissions_declared: manifest.permissions.clone(),
                permissions_granted: context.granted_permissions.clone(),
                dependencies: manifest.dependencies.clone(),
                conflicts: manifest.conflicts.iter().cloned().collect(),
                configuration,
                configuration_schema_cid: manifest.configuration_schema_cid.clone(),
                state_schema_version: manifest.state_schema_version,
                previous_configuration,
                versions,
                consecutive_failures: 0,
                last_error: None,
                updated_at: timestamp,
            };
            state
                .idempotency
                .insert(context.request_id.clone(), manifest.plugin_id.clone());
            state
                .plugins
                .insert(manifest.plugin_id.clone(), record.clone());
            push_audit(
                state,
                &manifest.plugin_id,
                "install",
                "installed",
                Some(context.request_id.clone()),
                BTreeMap::from([("version".into(), manifest.version.clone())]),
            );
            Ok(record)
        });
        let record = match transaction {
            Ok(record) => {
                if backup_dir.exists() {
                    std::fs::remove_dir_all(&backup_dir)?;
                }
                record
            }
            Err(error) => {
                let _ = std::fs::remove_dir_all(&version_dir);
                if backup_dir.exists() {
                    let _ = std::fs::rename(&backup_dir, &version_dir);
                }
                return Err(error.into());
            }
        };

        Ok(PluginInstallOutcome {
            record,
            artifact_path: install_path,
            idempotent_replay: false,
        })
    }

    pub fn enable(&self, plugin_id: &str) -> Result<PluginRuntimeRecord, PluginLifecycleError> {
        let revoked = self.store.snapshot().revoked_releases.clone();
        self.mutate_record(plugin_id, |record| {
            if matches!(
                record.lifecycle_state,
                PluginLifecycleState::Revoked | PluginLifecycleState::Quarantined
            ) {
                return Err(PluginLifecycleError::Quarantined);
            }
            // SEC-011：活动发布被撤销后，即使状态被改回 Disabled/Installed
            // 也不能重新启用（防止旧安全快照或手工状态绕过撤销）。
            let active_revoked = record
                .active_version
                .as_ref()
                .and_then(|version| record.versions.get(version))
                .is_some_and(|version| revoked.contains(&version.manifest_cid));
            if active_revoked {
                return Err(PluginLifecycleError::Revoked);
            }
            record.lifecycle_state = PluginLifecycleState::Enabled;
            record.consecutive_failures = 0;
            Ok(())
        })
    }

    pub fn disable(&self, plugin_id: &str) -> Result<PluginRuntimeRecord, PluginLifecycleError> {
        self.mutate_record(plugin_id, |record| {
            record.lifecycle_state = PluginLifecycleState::Disabled;
            Ok(())
        })
    }

    pub fn configure(
        &self,
        plugin_id: &str,
        configuration: serde_json::Value,
    ) -> Result<PluginRuntimeRecord, PluginLifecycleError> {
        if !configuration.is_object() {
            return Err(PluginLifecycleError::InvalidConfiguration);
        }
        self.mutate_record(plugin_id, move |record| {
            record.configuration = configuration;
            Ok(())
        })
    }

    pub fn revoke_permission(
        &self,
        plugin_id: &str,
        permission: PluginPermission,
    ) -> Result<PluginRuntimeRecord, PluginLifecycleError> {
        self.mutate_record(plugin_id, |record| {
            record.permissions_granted.remove(&permission);
            if record.permissions_declared.contains(&permission) {
                record.lifecycle_state = PluginLifecycleState::Disabled;
                record.last_error = Some(format!("required permission {permission:?} was revoked"));
            }
            Ok(())
        })
    }

    pub fn rollback(&self, plugin_id: &str) -> Result<PluginRuntimeRecord, PluginLifecycleError> {
        let revoked = self.store.snapshot().revoked_releases.clone();
        self.mutate_record(plugin_id, |record| {
            let previous = record
                .rollback_version
                .clone()
                .ok_or(PluginLifecycleError::NoRollback)?;
            let previous_version = record
                .versions
                .get(&previous)
                .ok_or(PluginLifecycleError::NoRollback)?;
            // SEC-011：撤销 Feed 快照防回滚——回滚目标发布被撤销时拒绝，
            // 防止重新启用已知恶意/被撤销版本。
            if revoked.contains(&previous_version.manifest_cid) {
                return Err(PluginLifecycleError::Revoked);
            }
            record.rollback_version = record.active_version.replace(previous);
            record.lifecycle_state = PluginLifecycleState::Installed;
            Ok(())
        })
    }

    pub fn revoke_release(
        &self,
        manifest_cid: &str,
    ) -> Result<Vec<PluginRuntimeRecord>, PluginLifecycleError> {
        Ok(self.store.transact(|state| {
            state.revoked_releases.insert(manifest_cid.into());
            let mut revoked = Vec::new();
            for record in state.plugins.values_mut() {
                let active_revoked = record
                    .active_version
                    .as_ref()
                    .and_then(|version| record.versions.get(version))
                    .is_some_and(|version| version.manifest_cid == manifest_cid);
                if active_revoked {
                    record.lifecycle_state = PluginLifecycleState::Revoked;
                    record.last_error = Some("active release was revoked".into());
                    revoked.push(record.clone());
                }
            }
            Ok(revoked)
        })?)
    }

    pub fn record_failure(
        &self,
        plugin_id: &str,
        error: String,
    ) -> Result<PluginRuntimeRecord, PluginLifecycleError> {
        let record = self.mutate_record(plugin_id, |record| {
            record.consecutive_failures = record.consecutive_failures.saturating_add(1);
            record.last_error = Some(error);
            if record.consecutive_failures >= 3 {
                record.lifecycle_state = PluginLifecycleState::Quarantined;
            }
            Ok(())
        })?;
        if record.lifecycle_state == PluginLifecycleState::Quarantined {
            self.store.transact(|state| {
                state.safe_mode = true;
                Ok(())
            })?;
        }
        Ok(record)
    }

    pub fn safe_mode(&self) -> bool {
        self.store.snapshot().safe_mode
    }

    pub fn register_service(
        &self,
        plugin_id: &str,
        service: &str,
    ) -> Result<(), PluginLifecycleError> {
        const TRUSTED: &[&str] = &[
            "identity",
            "crypto",
            "permission",
            "storage_isolation",
            "plugin_loader",
            "revocation",
            "safe_mode",
            "event_router",
        ];
        if TRUSTED.contains(&service) {
            return Err(PluginLifecycleError::TrustedService(service.into()));
        }
        if self.get(plugin_id).is_none() {
            return Err(PluginLifecycleError::NotFound(plugin_id.into()));
        }
        let result = self.store.transact(|state| {
            if state
                .service_owners
                .get(service)
                .is_some_and(|owner| owner != plugin_id)
            {
                return Err(StorageError::Corrupt {
                    path: PathBuf::from("service-registry"),
                    reason: format!("service `{service}` already owned"),
                });
            }
            state
                .service_owners
                .insert(service.into(), plugin_id.into());
            Ok(())
        });
        match result {
            Ok(()) => Ok(()),
            Err(error) if error.to_string().contains("already owned") => {
                Err(PluginLifecycleError::ServiceConflict(service.into()))
            }
            Err(error) => Err(error.into()),
        }
    }

    pub fn uninstall(&self, plugin_id: &str) -> Result<(), PluginLifecycleError> {
        let _guard = self.install_lock.lock().expect("install lock poisoned");
        let record = self
            .get(plugin_id)
            .ok_or_else(|| PluginLifecycleError::NotFound(plugin_id.into()))?;
        self.store.transact(|state| {
            state.plugins.remove(plugin_id);
            state.idempotency.retain(|_, value| value != plugin_id);
            state.service_owners.retain(|_, owner| owner != plugin_id);
            push_audit(
                state,
                plugin_id,
                "uninstall",
                "removed",
                None,
                BTreeMap::new(),
            );
            Ok(())
        })?;
        for version in record.versions.values() {
            if let Some(directory) = Path::new(&version.install_path).parent() {
                if directory.starts_with(&self.versions_dir) && directory.exists() {
                    std::fs::remove_dir_all(directory)?;
                }
            }
        }
        Ok(())
    }

    pub fn audit(&self) -> Vec<PluginAuditEvent> {
        self.store.snapshot().audit
    }

    fn mutate_record(
        &self,
        plugin_id: &str,
        update: impl FnOnce(&mut PluginRuntimeRecord) -> Result<(), PluginLifecycleError>,
    ) -> Result<PluginRuntimeRecord, PluginLifecycleError> {
        if self.get(plugin_id).is_none() {
            return Err(PluginLifecycleError::NotFound(plugin_id.into()));
        }
        let mut update = Some(update);
        let mut domain_error: Option<PluginLifecycleError> = None;
        let result = self.store.transact(|state| {
            let record = state.plugins.get_mut(plugin_id).expect("checked above");
            if let Err(error) = update.take().expect("called once")(record) {
                domain_error = Some(error);
                return Err(StorageError::Corrupt {
                    path: PathBuf::from("plugin-lifecycle"),
                    reason: "domain update rejected".into(),
                });
            }
            record.updated_at = now();
            Ok(record.clone())
        });
        match result {
            Ok(record) => Ok(record),
            Err(error) => {
                // 保留域错误（Revoked/NoRollback/Quarantined 等），
                // 不要用字符串匹配把语义压平。
                if let Some(domain) = domain_error.take() {
                    return Err(domain);
                }
                Err(PluginLifecycleError::Storage(error))
            }
        }
    }
}

fn check_core_version(
    current: &str,
    minimum: &str,
    maximum: &str,
) -> Result<(), PluginLifecycleError> {
    let Some(current_value) = parse_version(current) else {
        return Err(PluginLifecycleError::CoreVersion {
            minimum: minimum.into(),
            maximum: maximum.into(),
            current: current.into(),
        });
    };
    let min = parse_version_bound(minimum, false).unwrap_or((u64::MAX, 0, 0));
    let max = parse_version_bound(maximum, true).unwrap_or((0, 0, 0));
    if current_value < min || current_value > max {
        Err(PluginLifecycleError::CoreVersion {
            minimum: minimum.into(),
            maximum: maximum.into(),
            current: current.into(),
        })
    } else {
        Ok(())
    }
}

fn version_matches(actual: &str, requirement: &str) -> bool {
    let normalize = |value: &str| {
        let value = value.trim().trim_start_matches('v');
        match value.matches('.').count() {
            0 => format!("{value}.0.0"),
            1 => format!("{value}.0"),
            _ => value.to_string(),
        }
    };
    let Ok(actual) = Version::parse(&normalize(actual)) else {
        return false;
    };
    VersionReq::parse(requirement)
        .map(|requirement| requirement.matches(&actual))
        .unwrap_or(false)
}

fn parse_version(value: &str) -> Option<(u64, u64, u64)> {
    let stable = value
        .trim()
        .trim_start_matches('v')
        .split(['-', '+'])
        .next()?;
    let mut parts = stable.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

fn parse_version_bound(value: &str, upper: bool) -> Option<(u64, u64, u64)> {
    let stable = value
        .trim()
        .trim_start_matches('v')
        .split(['-', '+'])
        .next()?;
    let parts: Vec<_> = stable.split('.').collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    let wildcard = |part: &str| part.eq_ignore_ascii_case("x") || part == "*";
    let component = |index: usize| -> Option<u64> {
        match parts.get(index) {
            Some(part) if wildcard(part) => Some(if upper { u64::MAX } else { 0 }),
            Some(part) => part.parse().ok(),
            None => Some(if upper { u64::MAX } else { 0 }),
        }
    };
    let major = component(0)?;
    let minor = if wildcard(parts[0]) {
        component(1).map(|_| major)?
    } else {
        component(1)?
    };
    let patch = if wildcard(parts[0]) || parts.get(1).is_some_and(|part| wildcard(part)) {
        if upper {
            u64::MAX
        } else {
            0
        }
    } else {
        component(2)?
    };
    Some((major, minor, patch))
}

fn validate_entrypoint(entrypoint: &str) -> Result<(), PluginLifecycleError> {
    let path = Path::new(entrypoint);
    if entrypoint.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(PluginLifecycleError::UnsafeEntrypoint);
    }
    Ok(())
}

fn safe_component(value: &str) -> String {
    let digest = sha256_hex(value.as_bytes());
    format!("{}-{}", sanitize(value), &digest[..12])
}

fn sanitize(value: &str) -> String {
    let clean: String = value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        .take(48)
        .collect();
    if clean.is_empty() {
        "item".into()
    } else {
        clean
    }
}

fn write_synced(path: &Path, bytes: &[u8]) -> Result<(), std::io::Error> {
    use std::io::Write;
    let mut file = std::fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn push_audit(
    state: &mut PluginRepositoryState,
    plugin_id: &str,
    action: &str,
    result: &str,
    request_id: Option<String>,
    details: BTreeMap<String, String>,
) {
    state.audit.push(PluginAuditEvent {
        timestamp: now(),
        plugin_id: plugin_id.into(),
        action: action.into(),
        result: result.into(),
        request_id,
        details,
    });
    if state.audit.len() > 10_000 {
        state.audit.drain(..1_000);
    }
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
    use ed25519_dalek::{Signer, SigningKey};
    use jimmusic_protocol::cid_v1_for;

    #[test]
    fn state_schema_migration_preserves_config_migrates_on_bump_and_rejects_downgrade() {
        use jimmusic_protocol::cid_v1_for_bytes;

        let dir = tempfile::tempdir().unwrap();
        let service = PluginLifecycleService::open(dir.path()).unwrap();
        service
            .add_official_publisher("org.example".into())
            .unwrap();
        let key = SigningKey::from_bytes(&[41; 32]);
        let artifact = b"migration-artifact".to_vec();

        let build = |version: &str, state_schema_version: u16| {
            let mut manifest = PluginManifestV1 {
                schema_version: SCHEMA_V1,
                plugin_id: "org.example.migrate".into(),
                name: "Migrate".into(),
                version: version.into(),
                publisher: "org.example".into(),
                plugin_kind: "audio_processor".into(),
                interface_versions: BTreeMap::from([("audio_processor".into(), "2".into())]),
                minimum_core_version: "2.0.0".into(),
                maximum_core_version: "2.9.9".into(),
                artifacts: vec![PluginArtifactV1 {
                    artifact_cid: cid_v1_for_bytes(RAW_CODEC, &artifact),
                    platform: "linux".into(),
                    architecture: "x86_64".into(),
                    runtime: PluginRuntime::Native,
                    entrypoint: "libmigrate.so".into(),
                    byte_length: artifact.len() as u64,
                    sha256: sha256_hex(&artifact),
                    provenance_cid: None,
                    sbom_cid: Some("bafysbom".into()),
                    sandbox_profile: "official-native".into(),
                    required_host_capabilities: vec!["audio_realtime".into()],
                    hardware_requirements: Vec::new(),
                }],
                capabilities: vec!["audio_processor".into()],
                permissions: BTreeSet::from([PluginPermission::AudioRealtime]),
                dependencies: Vec::new(),
                conflicts: Vec::new(),
                configuration_schema_cid: "bafyschema".into(),
                state_schema_version,
                license: "GPL-3.0-only".into(),
                release_notes_cid: None,
                previous_release_cid: None,
                signature: None,
                revoked_at: None,
            };
            manifest.signature = Some(hex::encode(
                key.sign(&manifest.unsigned_bytes().unwrap()).to_bytes(),
            ));
            manifest
        };

        let ctx = |request_id: &str, defaults: serde_json::Value| InstallContext {
            request_id: request_id.into(),
            platform: "linux".into(),
            architecture: "x86_64".into(),
            core_version: "2.0.0".into(),
            public_key: hex::encode(key.verifying_key().to_bytes()),
            granted_permissions: BTreeSet::from([PluginPermission::AudioRealtime]),
            allow_community_native: false,
            configuration_defaults: defaults,
        };

        // 安装 v1（schema 1）并配置。
        service
            .install(
                build("1.0.0", 1),
                &artifact,
                ctx("migrate-1", serde_json::json!({})),
            )
            .unwrap();
        service
            .configure("org.example.migrate", serde_json::json!({"gain": 5.0}))
            .unwrap();

        // 同 schema 升级 v1.1 → 配置完整迁移。
        service
            .install(
                build("1.1.0", 1),
                &artifact,
                ctx("migrate-2", serde_json::json!({})),
            )
            .unwrap();
        let record = service.get("org.example.migrate").unwrap();
        assert_eq!(record.configuration, serde_json::json!({"gain": 5.0}));
        assert!(record.previous_configuration.is_none());

        // 跨 schema 升级 v2（schema 2）→ 新 Schema 默认值 + 旧配置封存。
        service
            .install(
                build("2.0.0", 2),
                &artifact,
                ctx(
                    "migrate-3",
                    serde_json::json!({"gain": 1.0, "mode": "standard"}),
                ),
            )
            .unwrap();
        let record = service.get("org.example.migrate").unwrap();
        assert_eq!(record.state_schema_version, 2);
        assert_eq!(
            record.configuration,
            serde_json::json!({"gain": 1.0, "mode": "standard"})
        );
        let previous = record.previous_configuration.expect("schema bumped");
        assert_eq!(previous.state_schema_version, 1);
        assert_eq!(previous.configuration, serde_json::json!({"gain": 5.0}));

        // 状态 Schema 降级被拒绝。
        let error = service
            .install(
                build("1.2.0", 1),
                &artifact,
                ctx("migrate-4", serde_json::json!({})),
            )
            .unwrap_err();
        assert!(
            matches!(
                error,
                PluginLifecycleError::StateSchemaDowngrade { from: 2, to: 1 }
            ),
            "{error}"
        );
    }

    fn signed_manifest(key: &SigningKey, bytes: &[u8], version: &str) -> PluginManifestV1 {
        let mut manifest = PluginManifestV1 {
            schema_version: SCHEMA_V1,
            plugin_id: "org.example.output".into(),
            name: "Output".into(),
            version: version.into(),
            publisher: "org.example".into(),
            plugin_kind: "audio_output".into(),
            interface_versions: BTreeMap::from([("audio_output".into(), "2".into())]),
            minimum_core_version: "2.0.0".into(),
            maximum_core_version: "2.9.9".into(),
            artifacts: vec![PluginArtifactV1 {
                artifact_cid: cid_v1_for_bytes(RAW_CODEC, bytes),
                platform: "linux".into(),
                architecture: "x86_64".into(),
                runtime: PluginRuntime::Native,
                entrypoint: "liboutput.so".into(),
                byte_length: bytes.len() as u64,
                sha256: sha256_hex(bytes),
                provenance_cid: None,
                sbom_cid: Some("bafysbom".into()),
                sandbox_profile: "official-native".into(),
                required_host_capabilities: vec!["audio_device".into()],
                hardware_requirements: Vec::new(),
            }],
            capabilities: vec!["audio_output".into()],
            permissions: BTreeSet::from([PluginPermission::AudioDevice]),
            dependencies: Vec::new(),
            conflicts: Vec::new(),
            configuration_schema_cid: "bafyschema".into(),
            state_schema_version: 1,
            license: "GPL-3.0-only".into(),
            release_notes_cid: None,
            previous_release_cid: None,
            signature: None,
            revoked_at: None,
        };
        manifest.signature = Some(hex::encode(
            key.sign(&manifest.unsigned_bytes().unwrap()).to_bytes(),
        ));
        manifest
    }

    fn context(key: &SigningKey, request_id: &str) -> InstallContext {
        InstallContext {
            request_id: request_id.into(),
            platform: "linux".into(),
            architecture: "x86_64".into(),
            core_version: "2.0.0".into(),
            public_key: hex::encode(key.verifying_key().to_bytes()),
            granted_permissions: BTreeSet::from([PluginPermission::AudioDevice]),
            allow_community_native: false,
            configuration_defaults: serde_json::json!({}),
        }
    }

    #[test]
    fn install_journal_persists_failure_and_success_clears() {
        let dir = tempfile::tempdir().unwrap();
        let service = PluginLifecycleService::open(dir.path()).unwrap();
        service
            .add_official_publisher("org.example".into())
            .unwrap();
        let key = SigningKey::from_bytes(&[1; 32]);

        // 摘要被破坏（重新签名保证签名有效）：安装失败，日志保留 failed + 结构化错误。
        let mut manifest = signed_manifest(&key, b"v1", "1.0.0");
        manifest.artifacts[0].sha256 = "11".repeat(32);
        manifest.signature = Some(hex::encode(
            key.sign(&manifest.unsigned_bytes().unwrap()).to_bytes(),
        ));
        assert!(service
            .install(manifest, b"v1", context(&key, "req-fail"))
            .is_err());
        let journal = service.install_journal();
        assert_eq!(journal.len(), 1);
        assert_eq!(journal[0].stage, "failed");
        let error_text = journal[0].error.clone().unwrap_or_default();
        assert!(
            error_text.to_lowercase().contains("digest"),
            "unexpected journal error: {error_text:?}"
        );

        // 成功安装只清自己的日志；失败条目保留供观测（PLG-013）。
        service
            .install(
                signed_manifest(&key, b"v2", "2.0.0"),
                b"v2",
                context(&key, "req-ok"),
            )
            .unwrap();
        let journal = service.install_journal();
        assert_eq!(journal.len(), 1);
        assert_eq!(journal[0].request_id, "req-fail");
    }

    #[test]
    fn interrupted_install_is_marked_on_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let service = PluginLifecycleService::open(dir.path()).unwrap();
        service
            .journal_begin("req-crash", "org.example.crash", "9.9.9")
            .unwrap();
        drop(service);
        let reopened = PluginLifecycleService::open(dir.path()).unwrap();
        let journal = reopened.install_journal();
        assert_eq!(journal.len(), 1);
        assert_eq!(journal[0].stage, "interrupted");
        assert_eq!(journal[0].plugin_id, "org.example.crash");
    }

    #[test]
    fn signed_official_install_is_atomic_and_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let service = PluginLifecycleService::open(dir.path()).unwrap();
        service
            .add_official_publisher("org.example".into())
            .unwrap();
        let key = SigningKey::from_bytes(&[1; 32]);
        let bytes = b"plugin-v1";
        let first = service
            .install(
                signed_manifest(&key, bytes, "1.0.0"),
                bytes,
                context(&key, "req-1"),
            )
            .unwrap();
        assert!(first.artifact_path.exists());
        assert!(!first.idempotent_replay);
        let replay = service
            .install(
                signed_manifest(&key, bytes, "1.0.0"),
                bytes,
                context(&key, "req-1"),
            )
            .unwrap();
        assert!(replay.idempotent_replay);
        assert_eq!(service.list().len(), 1);
    }

    #[test]
    fn compatibility_and_permissions_are_checked_before_commit() {
        let dir = tempfile::tempdir().unwrap();
        let service = PluginLifecycleService::open(dir.path()).unwrap();
        service
            .add_official_publisher("org.example".into())
            .unwrap();
        let key = SigningKey::from_bytes(&[1; 32]);
        let bytes = b"plugin";
        let mut wrong_platform = context(&key, "req-1");
        wrong_platform.platform = "windows".into();
        assert!(matches!(
            service.install(signed_manifest(&key, bytes, "1.0.0"), bytes, wrong_platform),
            Err(PluginLifecycleError::Incompatible { .. })
        ));
        let mut denied = context(&key, "req-2");
        denied.granted_permissions.clear();
        assert!(matches!(
            service.install(signed_manifest(&key, bytes, "1.0.0"), bytes, denied),
            Err(PluginLifecycleError::Permissions(_))
        ));
        assert!(service.list().is_empty());
    }

    #[test]
    fn revoked_release_cannot_be_re_enabled_after_disable() {
        let dir = tempfile::tempdir().unwrap();
        let service = PluginLifecycleService::open(dir.path()).unwrap();
        service
            .add_official_publisher("org.example".into())
            .unwrap();
        let key = SigningKey::from_bytes(&[1; 32]);
        let manifest = signed_manifest(&key, b"v1", "1.0.0");
        let manifest_cid = cid_v1_for(&manifest).unwrap();
        service
            .install(manifest, b"v1", context(&key, "req-1"))
            .unwrap();
        service.enable("org.example.output").unwrap();

        service.revoke_release(&manifest_cid).unwrap();
        let record = service.get("org.example.output").unwrap();
        assert_eq!(record.lifecycle_state, PluginLifecycleState::Revoked);
        // 状态被改回 Disabled 后，重新启用仍必须被拒绝（SEC-011 防绕过）。
        service.disable("org.example.output").unwrap();
        assert!(matches!(
            service.enable("org.example.output"),
            Err(PluginLifecycleError::Revoked)
        ));
    }

    #[test]
    fn rollback_to_revoked_release_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let service = PluginLifecycleService::open(dir.path()).unwrap();
        service
            .add_official_publisher("org.example".into())
            .unwrap();
        let key = SigningKey::from_bytes(&[1; 32]);
        let v1 = signed_manifest(&key, b"v1", "1.0.0");
        let v1_cid = cid_v1_for(&v1).unwrap();
        service.install(v1, b"v1", context(&key, "req-1")).unwrap();
        service
            .install(
                signed_manifest(&key, b"v2", "2.0.0"),
                b"v2",
                context(&key, "req-2"),
            )
            .unwrap();
        // 撤销 v1（当前为回滚目标，活动版本 v2 不受影响）。
        let revoked = service.revoke_release(&v1_cid).unwrap();
        assert!(revoked.is_empty());
        assert_eq!(
            service.get("org.example.output").unwrap().lifecycle_state,
            PluginLifecycleState::Installed
        );
        assert!(matches!(
            service.rollback("org.example.output"),
            Err(PluginLifecycleError::Revoked)
        ));
    }

    #[test]
    fn upgrade_preserves_rollback_and_failed_upgrade_keeps_active() {
        let dir = tempfile::tempdir().unwrap();
        let service = PluginLifecycleService::open(dir.path()).unwrap();
        service
            .add_official_publisher("org.example".into())
            .unwrap();
        let key = SigningKey::from_bytes(&[1; 32]);
        service
            .install(
                signed_manifest(&key, b"v1", "1.0.0"),
                b"v1",
                context(&key, "req-1"),
            )
            .unwrap();
        service
            .install(
                signed_manifest(&key, b"v2", "2.0.0"),
                b"v2",
                context(&key, "req-2"),
            )
            .unwrap();
        assert_eq!(
            service
                .get("org.example.output")
                .unwrap()
                .rollback_version
                .as_deref(),
            Some("1.0.0")
        );
        let mut corrupt = signed_manifest(&key, b"v3", "3.0.0");
        corrupt.artifacts[0].sha256 = "00".repeat(32);
        corrupt.signature = Some(hex::encode(
            key.sign(&corrupt.unsigned_bytes().unwrap()).to_bytes(),
        ));
        assert!(matches!(
            service.install(corrupt, b"v3", context(&key, "req-3")),
            Err(PluginLifecycleError::DigestMismatch)
        ));
        assert_eq!(
            service
                .get("org.example.output")
                .unwrap()
                .active_version
                .as_deref(),
            Some("2.0.0")
        );
        assert_eq!(
            service
                .rollback("org.example.output")
                .unwrap()
                .active_version
                .as_deref(),
            Some("1.0.0")
        );
    }

    #[test]
    fn permission_revocation_is_immediate_and_failures_quarantine() {
        let dir = tempfile::tempdir().unwrap();
        let service = PluginLifecycleService::open(dir.path()).unwrap();
        service
            .add_official_publisher("org.example".into())
            .unwrap();
        let key = SigningKey::from_bytes(&[1; 32]);
        service
            .install(
                signed_manifest(&key, b"v1", "1.0.0"),
                b"v1",
                context(&key, "req"),
            )
            .unwrap();
        service.enable("org.example.output").unwrap();
        let revoked = service
            .revoke_permission("org.example.output", PluginPermission::AudioDevice)
            .unwrap();
        assert_eq!(revoked.lifecycle_state, PluginLifecycleState::Disabled);
        for _ in 0..3 {
            service
                .record_failure("org.example.output", "crash".into())
                .unwrap();
        }
        assert_eq!(
            service.get("org.example.output").unwrap().lifecycle_state,
            PluginLifecycleState::Quarantined
        );
        assert!(service.safe_mode());
    }

    #[test]
    fn trusted_microkernel_services_cannot_be_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let service = PluginLifecycleService::open(dir.path()).unwrap();
        service
            .add_official_publisher("org.example".into())
            .unwrap();
        let key = SigningKey::from_bytes(&[1; 32]);
        service
            .install(
                signed_manifest(&key, b"v1", "1.0.0"),
                b"v1",
                context(&key, "req"),
            )
            .unwrap();
        assert!(matches!(
            service.register_service("org.example.output", "crypto"),
            Err(PluginLifecycleError::TrustedService(_))
        ));
        service
            .register_service("org.example.output", "PlaybackService")
            .unwrap();
    }

    #[test]
    fn core_version_ranges_accept_major_and_minor_wildcards() {
        assert!(check_core_version("2.4.1", "2.0.0", "2.x").is_ok());
        assert!(check_core_version("2.4.1", "2.4", "2.4.x").is_ok());
        assert!(check_core_version("2.4.1-beta.1", "2.4.0", "2.4.x").is_ok());
        assert!(check_core_version("3.0.0", "2.0.0", "2.x").is_err());
        assert!(check_core_version("2.5.0", "2.4", "2.4.x").is_err());
    }

    #[test]
    fn duplicate_service_registration_has_specific_error() {
        let dir = tempfile::tempdir().unwrap();
        let service = PluginLifecycleService::open(dir.path()).unwrap();
        service
            .add_official_publisher("org.example".into())
            .unwrap();
        let key = SigningKey::from_bytes(&[1; 32]);
        service
            .install(
                signed_manifest(&key, b"v1", "1.0.0"),
                b"v1",
                context(&key, "req"),
            )
            .unwrap();
        service
            .register_service("org.example.output", "PlaybackService")
            .unwrap();
        let mut second = signed_manifest(&key, b"v2", "1.0.0");
        second.plugin_id = "org.example.other".into();
        second.signature = Some(hex::encode(
            key.sign(&second.unsigned_bytes().unwrap()).to_bytes(),
        ));
        service
            .install(second, b"v2", context(&key, "req-2"))
            .unwrap();
        assert!(matches!(
            service.register_service("org.example.other", "PlaybackService"),
            Err(PluginLifecycleError::ServiceConflict(_))
        ));
    }

    #[test]
    fn dependencies_and_conflicts_are_rejected_during_preflight() {
        let dir = tempfile::tempdir().unwrap();
        let service = PluginLifecycleService::open(dir.path()).unwrap();
        service
            .add_official_publisher("org.example".into())
            .unwrap();
        let key = SigningKey::from_bytes(&[1; 32]);
        let mut dependent = signed_manifest(&key, b"v1", "1.0.0");
        dependent.dependencies.push(PluginDependencyV1 {
            plugin_id: "org.example.missing".into(),
            version_requirement: "^1.0".into(),
            optional: false,
        });
        dependent.signature = Some(hex::encode(
            key.sign(&dependent.unsigned_bytes().unwrap()).to_bytes(),
        ));
        let preflight_context = context(&key, "preflight");
        assert!(matches!(
            service.preflight(&dependent, &preflight_context),
            Err(PluginLifecycleError::DependencyMissing(_))
        ));

        service
            .install(
                signed_manifest(&key, b"v1", "1.0.0"),
                b"v1",
                context(&key, "installed"),
            )
            .unwrap();
        let mut conflicting = signed_manifest(&key, b"v2", "1.0.0");
        conflicting.plugin_id = "org.example.other".into();
        conflicting.conflicts.push("org.example.output".into());
        conflicting.signature = Some(hex::encode(
            key.sign(&conflicting.unsigned_bytes().unwrap()).to_bytes(),
        ));
        assert!(matches!(
            service.install(conflicting, b"v2", context(&key, "conflict")),
            Err(PluginLifecycleError::Conflict(_))
        ));
    }
}
