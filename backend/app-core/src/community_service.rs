//! 联邦社区源的 Catalog/Policy 双 Feed、本地索引与可解释策略合并。

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use jimmusic_protocol::{
    canonical_dag_cbor, CatalogAction, CatalogEventV1, CommunitySourceManifestV1, FeedKind,
    FeedSnapshotEntryV1, FeedSnapshotV1, MaintainerKeyAction, MaintainerKeyEventV1,
    ModerationReportV1, PolicyAction, PolicyEventV1, Validate, SCHEMA_V1,
};
use serde::{Deserialize, Serialize};

use crate::crypto::{verify_ed25519_hex, SignatureError};
use crate::node_service::{NodeError, NodeService};
use crate::storage::{AtomicJsonStore, StorageError};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommunitySourceRecord {
    pub manifest_cid: String,
    pub manifest: CommunitySourceManifestV1,
    pub maintainer_public_key: String,
    pub catalog_enabled: bool,
    pub policy_enabled: bool,
    pub trust_order: u32,
    pub last_catalog_sequence: Option<u64>,
    pub last_policy_sequence: Option<u64>,
    pub last_error: Option<String>,
    #[serde(default)]
    pub maintainer_key_revoked: bool,
    #[serde(default)]
    pub last_key_sequence: Option<u64>,
    #[serde(default)]
    pub last_key_event_cid: Option<String>,
    #[serde(default)]
    pub bootstrap: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredCatalogEvent {
    cid: String,
    event: CatalogEventV1,
    /// Canonical DAG-CBOR byte length of `event`; contributes to the feed size cap.
    #[serde(default)]
    byte_length: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredPolicyEvent {
    cid: String,
    event: PolicyEventV1,
    #[serde(default)]
    byte_length: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredMaintainerKeyEvent {
    cid: String,
    event: MaintainerKeyEventV1,
    #[serde(default)]
    byte_length: usize,
}

/// 单个 Feed 的持久化大小上限，防止恶意或失控 Feed 无限增长。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeedLimits {
    pub max_events_per_feed: usize,
    pub max_feed_bytes: usize,
}

impl Default for FeedLimits {
    fn default() -> Self {
        Self {
            max_events_per_feed: 10_000,
            max_feed_bytes: 8 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModerationReportStatus {
    Queued,
    Submitted,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModerationReportRecord {
    pub report_cid: String,
    pub report: ModerationReportV1,
    pub status: ModerationReportStatus,
    pub attempts: u32,
    pub last_attempt_at: Option<i64>,
    pub last_error: Option<String>,
    #[serde(default)]
    pub next_retry_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CommunityRepositoryState {
    schema_version: u16,
    sources: BTreeMap<String, CommunitySourceRecord>,
    catalog_feeds: BTreeMap<String, Vec<StoredCatalogEvent>>,
    policy_feeds: BTreeMap<String, Vec<StoredPolicyEvent>>,
    local_blocks: BTreeMap<String, String>,
    #[serde(default)]
    maintainer_key_feeds: BTreeMap<String, Vec<StoredMaintainerKeyEvent>>,
    #[serde(default)]
    moderation_reports: BTreeMap<String, ModerationReportRecord>,
    #[serde(default)]
    removed_bootstrap_sources: BTreeSet<String>,
    #[serde(default)]
    feed_limits: FeedLimits,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CatalogSearchResult {
    pub target_cid: String,
    pub target_type: String,
    pub categories: Vec<String>,
    pub tags: Vec<String>,
    pub annotation: Option<String>,
    pub source_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PolicyDecision {
    pub target: String,
    pub action: Option<PolicyAction>,
    pub reason: Option<String>,
    pub source_ids: Vec<String>,
    pub expires_at: Option<i64>,
    pub locally_overridden: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum CommunityError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Node(#[from] NodeError),
    #[error(transparent)]
    Signature(#[from] SignatureError),
    #[error("invalid community object: {0}")]
    Invalid(String),
    #[error("source `{0}` does not exist")]
    SourceNotFound(String),
    #[error("signature is required")]
    MissingSignature,
    #[error("feed sequence mismatch: expected {expected}, got {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("feed previous CID mismatch")]
    PreviousEvent,
    #[error("community source maintainer key has been revoked")]
    MaintainerRevoked,
    #[error("maintainer key continuity check failed")]
    MaintainerKeyMismatch,
    #[error("moderation report `{0}` already exists")]
    DuplicateReport(String),
    #[error("moderation report `{0}` does not exist")]
    ReportNotFound(String),
    #[error("feed size limit exceeded: {0}")]
    FeedLimitExceeded(String),
    #[error("invalid feed limits: {0}")]
    InvalidFeedLimits(String),
}

pub struct CommunitySourceService {
    store: AtomicJsonStore<CommunityRepositoryState>,
}

impl CommunitySourceService {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, CommunityError> {
        Ok(Self {
            store: AtomicJsonStore::open(
                path,
                CommunityRepositoryState {
                    schema_version: SCHEMA_V1,
                    sources: BTreeMap::new(),
                    catalog_feeds: BTreeMap::new(),
                    policy_feeds: BTreeMap::new(),
                    local_blocks: BTreeMap::new(),
                    maintainer_key_feeds: BTreeMap::new(),
                    moderation_reports: BTreeMap::new(),
                    removed_bootstrap_sources: BTreeSet::new(),
                    feed_limits: FeedLimits::default(),
                },
            )?,
        })
    }

    pub fn add_source(
        &self,
        manifest: CommunitySourceManifestV1,
        maintainer_public_key: String,
        node: &NodeService,
        trust_order: u32,
    ) -> Result<CommunitySourceRecord, CommunityError> {
        manifest
            .validate()
            .map_err(|error| CommunityError::Invalid(error.to_string()))?;
        verify_ed25519_hex(
            &maintainer_public_key,
            manifest
                .signature
                .as_deref()
                .ok_or(CommunityError::MissingSignature)?,
            &manifest
                .unsigned_bytes()
                .map_err(|error| CommunityError::Invalid(error.to_string()))?,
        )?;
        if let Some(existing) = self.store.snapshot().sources.get(&manifest.source_id) {
            if existing.maintainer_key_revoked {
                return Err(CommunityError::MaintainerRevoked);
            }
            if existing.maintainer_public_key != maintainer_public_key {
                return Err(CommunityError::MaintainerKeyMismatch);
            }
        }
        let bytes = canonical_dag_cbor(&manifest)
            .map_err(|error| CommunityError::Invalid(error.to_string()))?;
        let manifest_cid = node.add_dag_cbor(&bytes, true)?;
        let record = CommunitySourceRecord {
            manifest_cid,
            manifest: manifest.clone(),
            maintainer_public_key,
            catalog_enabled: true,
            policy_enabled: true,
            trust_order,
            last_catalog_sequence: None,
            last_policy_sequence: None,
            last_error: None,
            maintainer_key_revoked: false,
            last_key_sequence: None,
            last_key_event_cid: None,
            bootstrap: false,
        };
        self.store.transact(|state| {
            if let Some(existing) = state.sources.get_mut(&manifest.source_id) {
                existing.manifest_cid = record.manifest_cid.clone();
                existing.manifest = record.manifest.clone();
                existing.trust_order = trust_order;
                Ok(())
            } else {
                state
                    .sources
                    .insert(manifest.source_id.clone(), record.clone());
                Ok(())
            }
        })?;
        Ok(self
            .store
            .snapshot()
            .sources
            .get(&manifest.source_id)
            .cloned()
            .expect("source inserted above"))
    }

    pub fn list_sources(&self) -> Vec<CommunitySourceRecord> {
        self.store.snapshot().sources.into_values().collect()
    }

    pub fn feed_limits(&self) -> FeedLimits {
        self.store.snapshot().feed_limits
    }

    pub fn set_feed_limits(&self, limits: FeedLimits) -> Result<(), CommunityError> {
        if limits.max_events_per_feed == 0 || limits.max_feed_bytes == 0 {
            return Err(CommunityError::InvalidFeedLimits(
                "limits must be greater than zero".into(),
            ));
        }
        self.store.transact(|state| {
            state.feed_limits = limits;
            Ok(())
        })?;
        Ok(())
    }

    /// Installs a signed built-in source exactly once. If a user removes the
    /// bootstrap source, a durable tombstone prevents it from reappearing on
    /// restart. Disabling its Catalog/Policy switches also persists normally.
    pub fn ensure_bootstrap_source(
        &self,
        manifest: CommunitySourceManifestV1,
        maintainer_public_key: String,
        node: &NodeService,
        trust_order: u32,
    ) -> Result<Option<CommunitySourceRecord>, CommunityError> {
        let source_id = manifest.source_id.clone();
        let snapshot = self.store.snapshot();
        if snapshot.sources.contains_key(&source_id)
            || snapshot.removed_bootstrap_sources.contains(&source_id)
        {
            return Ok(None);
        }
        self.add_source(manifest, maintainer_public_key, node, trust_order)?;
        let record = self.store.transact(|state| {
            let record =
                state
                    .sources
                    .get_mut(&source_id)
                    .ok_or_else(|| StorageError::Corrupt {
                        path: PathBuf::from("community-source"),
                        reason: "bootstrap source disappeared during installation".into(),
                    })?;
            record.bootstrap = true;
            Ok(record.clone())
        })?;
        Ok(Some(record))
    }

    pub fn set_enabled(
        &self,
        source_id: &str,
        catalog_enabled: bool,
        policy_enabled: bool,
    ) -> Result<CommunitySourceRecord, CommunityError> {
        if !self.store.snapshot().sources.contains_key(source_id) {
            return Err(CommunityError::SourceNotFound(source_id.into()));
        }
        Ok(self.store.transact(|state| {
            let record = state.sources.get_mut(source_id).expect("checked above");
            record.catalog_enabled = catalog_enabled;
            record.policy_enabled = policy_enabled;
            Ok(record.clone())
        })?)
    }

    pub fn remove_source(&self, source_id: &str) -> Result<(), CommunityError> {
        let snapshot = self.store.snapshot();
        let Some(record) = snapshot.sources.get(source_id) else {
            return Err(CommunityError::SourceNotFound(source_id.into()));
        };
        let bootstrap = record.bootstrap;
        self.store.transact(|state| {
            state.sources.remove(source_id);
            state.catalog_feeds.remove(source_id);
            state.policy_feeds.remove(source_id);
            state.maintainer_key_feeds.remove(source_id);
            if bootstrap {
                state.removed_bootstrap_sources.insert(source_id.into());
            }
            Ok(())
        })?;
        Ok(())
    }

    pub fn ingest_catalog(
        &self,
        source_id: &str,
        event: CatalogEventV1,
        node: &NodeService,
    ) -> Result<String, CommunityError> {
        event
            .validate()
            .map_err(|error| CommunityError::Invalid(error.to_string()))?;
        let source = self
            .store
            .snapshot()
            .sources
            .get(source_id)
            .cloned()
            .ok_or_else(|| CommunityError::SourceNotFound(source_id.into()))?;
        if source.maintainer_key_revoked {
            return Err(CommunityError::MaintainerRevoked);
        }
        verify_ed25519_hex(
            &source.maintainer_public_key,
            event
                .signature
                .as_deref()
                .ok_or(CommunityError::MissingSignature)?,
            &event
                .unsigned_bytes()
                .map_err(|error| CommunityError::Invalid(error.to_string()))?,
        )?;
        let snapshot = self.store.snapshot();
        let feed = snapshot.catalog_feeds.get(source_id);
        check_catalog_chain(feed, &event)?;
        let bytes = canonical_dag_cbor(&event)
            .map_err(|error| CommunityError::Invalid(error.to_string()))?;
        check_feed_limits(
            feed.map_or(0, Vec::len),
            feed.map_or(0, |feed| feed.iter().map(|stored| stored.byte_length).sum()),
            bytes.len(),
            snapshot.feed_limits,
            "catalog feed",
        )?;
        let cid = node.add_dag_cbor(&bytes, false)?;
        self.store.transact(|state| {
            check_catalog_chain(state.catalog_feeds.get(source_id), &event).map_err(|error| {
                StorageError::Corrupt {
                    path: PathBuf::from("catalog-feed"),
                    reason: error.to_string(),
                }
            })?;
            let feed = state.catalog_feeds.entry(source_id.into()).or_default();
            check_feed_limits(
                feed.len(),
                feed.iter().map(|stored| stored.byte_length).sum(),
                bytes.len(),
                state.feed_limits,
                "catalog feed",
            )
            .map_err(|error| StorageError::Corrupt {
                path: PathBuf::from("catalog-feed"),
                reason: error.to_string(),
            })?;
            feed.push(StoredCatalogEvent {
                cid: cid.clone(),
                event: event.clone(),
                byte_length: bytes.len(),
            });
            state
                .sources
                .get_mut(source_id)
                .expect("source checked above")
                .last_catalog_sequence = Some(event.sequence);
            Ok(())
        })?;
        Ok(cid)
    }

    pub fn ingest_policy(
        &self,
        source_id: &str,
        event: PolicyEventV1,
        node: &NodeService,
    ) -> Result<String, CommunityError> {
        event
            .validate()
            .map_err(|error| CommunityError::Invalid(error.to_string()))?;
        let source = self
            .store
            .snapshot()
            .sources
            .get(source_id)
            .cloned()
            .ok_or_else(|| CommunityError::SourceNotFound(source_id.into()))?;
        if source.maintainer_key_revoked {
            return Err(CommunityError::MaintainerRevoked);
        }
        verify_ed25519_hex(
            &source.maintainer_public_key,
            event
                .signature
                .as_deref()
                .ok_or(CommunityError::MissingSignature)?,
            &event
                .unsigned_bytes()
                .map_err(|error| CommunityError::Invalid(error.to_string()))?,
        )?;
        let snapshot = self.store.snapshot();
        let feed = snapshot.policy_feeds.get(source_id);
        check_policy_chain(feed, &event)?;
        let bytes = canonical_dag_cbor(&event)
            .map_err(|error| CommunityError::Invalid(error.to_string()))?;
        check_feed_limits(
            feed.map_or(0, Vec::len),
            feed.map_or(0, |feed| feed.iter().map(|stored| stored.byte_length).sum()),
            bytes.len(),
            snapshot.feed_limits,
            "policy feed",
        )?;
        let cid = node.add_dag_cbor(&bytes, false)?;
        self.store.transact(|state| {
            check_policy_chain(state.policy_feeds.get(source_id), &event).map_err(|error| {
                StorageError::Corrupt {
                    path: PathBuf::from("policy-feed"),
                    reason: error.to_string(),
                }
            })?;
            let feed = state.policy_feeds.entry(source_id.into()).or_default();
            check_feed_limits(
                feed.len(),
                feed.iter().map(|stored| stored.byte_length).sum(),
                bytes.len(),
                state.feed_limits,
                "policy feed",
            )
            .map_err(|error| StorageError::Corrupt {
                path: PathBuf::from("policy-feed"),
                reason: error.to_string(),
            })?;
            feed.push(StoredPolicyEvent {
                cid: cid.clone(),
                event: event.clone(),
                byte_length: bytes.len(),
            });
            state
                .sources
                .get_mut(source_id)
                .expect("source checked above")
                .last_policy_sequence = Some(event.sequence);
            Ok(())
        })?;
        Ok(cid)
    }

    /// Applies a signed maintainer key rotation/revocation event.
    ///
    /// Rotation requires signatures from both the current and replacement
    /// keys. Revocation is terminal: subsequent feed events and source
    /// manifest replacement are rejected.
    pub fn apply_maintainer_key_event(
        &self,
        source_id: &str,
        event: MaintainerKeyEventV1,
        node: &NodeService,
    ) -> Result<String, CommunityError> {
        event
            .validate()
            .map_err(|error| CommunityError::Invalid(error.to_string()))?;
        if event.source_id != source_id {
            return Err(CommunityError::Invalid(
                "source_id does not match route".into(),
            ));
        }
        let snapshot = self.store.snapshot();
        let source = snapshot
            .sources
            .get(source_id)
            .ok_or_else(|| CommunityError::SourceNotFound(source_id.into()))?;
        if source.maintainer_key_revoked {
            return Err(CommunityError::MaintainerRevoked);
        }
        if event.current_public_key != source.maintainer_public_key {
            return Err(CommunityError::MaintainerKeyMismatch);
        }
        let feed = snapshot.maintainer_key_feeds.get(source_id);
        check_chain(
            feed.map(Vec::len).unwrap_or(0),
            feed.and_then(|events| events.last().map(|stored| stored.cid.as_str())),
            event.sequence,
            event.previous_event_cid.as_deref(),
        )?;
        let unsigned = event
            .unsigned_bytes()
            .map_err(|error| CommunityError::Invalid(error.to_string()))?;
        verify_ed25519_hex(
            &source.maintainer_public_key,
            event
                .signature
                .as_deref()
                .ok_or(CommunityError::MissingSignature)?,
            &unsigned,
        )?;
        if event.action == MaintainerKeyAction::Rotate {
            verify_ed25519_hex(
                event
                    .new_public_key
                    .as_deref()
                    .ok_or_else(|| CommunityError::Invalid("new key is required".into()))?,
                event
                    .new_key_proof
                    .as_deref()
                    .ok_or_else(|| CommunityError::Invalid("new key proof is required".into()))?,
                &unsigned,
            )?;
        }
        let bytes = canonical_dag_cbor(&event)
            .map_err(|error| CommunityError::Invalid(error.to_string()))?;
        let cid = node.add_dag_cbor(&bytes, true)?;
        self.store.transact(|state| {
            let record = state
                .sources
                .get_mut(source_id)
                .ok_or_else(|| StorageError::Corrupt {
                    path: PathBuf::from("community-source"),
                    reason: "source disappeared during key update".into(),
                })?;
            if record.maintainer_key_revoked
                || record.maintainer_public_key != event.current_public_key
            {
                return Err(StorageError::Corrupt {
                    path: PathBuf::from("maintainer-key-feed"),
                    reason: "maintainer key changed concurrently".into(),
                });
            }
            let events = state
                .maintainer_key_feeds
                .entry(source_id.into())
                .or_default();
            check_chain(
                events.len(),
                events.last().map(|stored| stored.cid.as_str()),
                event.sequence,
                event.previous_event_cid.as_deref(),
            )
            .map_err(|error| StorageError::Corrupt {
                path: PathBuf::from("maintainer-key-feed"),
                reason: error.to_string(),
            })?;
            events.push(StoredMaintainerKeyEvent {
                cid: cid.clone(),
                event: event.clone(),
                byte_length: bytes.len(),
            });
            match event.action {
                MaintainerKeyAction::Rotate => {
                    record.maintainer_public_key = event
                        .new_public_key
                        .clone()
                        .expect("validated rotation key");
                }
                MaintainerKeyAction::Revoke => record.maintainer_key_revoked = true,
            }
            record.last_key_sequence = Some(event.sequence);
            record.last_key_event_cid = Some(cid.clone());
            Ok(())
        })?;
        Ok(cid)
    }

    /// Verifies and durably queues a moderation report before any network
    /// attempt. Anonymous reports must use a fresh pseudonymous signing key.
    pub fn queue_moderation_report(
        &self,
        report: ModerationReportV1,
        node: &NodeService,
    ) -> Result<ModerationReportRecord, CommunityError> {
        report
            .validate()
            .map_err(|error| CommunityError::Invalid(error.to_string()))?;
        let snapshot = self.store.snapshot();
        let source = snapshot
            .sources
            .get(&report.recipient_source_id)
            .ok_or_else(|| CommunityError::SourceNotFound(report.recipient_source_id.clone()))?;
        if source.maintainer_key_revoked {
            return Err(CommunityError::MaintainerRevoked);
        }
        if snapshot.moderation_reports.contains_key(&report.report_id) {
            return Err(CommunityError::DuplicateReport(report.report_id));
        }
        verify_ed25519_hex(
            &report.reporter_public_key,
            report
                .signature
                .as_deref()
                .ok_or(CommunityError::MissingSignature)?,
            &report
                .unsigned_bytes()
                .map_err(|error| CommunityError::Invalid(error.to_string()))?,
        )?;
        let bytes = canonical_dag_cbor(&report)
            .map_err(|error| CommunityError::Invalid(error.to_string()))?;
        let report_cid = node.add_dag_cbor(&bytes, false)?;
        let record = ModerationReportRecord {
            report_cid,
            report: report.clone(),
            status: ModerationReportStatus::Queued,
            attempts: 0,
            last_attempt_at: None,
            last_error: None,
            next_retry_at: None,
        };
        self.store.transact(|state| {
            if state.moderation_reports.contains_key(&report.report_id) {
                return Err(StorageError::Corrupt {
                    path: PathBuf::from("moderation-reports"),
                    reason: format!("duplicate report id {}", report.report_id),
                });
            }
            state
                .moderation_reports
                .insert(report.report_id.clone(), record.clone());
            Ok(())
        })?;
        Ok(record)
    }

    pub fn list_moderation_reports(&self) -> Vec<ModerationReportRecord> {
        self.store
            .snapshot()
            .moderation_reports
            .into_values()
            .collect()
    }

    pub fn moderation_report(
        &self,
        report_id: &str,
    ) -> Result<ModerationReportRecord, CommunityError> {
        self.store
            .snapshot()
            .moderation_reports
            .get(report_id)
            .cloned()
            .ok_or_else(|| CommunityError::ReportNotFound(report_id.into()))
    }

    pub fn due_moderation_reports(&self, timestamp: i64) -> Vec<ModerationReportRecord> {
        self.list_moderation_reports()
            .into_iter()
            .filter(|record| {
                record.status == ModerationReportStatus::Queued
                    || (record.status == ModerationReportStatus::Failed
                        && record
                            .next_retry_at
                            .is_none_or(|retry_at| retry_at <= timestamp))
            })
            .collect()
    }

    pub fn record_report_attempt(
        &self,
        report_id: &str,
        succeeded: bool,
        attempted_at: i64,
        error: Option<String>,
    ) -> Result<ModerationReportRecord, CommunityError> {
        if !self
            .store
            .snapshot()
            .moderation_reports
            .contains_key(report_id)
        {
            return Err(CommunityError::ReportNotFound(report_id.into()));
        }
        Ok(self.store.transact(|state| {
            let record = state
                .moderation_reports
                .get_mut(report_id)
                .expect("report checked above");
            record.attempts = record.attempts.saturating_add(1);
            record.last_attempt_at = Some(attempted_at);
            record.status = if succeeded {
                ModerationReportStatus::Submitted
            } else {
                ModerationReportStatus::Failed
            };
            record.last_error = if succeeded { None } else { error };
            record.next_retry_at = if succeeded {
                None
            } else {
                let exponent = record.attempts.saturating_sub(1).min(7);
                let delay = 30i64.saturating_mul(1i64 << exponent);
                Some(attempted_at.saturating_add(delay.min(3_600)))
            };
            Ok(record.clone())
        })?)
    }

    /// 从所有启用 Catalog 的最新事件重建候选；相同 CID 合并来源，remove 只影响所属源。
    pub fn search_catalog(&self, query: &str, now: i64) -> Vec<CatalogSearchResult> {
        let state = self.store.snapshot();
        let query = query.trim().to_lowercase();
        let mut latest: BTreeMap<(String, String), &StoredCatalogEvent> = BTreeMap::new();
        for (source_id, feed) in &state.catalog_feeds {
            if !state
                .sources
                .get(source_id)
                .is_some_and(|source| source.catalog_enabled)
            {
                continue;
            }
            for stored in feed {
                if stored.event.expires_at.is_some_and(|expiry| expiry <= now) {
                    continue;
                }
                latest.insert((source_id.clone(), stored.event.target_cid.clone()), stored);
            }
        }

        let mut merged: BTreeMap<String, CatalogSearchResult> = BTreeMap::new();
        for ((source_id, _), stored) in latest {
            if stored.event.action == CatalogAction::Remove {
                continue;
            }
            let searchable = format!(
                "{} {} {} {}",
                stored.event.target_cid,
                stored.event.categories.join(" "),
                stored.event.tags.join(" "),
                stored.event.annotation.as_deref().unwrap_or_default()
            )
            .to_lowercase();
            if !query.is_empty() && !searchable.contains(&query) {
                continue;
            }
            let result = merged
                .entry(stored.event.target_cid.clone())
                .or_insert_with(|| CatalogSearchResult {
                    target_cid: stored.event.target_cid.clone(),
                    target_type: stored.event.target_type.clone(),
                    categories: stored.event.categories.clone(),
                    tags: stored.event.tags.clone(),
                    annotation: stored.event.annotation.clone(),
                    source_ids: Vec::new(),
                });
            result.source_ids.push(source_id);
        }
        merged.into_values().collect()
    }

    /// 本地屏蔽优先；否则取所有启用且未过期 Policy 中最高严重度，并列出决策来源。
    /// 当前生效（未过期、来源策略已启用）的 Revoke 决策目标集合。
    /// 插件管理面用它在下一次策略刷新/摄取后自动停用被撤销的发布
    /// （PLG-009）：目标按内容寻址 CID 匹配已安装版本的 manifest CID。
    pub fn active_revoke_targets(&self, now: i64) -> BTreeSet<String> {
        let state = self.store.snapshot();
        let mut targets = BTreeSet::new();
        for (source_id, feed) in &state.policy_feeds {
            if !state
                .sources
                .get(source_id)
                .is_some_and(|source| source.policy_enabled)
            {
                continue;
            }
            for event in feed {
                if event.event.action == PolicyAction::Revoke
                    && !event.event.expires_at.is_some_and(|expiry| expiry <= now)
                {
                    targets.insert(event.event.target.clone());
                }
            }
        }
        targets
    }

    pub fn policy_decision(&self, target: &str, now: i64) -> PolicyDecision {
        let state = self.store.snapshot();
        if let Some(reason) = state.local_blocks.get(target) {
            return PolicyDecision {
                target: target.into(),
                action: Some(PolicyAction::Block),
                reason: Some(reason.clone()),
                source_ids: vec!["local".into()],
                expires_at: None,
                locally_overridden: true,
            };
        }

        let mut latest_by_source: BTreeMap<String, &StoredPolicyEvent> = BTreeMap::new();
        for (source_id, feed) in &state.policy_feeds {
            if !state
                .sources
                .get(source_id)
                .is_some_and(|source| source.policy_enabled)
            {
                continue;
            }
            for event in feed {
                if event.event.target == target
                    && !event.event.expires_at.is_some_and(|expiry| expiry <= now)
                {
                    latest_by_source.insert(source_id.clone(), event);
                }
            }
        }
        let Some(highest) = latest_by_source
            .values()
            .max_by_key(|stored| stored.event.action)
        else {
            return PolicyDecision {
                target: target.into(),
                action: None,
                reason: None,
                source_ids: Vec::new(),
                expires_at: None,
                locally_overridden: false,
            };
        };
        let action = highest.event.action;
        let reason = highest.event.description.clone();
        let matching: Vec<_> = latest_by_source
            .into_iter()
            .filter(|(_, stored)| stored.event.action == action)
            .collect();
        PolicyDecision {
            target: target.into(),
            action: Some(action),
            reason: Some(reason),
            source_ids: matching.iter().map(|(source, _)| source.clone()).collect(),
            expires_at: matching
                .iter()
                .filter_map(|(_, stored)| stored.event.expires_at)
                .min(),
            locally_overridden: false,
        }
    }

    pub fn set_local_block(
        &self,
        target: &str,
        reason: Option<String>,
    ) -> Result<(), CommunityError> {
        self.store.transact(|state| {
            if let Some(reason) = reason {
                state.local_blocks.insert(target.into(), reason);
            } else {
                state.local_blocks.remove(target);
            }
            Ok(())
        })?;
        Ok(())
    }

    pub fn rebuild_index(&self, now: i64) -> Vec<CatalogSearchResult> {
        self.search_catalog("", now)
    }

    pub fn catalog_targets(&self, now: i64) -> BTreeSet<String> {
        self.search_catalog("", now)
            .into_iter()
            .map(|result| result.target_cid)
            .collect()
    }

    /// 生成 Catalog Feed 的紧凑快照：每个目标只保留最新未过期事件，已过期事件被丢弃。
    /// 这是大型 Feed 无需每次全量回放的“压缩”形态，head 锚定到签名事件链。
    pub fn snapshot_catalog(
        &self,
        source_id: &str,
        now: i64,
    ) -> Result<FeedSnapshotV1, CommunityError> {
        let state = self.store.snapshot();
        let source = state
            .sources
            .get(source_id)
            .ok_or_else(|| CommunityError::SourceNotFound(source_id.into()))?;
        let feed = state
            .catalog_feeds
            .get(source_id)
            .cloned()
            .unwrap_or_default();

        let mut latest: BTreeMap<String, StoredCatalogEvent> = BTreeMap::new();
        for stored in &feed {
            if stored.event.expires_at.is_some_and(|expiry| expiry <= now) {
                continue;
            }
            latest.insert(stored.event.target_cid.clone(), stored.clone());
        }

        let entries = latest
            .into_values()
            .map(|stored| FeedSnapshotEntryV1 {
                target: stored.event.target_cid.clone(),
                target_type: stored.event.target_type.clone(),
                action: snake_case(stored.event.action),
                categories: stored.event.categories.clone(),
                tags: stored.event.tags.clone(),
                annotation: stored.event.annotation.clone(),
                reason_code: None,
                description: None,
                issued_at: stored.event.issued_at,
                expires_at: stored.event.expires_at,
            })
            .collect();
        let head = feed
            .last()
            .map(|stored| (stored.event.sequence, stored.cid.clone()));
        Ok(FeedSnapshotV1 {
            schema_version: SCHEMA_V1,
            source_id: source_id.into(),
            feed_kind: FeedKind::Catalog,
            head_sequence: head
                .as_ref()
                .map_or(source.last_catalog_sequence.unwrap_or(0), |(s, _)| *s),
            head_event_cid: head.map(|(_, cid)| cid),
            created_at: now,
            entries,
        })
    }

    /// 生成 Policy Feed 的紧凑快照：每个目标只保留最新未过期决策。
    pub fn snapshot_policy(
        &self,
        source_id: &str,
        now: i64,
    ) -> Result<FeedSnapshotV1, CommunityError> {
        let state = self.store.snapshot();
        let source = state
            .sources
            .get(source_id)
            .ok_or_else(|| CommunityError::SourceNotFound(source_id.into()))?;
        let feed = state
            .policy_feeds
            .get(source_id)
            .cloned()
            .unwrap_or_default();

        let mut latest: BTreeMap<String, StoredPolicyEvent> = BTreeMap::new();
        for stored in &feed {
            if stored.event.expires_at.is_some_and(|expiry| expiry <= now) {
                continue;
            }
            latest.insert(stored.event.target.clone(), stored.clone());
        }

        let entries = latest
            .into_values()
            .map(|stored| FeedSnapshotEntryV1 {
                target: stored.event.target.clone(),
                target_type: stored.event.target_type.clone(),
                action: snake_case(stored.event.action),
                categories: Vec::new(),
                tags: Vec::new(),
                annotation: None,
                reason_code: Some(stored.event.reason_code.clone()),
                description: Some(stored.event.description.clone()),
                issued_at: stored.event.issued_at,
                expires_at: stored.event.expires_at,
            })
            .collect();
        let head = feed
            .last()
            .map(|stored| (stored.event.sequence, stored.cid.clone()));
        Ok(FeedSnapshotV1 {
            schema_version: SCHEMA_V1,
            source_id: source_id.into(),
            feed_kind: FeedKind::Policy,
            head_sequence: head
                .as_ref()
                .map_or(source.last_policy_sequence.unwrap_or(0), |(s, _)| *s),
            head_event_cid: head.map(|(_, cid)| cid),
            created_at: now,
            entries,
        })
    }
}

fn check_catalog_chain(
    feed: Option<&Vec<StoredCatalogEvent>>,
    event: &CatalogEventV1,
) -> Result<(), CommunityError> {
    check_chain(
        feed.map(|feed| feed.len()).unwrap_or(0),
        feed.and_then(|feed| feed.last().map(|entry| entry.cid.as_str())),
        event.sequence,
        event.previous_event_cid.as_deref(),
    )
}

fn check_policy_chain(
    feed: Option<&Vec<StoredPolicyEvent>>,
    event: &PolicyEventV1,
) -> Result<(), CommunityError> {
    check_chain(
        feed.map(|feed| feed.len()).unwrap_or(0),
        feed.and_then(|feed| feed.last().map(|entry| entry.cid.as_str())),
        event.sequence,
        event.previous_event_cid.as_deref(),
    )
}

fn check_chain(
    length: usize,
    last_cid: Option<&str>,
    sequence: u64,
    previous: Option<&str>,
) -> Result<(), CommunityError> {
    if sequence != length as u64 {
        return Err(CommunityError::Sequence {
            expected: length as u64,
            actual: sequence,
        });
    }
    if previous != last_cid {
        return Err(CommunityError::PreviousEvent);
    }
    Ok(())
}

/// 把 serde snake_case 枚举变体转成稳定字符串（如 `Include` -> `"include"`）。
fn snake_case<T: Serialize>(value: T) -> String {
    serde_json::to_value(&value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".into())
}

/// 统一执行单个 Feed 的事件数上限与累计字节上限，防止恶意或失控 Feed 无限增长。
fn check_feed_limits(
    events_len: usize,
    feed_bytes: usize,
    event_bytes: usize,
    limits: FeedLimits,
    feed_name: &str,
) -> Result<(), CommunityError> {
    if events_len.saturating_add(1) > limits.max_events_per_feed {
        return Err(CommunityError::FeedLimitExceeded(format!(
            "{feed_name} would grow to {} events; limit is {}",
            events_len.saturating_add(1),
            limits.max_events_per_feed
        )));
    }
    if feed_bytes.saturating_add(event_bytes) > limits.max_feed_bytes {
        return Err(CommunityError::FeedLimitExceeded(format!(
            "{feed_name} would grow to {} bytes; limit is {}",
            feed_bytes.saturating_add(event_bytes),
            limits.max_feed_bytes
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn signed_source(key: &SigningKey, source_id: &str) -> CommunitySourceManifestV1 {
        let mut source = CommunitySourceManifestV1 {
            schema_version: SCHEMA_V1,
            source_id: source_id.into(),
            name: source_id.into(),
            description: "test".into(),
            languages: vec!["en".into()],
            maintainer_identity_cid: "bafymaintainer".into(),
            catalog_head: None,
            policy_head: None,
            supported_schemas: vec![SCHEMA_V1],
            report_endpoint: None,
            report_encryption_public_key: None,
            updated_at: 1,
            signature: None,
        };
        source.signature = Some(hex::encode(
            key.sign(&source.unsigned_bytes().unwrap()).to_bytes(),
        ));
        source
    }

    fn signed_catalog(key: &SigningKey, target: &str) -> CatalogEventV1 {
        let mut event = CatalogEventV1 {
            schema_version: SCHEMA_V1,
            action: CatalogAction::Include,
            target_type: "music_manifest".into(),
            target_cid: target.into(),
            categories: vec!["music".into()],
            tags: vec!["ambient".into()],
            annotation: Some("Calm track".into()),
            sequence: 0,
            previous_event_cid: None,
            expires_at: None,
            issued_at: 2,
            signature: None,
        };
        event.signature = Some(hex::encode(
            key.sign(&event.unsigned_bytes().unwrap()).to_bytes(),
        ));
        event
    }

    fn signed_policy(key: &SigningKey, target: &str, action: PolicyAction) -> PolicyEventV1 {
        let mut event = PolicyEventV1 {
            schema_version: SCHEMA_V1,
            action,
            target_type: "cid".into(),
            target: target.into(),
            reason_code: "community_rule".into(),
            description: format!("{action:?} from test source"),
            evidence_cids: Vec::new(),
            scope: Vec::new(),
            issued_at: 2,
            expires_at: Some(100),
            sequence: 0,
            previous_event_cid: None,
            signature: None,
        };
        event.signature = Some(hex::encode(
            key.sign(&event.unsigned_bytes().unwrap()).to_bytes(),
        ));
        event
    }

    fn setup() -> (
        tempfile::TempDir,
        CommunitySourceService,
        NodeService,
        SigningKey,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let service = CommunitySourceService::open(dir.path().join("community.json")).unwrap();
        let node = NodeService::open(dir.path().join("node"), "peer").unwrap();
        let key = SigningKey::from_bytes(&[3; 32]);
        service
            .add_source(
                signed_source(&key, "source-a"),
                hex::encode(key.verifying_key().to_bytes()),
                &node,
                0,
            )
            .unwrap();
        (dir, service, node, key)
    }

    #[test]
    fn catalog_and_policy_switches_are_independent() {
        let (_dir, service, node, key) = setup();
        service
            .ingest_catalog("source-a", signed_catalog(&key, "bafytarget"), &node)
            .unwrap();
        service
            .ingest_policy(
                "source-a",
                signed_policy(&key, "bafytarget", PolicyAction::Warn),
                &node,
            )
            .unwrap();
        service.set_enabled("source-a", false, true).unwrap();
        assert!(service.search_catalog("ambient", 10).is_empty());
        assert_eq!(
            service.policy_decision("bafytarget", 10).action,
            Some(PolicyAction::Warn)
        );
    }

    #[test]
    fn duplicate_catalog_cid_merges_sources() {
        let (_dir, service, node, key) = setup();
        let other_key = SigningKey::from_bytes(&[4; 32]);
        service
            .add_source(
                signed_source(&other_key, "source-b"),
                hex::encode(other_key.verifying_key().to_bytes()),
                &node,
                1,
            )
            .unwrap();
        service
            .ingest_catalog("source-a", signed_catalog(&key, "bafytarget"), &node)
            .unwrap();
        service
            .ingest_catalog("source-b", signed_catalog(&other_key, "bafytarget"), &node)
            .unwrap();
        let results = service.search_catalog("calm", 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source_ids.len(), 2);
    }

    #[test]
    fn highest_policy_severity_wins_and_local_block_has_priority() {
        let (_dir, service, node, key) = setup();
        service
            .ingest_policy(
                "source-a",
                signed_policy(&key, "bafytarget", PolicyAction::Hide),
                &node,
            )
            .unwrap();
        assert_eq!(
            service.policy_decision("bafytarget", 10).action,
            Some(PolicyAction::Hide)
        );
        service
            .set_local_block("bafytarget", Some("my choice".into()))
            .unwrap();
        let decision = service.policy_decision("bafytarget", 10);
        assert_eq!(decision.action, Some(PolicyAction::Block));
        assert!(decision.locally_overridden);
        assert_eq!(decision.source_ids, vec!["local"]);
    }

    #[test]
    fn invalid_signature_and_replay_are_rejected() {
        let (_dir, service, node, key) = setup();
        let mut event = signed_catalog(&key, "bafytarget");
        event.annotation = Some("tampered".into());
        assert!(matches!(
            service.ingest_catalog("source-a", event, &node),
            Err(CommunityError::Signature(_))
        ));
        let event = signed_catalog(&key, "bafytarget");
        service
            .ingest_catalog("source-a", event.clone(), &node)
            .unwrap();
        assert!(matches!(
            service.ingest_catalog("source-a", event, &node),
            Err(CommunityError::Sequence { .. })
        ));
    }

    #[test]
    fn maintainer_rotation_requires_continuity_and_new_key_proof() {
        let (_dir, service, node, old_key) = setup();
        let new_key = SigningKey::from_bytes(&[8; 32]);
        let mut rotation = MaintainerKeyEventV1 {
            schema_version: SCHEMA_V1,
            source_id: "source-a".into(),
            action: MaintainerKeyAction::Rotate,
            sequence: 0,
            previous_event_cid: None,
            current_public_key: hex::encode(old_key.verifying_key().to_bytes()),
            new_public_key: Some(hex::encode(new_key.verifying_key().to_bytes())),
            issued_at: 3,
            signature: None,
            new_key_proof: None,
        };
        let bytes = rotation.unsigned_bytes().unwrap();
        rotation.signature = Some(hex::encode(old_key.sign(&bytes).to_bytes()));
        rotation.new_key_proof = Some(hex::encode(new_key.sign(&bytes).to_bytes()));
        service
            .apply_maintainer_key_event("source-a", rotation, &node)
            .unwrap();

        service
            .ingest_catalog("source-a", signed_catalog(&new_key, "bafynew"), &node)
            .unwrap();
        let mut old_event = signed_catalog(&old_key, "bafyold");
        old_event.sequence = 1;
        old_event.previous_event_cid = service
            .store
            .snapshot()
            .catalog_feeds
            .get("source-a")
            .and_then(|events| events.last())
            .map(|event| event.cid.clone());
        old_event.signature = Some(hex::encode(
            old_key
                .sign(&old_event.unsigned_bytes().unwrap())
                .to_bytes(),
        ));
        assert!(matches!(
            service.ingest_catalog("source-a", old_event, &node),
            Err(CommunityError::Signature(_))
        ));

        assert!(matches!(
            service.add_source(
                signed_source(&old_key, "source-a"),
                hex::encode(old_key.verifying_key().to_bytes()),
                &node,
                0,
            ),
            Err(CommunityError::MaintainerKeyMismatch)
        ));
    }

    #[test]
    fn signed_moderation_reports_queue_offline_and_retry_durably() {
        let (dir, service, node, _maintainer) = setup();
        let reporter = SigningKey::from_bytes(&[9; 32]);
        let mut report = ModerationReportV1 {
            schema_version: SCHEMA_V1,
            report_id: "report-1".into(),
            target: "bafytarget".into(),
            reason_code: "copyright".into(),
            description: "evidence attached".into(),
            evidence_cids: vec!["bafyevidence".into()],
            reporter_identity: None,
            reporter_public_key: hex::encode(reporter.verifying_key().to_bytes()),
            anonymous: true,
            recipient_source_id: "source-a".into(),
            created_at: 10,
            signature: None,
            encrypted_envelope: Some("age-envelope".into()),
        };
        report.signature = Some(hex::encode(
            reporter.sign(&report.unsigned_bytes().unwrap()).to_bytes(),
        ));
        let queued = service.queue_moderation_report(report, &node).unwrap();
        assert_eq!(queued.status, ModerationReportStatus::Queued);
        assert_eq!(queued.attempts, 0);
        let failed = service
            .record_report_attempt("report-1", false, 11, Some("offline".into()))
            .unwrap();
        assert_eq!(failed.status, ModerationReportStatus::Failed);
        assert_eq!(failed.next_retry_at, Some(41));
        assert!(service.due_moderation_reports(40).is_empty());
        assert_eq!(service.due_moderation_reports(41).len(), 1);
        let submitted = service
            .record_report_attempt("report-1", true, 12, None)
            .unwrap();
        assert_eq!(submitted.status, ModerationReportStatus::Submitted);
        assert_eq!(submitted.attempts, 2);
        assert_eq!(submitted.next_retry_at, None);

        drop(service);
        let reopened = CommunitySourceService::open(dir.path().join("community.json")).unwrap();
        assert_eq!(
            reopened.moderation_report("report-1").unwrap().status,
            ModerationReportStatus::Submitted
        );
    }

    fn last_catalog_cid(service: &CommunitySourceService) -> String {
        service
            .store
            .snapshot()
            .catalog_feeds
            .get("source-a")
            .and_then(|feed| feed.last())
            .map(|stored| stored.cid.clone())
            .expect("catalog feed has a head")
    }

    fn last_policy_cid(service: &CommunitySourceService) -> String {
        service
            .store
            .snapshot()
            .policy_feeds
            .get("source-a")
            .and_then(|feed| feed.last())
            .map(|stored| stored.cid.clone())
            .expect("policy feed has a head")
    }

    #[test]
    fn feed_size_limit_is_enforced_before_ingest() {
        let (_dir, service, node, key) = setup();
        service
            .set_feed_limits(FeedLimits {
                max_events_per_feed: 2,
                max_feed_bytes: 1024 * 1024,
            })
            .unwrap();
        service
            .ingest_catalog("source-a", signed_catalog(&key, "bafya"), &node)
            .unwrap();
        let mut second = signed_catalog(&key, "bafyb");
        second.sequence = 1;
        second.previous_event_cid = Some(last_catalog_cid(&service));
        second.signature = Some(hex::encode(
            key.sign(&second.unsigned_bytes().unwrap()).to_bytes(),
        ));
        service.ingest_catalog("source-a", second, &node).unwrap();

        let mut third = signed_catalog(&key, "bafyc");
        third.sequence = 2;
        third.previous_event_cid = Some(last_catalog_cid(&service));
        third.signature = Some(hex::encode(
            key.sign(&third.unsigned_bytes().unwrap()).to_bytes(),
        ));
        assert!(matches!(
            service.ingest_catalog("source-a", third, &node),
            Err(CommunityError::FeedLimitExceeded(_))
        ));
        // Rejected event must not have been appended.
        assert_eq!(
            service
                .store
                .snapshot()
                .catalog_feeds
                .get("source-a")
                .map(Vec::len),
            Some(2)
        );
    }

    #[test]
    fn catalog_snapshot_compacts_superseded_and_expired_events() {
        let (_dir, service, node, key) = setup();
        service
            .ingest_catalog("source-a", signed_catalog(&key, "bafya"), &node)
            .unwrap();
        let mut update = signed_catalog(&key, "bafya");
        update.action = CatalogAction::Update;
        update.sequence = 1;
        update.previous_event_cid = Some(last_catalog_cid(&service));
        update.signature = Some(hex::encode(
            key.sign(&update.unsigned_bytes().unwrap()).to_bytes(),
        ));
        service.ingest_catalog("source-a", update, &node).unwrap();
        let mut expired = signed_catalog(&key, "bafyb");
        expired.sequence = 2;
        expired.previous_event_cid = Some(last_catalog_cid(&service));
        expired.expires_at = Some(5);
        expired.signature = Some(hex::encode(
            key.sign(&expired.unsigned_bytes().unwrap()).to_bytes(),
        ));
        service.ingest_catalog("source-a", expired, &node).unwrap();

        let snapshot = service.snapshot_catalog("source-a", 10).unwrap();
        assert_eq!(snapshot.head_sequence, 2);
        // `bafya` collapses to its latest action; the expired `bafyb` is dropped.
        assert_eq!(snapshot.entries.len(), 1);
        assert_eq!(snapshot.entries[0].target, "bafya");
        assert_eq!(snapshot.entries[0].action, "update");
    }

    #[test]
    fn policy_snapshot_keeps_latest_decision_per_target() {
        let (_dir, service, node, key) = setup();
        service
            .ingest_policy(
                "source-a",
                signed_policy(&key, "bafytarget", PolicyAction::Warn),
                &node,
            )
            .unwrap();
        let mut block = signed_policy(&key, "bafytarget", PolicyAction::Block);
        block.sequence = 1;
        block.previous_event_cid = Some(last_policy_cid(&service));
        block.signature = Some(hex::encode(
            key.sign(&block.unsigned_bytes().unwrap()).to_bytes(),
        ));
        service.ingest_policy("source-a", block, &node).unwrap();

        let snapshot = service.snapshot_policy("source-a", 10).unwrap();
        assert_eq!(snapshot.head_sequence, 1);
        assert_eq!(snapshot.entries.len(), 1);
        assert_eq!(snapshot.entries[0].target, "bafytarget");
        assert_eq!(snapshot.entries[0].action, "block");
        assert_eq!(
            snapshot.entries[0].reason_code.as_deref(),
            Some("community_rule")
        );
    }

    #[test]
    fn snapshot_is_deterministic_and_reopenable() {
        let (dir, service, node, key) = setup();
        service
            .ingest_catalog("source-a", signed_catalog(&key, "bafya"), &node)
            .unwrap();
        let before = service.snapshot_catalog("source-a", 10).unwrap();
        before.validate().unwrap();
        let bytes = canonical_dag_cbor(&before).unwrap();
        assert_eq!(
            canonical_dag_cbor(&service.snapshot_catalog("source-a", 10).unwrap()).unwrap(),
            bytes
        );

        drop(service);
        drop(node);
        let reopened = CommunitySourceService::open(dir.path().join("community.json")).unwrap();
        assert_eq!(reopened.snapshot_catalog("source-a", 10).unwrap(), before);
    }
}
