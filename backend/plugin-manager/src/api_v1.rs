//! JimMusic 2.x `/v1` 控制面。所有写入都经过持久服务与幂等/事务边界。

use std::collections::BTreeSet;
use std::convert::Infallible;
use std::io::Write;
use std::path::PathBuf as FilePath;
use std::sync::Arc;
use std::time::Duration;

use app_core::identity::{EncryptedIdentityBundleV1, PublisherIdentityVault};
use app_core::library_service::PlaybackSessionV1;
use app_core::node_service::NodeConfig;
use app_core::Event;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::header::{ACCEPT_ENCODING, CONTENT_ENCODING, RANGE};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event as SseEvent, KeepAlive};
use axum::response::{IntoResponse, Response, Sse};
use axum::routing::{delete, get, patch, post, put};
use axum::{Json, Router};
use flate2::write::GzEncoder;
use flate2::Compression;
use futures::{SinkExt, StreamExt};
use jimmusic_protocol::{
    cid_v1_for, AudioGraphSpecV1, CatalogEventV1, CommunitySourceManifestV1, ErrorEnvelopeV1,
    MaintainerKeyEventV1, ModerationReportV1, MusicManifestV1, NetworkPolicyV1, PluginManifestV1,
    PluginPermission, PublicationEventType, PublicationEventV1, PublisherIdentityV1, TransferKind,
    TransferState, SCHEMA_V1,
};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};

use crate::lifecycle::InstallContext;
use crate::state::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/health", get(health))
        .route("/diagnostics", get(diagnostics))
        .route("/events", get(events))
        .route("/node/status", get(node_status))
        .route("/node/peers", get(node_peers).post(connect_node_peer))
        .route("/node/peers/{peer_id}", delete(disconnect_node_peer))
        .route("/node/config", get(get_node_config).put(set_node_config))
        .route("/pins", get(list_pins))
        .route("/pins/{cid}", post(pin).delete(unpin))
        .route("/transfers", post(create_transfer).get(list_transfers))
        .route("/transfers/{id}", get(get_transfer))
        .route("/transfers/{id}/pause", post(pause_transfer))
        .route("/transfers/{id}/resume", post(resume_transfer))
        .route("/transfers/{id}/cancel", post(cancel_transfer))
        .route("/transfers/{id}/retry", post(retry_transfer))
        .route("/transfers/{id}/priority", patch(set_transfer_priority))
        .route("/transfers/{id}/stream", get(stream_transfer))
        .route("/identities", post(register_identity))
        .route("/identities/generate", post(generate_identity))
        .route("/identities/import", post(import_identity))
        .route("/identities/rotate", post(rotate_identity))
        .route("/identities/revoke", post(revoke_identity))
        .route("/publications", post(publish))
        .route("/publications/sign", post(sign_publication))
        .route("/publications/{identity_cid}/tombstone", post(tombstone))
        .route("/community-sources", get(list_sources).post(add_source))
        .route("/community-sources/import", post(import_source))
        .route(
            "/community-sources/{id}",
            patch(update_source).delete(remove_source),
        )
        .route("/community-sources/{id}/refresh", post(refresh_source))
        .route(
            "/community-sources/{id}/catalog-events",
            post(ingest_catalog),
        )
        .route("/community-sources/{id}/policy-events", post(ingest_policy))
        .route(
            "/community-sources/{id}/maintainer-key-events",
            post(apply_maintainer_key_event),
        )
        .route("/community-sources/{id}/snapshot", get(source_snapshot))
        .route(
            "/moderation-reports",
            get(list_moderation_reports).post(queue_moderation_report),
        )
        .route(
            "/moderation-reports/{id}/retry",
            post(retry_moderation_report),
        )
        .route("/policy/{target}", get(policy_decision))
        .route("/search", get(search))
        .route("/plugins", get(list_plugins))
        .route("/plugins/install", post(install_plugin))
        .route("/plugins/{id}", get(get_plugin).delete(uninstall_plugin))
        .route("/plugins/{id}/enable", post(enable_plugin))
        .route("/plugins/{id}/disable", post(disable_plugin))
        .route("/plugins/{id}/rollback", post(rollback_plugin))
        .route(
            "/plugins/{id}/config",
            get(get_plugin_config).put(set_plugin_config),
        )
        .route(
            "/plugins/{id}/permissions/{permission}",
            delete(revoke_permission),
        )
        .route("/audio/graph", get(get_audio_graph).put(put_audio_graph))
        .route("/audio/path", get(audio_path))
        .route("/audio/stats", get(audio_stats))
        .route("/library/tracks", get(library_tracks))
        .route("/library/tracks/import-local", post(import_local_track))
        .route("/library/tracks/{id}/favorite", put(set_favorite))
        .route("/library/manifests/{cid}", post(import_manifest))
        .route("/library/scan", post(scan_library))
        .route("/library/availability/refresh", post(refresh_availability))
        .route(
            "/library/music-directory",
            get(get_music_directory).put(set_music_directory),
        )
        .route(
            "/library/playlists",
            get(list_playlists).post(create_playlist),
        )
        .route(
            "/library/playlists/{id}",
            patch(update_playlist).delete(remove_playlist),
        )
        .route("/library/session", get(get_session).put(save_session))
}

#[derive(Debug)]
pub(crate) struct ApiError {
    status: StatusCode,
    body: Box<ErrorEnvelopeV1>,
}

impl ApiError {
    fn bad_request(subsystem: &str, operation: &str, error: impl ToString) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            subsystem,
            operation,
            error,
            false,
        )
    }

    fn not_found(subsystem: &str, operation: &str, id: &str) -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "not_found",
            subsystem,
            operation,
            format!("resource `{id}` was not found"),
            false,
        )
    }

    fn conflict(subsystem: &str, operation: &str, error: impl ToString) -> Self {
        Self::new(
            StatusCode::CONFLICT,
            "conflict",
            subsystem,
            operation,
            error,
            false,
        )
    }

    fn payload_too_large(subsystem: &str, operation: &str, error: impl ToString) -> Self {
        Self::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
            subsystem,
            operation,
            error,
            false,
        )
    }

    /// 显式不支持的能力（PROD-004）：错误信封携带结构化 unsupported_reason。
    fn unsupported(
        subsystem: &str,
        operation: &str,
        error: impl ToString,
        reason: impl Into<String>,
    ) -> Self {
        let mut envelope = Self::new(
            StatusCode::BAD_REQUEST,
            "unsupported",
            subsystem,
            operation,
            error,
            false,
        );
        envelope.body.unsupported_reason = Some(reason.into());
        envelope
    }

    fn new(
        status: StatusCode,
        code: &str,
        subsystem: &str,
        operation: &str,
        error: impl ToString,
        retryable: bool,
    ) -> Self {
        Self {
            status,
            body: Box::new(ErrorEnvelopeV1 {
                schema_version: SCHEMA_V1,
                code: code.into(),
                message: error.to_string(),
                subsystem: subsystem.into(),
                operation: operation.into(),
                retryable,
                unsupported_reason: None,
                details: Default::default(),
                request_id: None,
                causes: Vec::new(),
            }),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(*self.body)).into_response()
    }
}

type ApiResult<T> = Result<Json<T>, ApiError>;

async fn health(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let node_status = state.node_status().await;
    Json(serde_json::json!({
        "schema_version": 1,
        "status": "ok",
        "core_version": env!("CARGO_PKG_VERSION"),
        "node": node_status.lifecycle_state,
        "safe_mode": state.lifecycle.safe_mode(),
        "reliability": state.reliability.report(),
    }))
}

/// 可安全分享的诊断快照：不包含 API token、身份私钥、口令、媒体路径、插件配置或制品路径。
async fn diagnostics(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let node_status = state.node_status().await;
    let tracks = state.library.tracks();
    let plugins: Vec<_> = state
        .lifecycle
        .list()
        .into_iter()
        .map(|record| {
            serde_json::json!({
                "plugin_id": record.plugin_id,
                "version": record.active_version,
                "kind": record.kind,
                "state": record.lifecycle_state,
                "trust_channel": record.trust_channel,
                "consecutive_failures": record.consecutive_failures,
                "has_error": record.last_error.is_some(),
            })
        })
        .collect();
    let transfers: Vec<_> = state
        .transfers
        .list()
        .into_iter()
        .map(|task| {
            serde_json::json!({
                "task_id": task.task_id,
                "kind": task.kind,
                "state": task.state,
                "priority": task.priority,
                "bytes_total": task.bytes_total,
                "bytes_completed": task.bytes_completed,
                "retry_count": task.retry_count,
                "provider_count": task.providers.len(),
                "error_code": task.error.as_ref().map(|error| error.code.as_str()),
                "error_subsystem": task.error.as_ref().map(|error| error.subsystem.as_str()),
                "error_operation": task.error.as_ref().map(|error| error.operation.as_str()),
                "error_retryable": task.error.as_ref().map(|error| error.retryable),
            })
        })
        .collect();
    let community_sources: Vec<_> = state
        .community
        .list_sources()
        .into_iter()
        .map(|source| {
            serde_json::json!({
                "source_id": source.manifest.source_id,
                "manifest_cid": source.manifest_cid,
                "catalog_enabled": source.catalog_enabled,
                "policy_enabled": source.policy_enabled,
                "trust_order": source.trust_order,
                "last_catalog_sequence": source.last_catalog_sequence,
                "last_policy_sequence": source.last_policy_sequence,
                "bootstrap": source.bootstrap,
                "maintainer_key_revoked": source.maintainer_key_revoked,
                "last_key_sequence": source.last_key_sequence,
                "has_error": source.last_error.is_some(),
            })
        })
        .collect();
    let moderation_reports = state.community.list_moderation_reports();
    let moderation_queued = moderation_reports
        .iter()
        .filter(|record| {
            record.status == app_core::community_service::ModerationReportStatus::Queued
        })
        .count();
    let moderation_failed = moderation_reports
        .iter()
        .filter(|record| {
            record.status == app_core::community_service::ModerationReportStatus::Failed
        })
        .count();
    let missing_tracks = tracks
        .iter()
        .filter(|track| {
            matches!(
                track.scan_state,
                app_core::library_service::ScanState::Missing
                    | app_core::library_service::ScanState::Failed
            )
        })
        .count();

    Json(serde_json::json!({
        "schema_version": 1,
        "generated_at": now(),
        "core_version": env!("CARGO_PKG_VERSION"),
        "platform": std::env::consts::OS,
        "architecture": std::env::consts::ARCH,
        "safe_to_share": true,
        "redacted": [
            "api_token",
            "identity_private_keys",
            "passphrases",
            "media_paths",
            "plugin_configuration",
            "plugin_install_paths",
            "provider_addresses"
        ],
        "node": {
            "peer_id": node_status.peer_id,
            "lifecycle_state": node_status.lifecycle_state,
            "transports": node_status.transports,
            "connected_peers": node_status.connected_peers,
            "routing_status": node_status.routing_status,
            "repository_bytes": node_status.repository_bytes,
            "cache_bytes": node_status.cache_bytes,
            "pinned_bytes": node_status.pinned_bytes,
            "bytes_up": node_status.bytes_up,
            "bytes_down": node_status.bytes_down,
            "limitations": node_status.limitations,
            "config": state.node.config(),
            "pin_count": state.node.list_pins().len(),
        },
        "library": {
            "track_count": tracks.len(),
            "missing_or_failed_count": missing_tracks,
            "playlist_count": state.library.playlists().len(),
        },
        "plugins": plugins,
        "transfers": transfers,
        "community_sources": community_sources,
        "moderation_reports": {
            "total": moderation_reports.len(),
            "queued": moderation_queued,
            "failed": moderation_failed,
            "submitted": moderation_reports.len() - moderation_queued - moderation_failed,
        },
        "reliability": state.reliability.report(),
        "audio": {
            "path": state.audio_graph.audio_path(),
            "bit_perfect": state.audio_graph.bit_perfect_status(None),
            "stats": state.audio_graph.stats(),
        },
        "event_sequence": state.events.latest_sequence(),
    }))
}

#[derive(Debug, Deserialize, Default)]
struct EventsQuery {
    #[serde(default)]
    after: Option<u64>,
}

async fn events(
    State(state): State<Arc<AppState>>,
    Query(query): Query<EventsQuery>,
) -> Sse<impl futures::Stream<Item = Result<SseEvent, Infallible>>> {
    let latest = state.events.latest_sequence();
    let initial_type = if query.after.is_some_and(|after| after != latest) {
        "snapshot.required"
    } else {
        "stream.ready"
    };
    let initial = futures::stream::once(async move {
        Ok(SseEvent::default().event(initial_type).data(
            serde_json::json!({
                "schema_version": 1,
                "sequence": latest,
                "event_type": initial_type,
                "snapshot_endpoints": ["/v1/transfers", "/v1/plugins", "/v1/audio/path"]
            })
            .to_string(),
        ))
    });
    let receiver = state.events.subscribe_versioned();
    let event_bus = state.events.clone();
    let live = futures::stream::unfold(
        (receiver, event_bus),
        |(mut receiver, event_bus)| async move {
            match receiver.recv().await {
                Ok(event) => Some((
                    Ok(SseEvent::default()
                        .id(event.sequence.to_string())
                        .event(event.event_type)
                        .data(serde_json::to_string(&event).unwrap_or_else(|_| "{}".into()))),
                    (receiver, event_bus),
                )),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => Some((
                    Ok(SseEvent::default().event("snapshot.required").data(
                        serde_json::json!({
                            "schema_version": 1,
                            "sequence": event_bus.latest_sequence(),
                            "event_type": "snapshot.required",
                            "skipped": skipped,
                            "snapshot_endpoints": ["/v1/transfers", "/v1/plugins", "/v1/audio/path"]
                        })
                        .to_string(),
                    )),
                    (receiver, event_bus),
                )),
                Err(tokio::sync::broadcast::error::RecvError::Closed) => None,
            }
        },
    );
    Sse::new(initial.chain(live)).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}

async fn node_status(State(state): State<Arc<AppState>>) -> Json<jimmusic_protocol::NodeStatusV1> {
    Json(state.node_status().await)
}

async fn node_peers(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let status = state.node_status().await;
    Json(serde_json::json!({
        "schema_version": 1,
        "peer_id": status.peer_id,
        "listen_addresses": status.listen_addresses,
        "peers": status.peers,
        "connected": status.connected_peers,
        "routing_status": status.routing_status,
        "limitations": status.limitations,
    }))
}

#[derive(Debug, Deserialize)]
struct ConnectPeerRequest {
    address: String,
}

async fn connect_node_peer(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ConnectPeerRequest>,
) -> ApiResult<serde_json::Value> {
    let embedded = state.embedded_node().await.ok_or_else(|| {
        ApiError::conflict("node", "connect_peer", "embedded IPFS node is not running")
    })?;
    embedded.connect(&request.address).await.map_err(|error| {
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            "peer_connect_failed",
            "node",
            "connect_peer",
            error,
            true,
        )
    })?;
    state.events.publish(Event::NodeChanged {
        state: "peer_connected".into(),
    });
    Ok(Json(serde_json::json!({
        "schema_version": 1,
        "connected": true,
        "address": request.address,
        "status": embedded.status().await.map_err(|error| {
            ApiError::conflict("node", "connect_peer_status", error)
        })?,
    })))
}

async fn disconnect_node_peer(
    State(state): State<Arc<AppState>>,
    Path(peer_id): Path<String>,
) -> ApiResult<serde_json::Value> {
    let embedded = state.embedded_node().await.ok_or_else(|| {
        ApiError::conflict(
            "node",
            "disconnect_peer",
            "embedded IPFS node is not running",
        )
    })?;
    embedded
        .disconnect(&peer_id)
        .await
        .map_err(|error| ApiError::conflict("node", "disconnect_peer", error))?;
    state.events.publish(Event::NodeChanged {
        state: "peer_disconnected".into(),
    });
    Ok(Json(serde_json::json!({
        "schema_version": 1,
        "peer_id": peer_id,
        "connected": false,
    })))
}

async fn get_node_config(State(state): State<Arc<AppState>>) -> Json<NodeConfig> {
    Json(state.node.config())
}

async fn list_pins(State(state): State<Arc<AppState>>) -> Json<Vec<serde_json::Value>> {
    Json(
        state
            .node
            .list_pins()
            .into_iter()
            .map(|cid| {
                let health = state.node.provider_health(&cid);
                serde_json::json!({
                    "cid": cid,
                    "health": health,
                })
            })
            .collect(),
    )
}

async fn set_node_config(
    State(state): State<Arc<AppState>>,
    Json(config): Json<NodeConfig>,
) -> ApiResult<NodeConfig> {
    // 上传限速目前无法在内嵌 Bitswap 服务中强制执行（rust-ipfs 无带宽
    // 节流），按 PROD-004 显式拒绝而不是静默接受后不生效。
    if config.upload_limit_bytes_per_second.is_some() {
        return Err(ApiError::unsupported(
            "node",
            "set_config",
            "upload rate limiting is not supported",
            "the embedded Bitswap server (rust-ipfs 0.16) has no bandwidth ".to_string()
                + "throttle; uploads are served unthrottled. Leave the limit "
                + "unset instead of expecting silent enforcement.",
        ));
    }
    let concurrency = config.max_concurrent_transfers as usize;
    state
        .node
        .set_config(config.clone())
        .map_err(|error| ApiError::bad_request("node", "set_config", error))?;
    *state.transfer_slots.write().await = Arc::new(tokio::sync::Semaphore::new(concurrency));
    // NOD-006：网络类别变化 → 暂停/恢复受策略约束的传输，并重新调度恢复的任务。
    let effect = state
        .transfers
        .apply_network_class(
            config.network_class.as_deref(),
            config.metered_network_allowed,
            now(),
        )
        .map_err(|error| ApiError::bad_request("transfer", "network_policy", error))?;
    for task in effect.resumed {
        crate::transfer_runner::spawn(state.clone(), task.task_id);
    }
    state.events.publish(Event::NodeChanged {
        state: "configured".into(),
    });
    Ok(Json(config))
}

async fn pin(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(cid): Path<String>,
) -> ApiResult<serde_json::Value> {
    if let Some(embedded) = state.embedded_node().await {
        let bytes = state
            .node
            .cat(&cid)
            .map_err(|_| ApiError::not_found("node", "pin", &cid))?;
        embedded
            .put_block(&cid, &bytes, true)
            .await
            .map_err(|error| ApiError::conflict("node", "pin_network_block", error))?;
    }
    let request_id = request_id(&headers, None)?;
    let fingerprint = crate::state::sha256_hex(cid.as_bytes());
    let (response, replayed) = state
        .idempotency
        .execute("node.pin", &request_id, &fingerprint, || {
            state.node.pin(&cid).map_err(|error| error.to_string())?;
            Ok(serde_json::json!({
                "cid": cid,
                "pinned": true,
                "health": state.node.provider_health(&cid),
            }))
        })
        .map_err(|error| map_idempotency("node", "pin", error))?;
    if !replayed {
        // 第三方 Pin 服务（DST-009）：本地 Pin 成功后异步推送。
        spawn_remote_pins(&state, cid);
    }
    Ok(Json(response))
}

async fn unpin(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(cid): Path<String>,
) -> ApiResult<serde_json::Value> {
    let request_id = request_id(&headers, None)?;
    let fingerprint = crate::state::sha256_hex(cid.as_bytes());
    let (response, _) = state
        .idempotency
        .execute("node.unpin", &request_id, &fingerprint, || {
            state.node.unpin(&cid).map_err(|error| error.to_string())?;
            Ok(serde_json::json!({"cid": cid, "pinned": false}))
        })
        .map_err(|error| map_idempotency("node", "unpin", error))?;
    if let Some(embedded) = state.embedded_node().await {
        embedded
            .unpin(&cid)
            .await
            .map_err(|error| ApiError::conflict("node", "unpin_network_block", error))?;
    }
    Ok(Json(response))
}

#[derive(Debug, Deserialize)]
struct CreateTransferRequest {
    request_id: Option<String>,
    kind: TransferKind,
    target_cid: String,
    destination: Option<String>,
    network_policy: NetworkPolicyV1,
    #[serde(default)]
    priority: i16,
}

async fn create_transfer(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<CreateTransferRequest>,
) -> ApiResult<jimmusic_protocol::TransferTaskV1> {
    let request_id = request_id(&headers, request.request_id.as_deref())?;
    let task = state
        .transfers
        .create_with_priority(
            &request_id,
            request.kind,
            request.target_cid,
            request.destination,
            request.network_policy,
            request.priority,
        )
        .map_err(|error| ApiError::bad_request("transfer", "create", error))?;
    // NOD-006：新任务也要服从当前网络类别策略（仅 Wi-Fi / 计量开关）。
    apply_current_network_class(&state).await?;
    let task = state
        .transfers
        .get(&task.task_id)
        .expect("task created above");
    if task.state == jimmusic_protocol::TransferState::Queued {
        crate::transfer_runner::spawn(state.clone(), task.task_id.clone());
    }
    Ok(Json(task))
}

/// 按当前节点配置应用网络类别策略并重新调度被自动恢复的任务（NOD-006）。
async fn apply_current_network_class(state: &Arc<AppState>) -> Result<(), ApiError> {
    let config = state.node.config();
    let effect = state
        .transfers
        .apply_network_class(
            config.network_class.as_deref(),
            config.metered_network_allowed,
            now(),
        )
        .map_err(|error| ApiError::bad_request("transfer", "network_policy", error))?;
    for task in effect.resumed {
        crate::transfer_runner::spawn(state.clone(), task.task_id);
    }
    Ok(())
}

/// DST-009：把 CID 异步推送给所有显式配置的第三方 Kubo 兼容 Pin 服务。
/// 失败仅记日志（第三方服务可用性不影响本地操作结果）。
fn spawn_remote_pins(state: &Arc<AppState>, cid: String) {
    let services = state.node.config().pin_services;
    if services.is_empty() {
        return;
    }
    tokio::spawn(async move {
        for service in services {
            let client = app_core::IpfsClient::new(service);
            if let Err(error) = client.pin_add(&cid).await {
                tracing::warn!(%cid, %error, "third-party pin service failed");
            }
        }
    });
}

/// PLG-009：把当前生效的 Revoke 策略应用到已安装插件。
/// 目标 CID 与各版本 manifest CID 匹配；撤销后记录进入 Revoked 状态并推送事件。
/// 幂等：重复调用不会重复产生副作用。
fn apply_policy_revocations(state: &Arc<AppState>) {
    let targets = state.community.active_revoke_targets(now());
    for cid in targets {
        match state.lifecycle.revoke_release(&cid) {
            Ok(records) => {
                for record in records {
                    state.events.publish(Event::PluginChanged {
                        plugin_id: record.plugin_id,
                        state: "revoked".into(),
                        version: record.active_version,
                    });
                }
            }
            Err(error) => tracing::warn!(%error, %cid, "policy revocation could not be applied"),
        }
    }
}

async fn list_transfers(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<jimmusic_protocol::TransferTaskV1>> {
    Json(state.transfers.list())
}

async fn get_transfer(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<jimmusic_protocol::TransferTaskV1> {
    state
        .transfers
        .get(&id)
        .map(Json)
        .ok_or_else(|| ApiError::not_found("transfer", "get", &id))
}

async fn pause_transfer(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<jimmusic_protocol::TransferTaskV1> {
    Ok(Json(state.transfers.pause(&id).map_err(|error| {
        ApiError::conflict("transfer", "pause", error)
    })?))
}

async fn resume_transfer(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<jimmusic_protocol::TransferTaskV1> {
    state
        .transfers
        .resume(&id)
        .map_err(|error| ApiError::conflict("transfer", "resume", error))?;
    // 手动恢复同样服从网络策略：受限时会被立即重新暂停并给出结构化原因。
    apply_current_network_class(&state).await?;
    let task = state.transfers.get(&id).expect("task resumed above");
    if task.state == jimmusic_protocol::TransferState::Queued {
        crate::transfer_runner::spawn(state, task.task_id.clone());
    }
    Ok(Json(task))
}

async fn cancel_transfer(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<jimmusic_protocol::TransferTaskV1> {
    Ok(Json(state.transfers.cancel(&id).map_err(|error| {
        ApiError::conflict("transfer", "cancel", error)
    })?))
}

async fn retry_transfer(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<jimmusic_protocol::TransferTaskV1> {
    state
        .transfers
        .retry(&id)
        .map_err(|error| ApiError::conflict("transfer", "retry", error))?;
    apply_current_network_class(&state).await?;
    let task = state.transfers.get(&id).expect("task retried above");
    if task.state == jimmusic_protocol::TransferState::Queued {
        crate::transfer_runner::spawn(state, task.task_id.clone());
    }
    Ok(Json(task))
}

#[derive(Debug, Deserialize)]
struct TransferPriorityRequest {
    priority: i16,
}

async fn set_transfer_priority(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(request): Json<TransferPriorityRequest>,
) -> ApiResult<jimmusic_protocol::TransferTaskV1> {
    Ok(Json(
        state
            .transfers
            .set_priority(&id, request.priority)
            .map_err(|error| ApiError::conflict("transfer", "set_priority", error))?,
    ))
}

/// 边下边播流端点（DST-007）：把传输任务正在写入的 part 文件以有界块流出。
///
/// - 支持 `Range: bytes=start-end`（单范围），未知总长用 `*` 表示；
/// - 下载前沿（EOF）处轮询等待写入者增长文件，不占用无限内存；
/// - 任务进入终结状态后把已落盘字节服务完并结束；已完成任务可用该端点
///   继续服务完整 part 直到下次清理（客户端届时应切到已提交的离线源）；
/// - part 文件按 64 KiB 块读取，路径由 `tr_` + 24 hex 的任务 ID 构成，无穿越风险。
const TRANSFER_STREAM_CHUNK: usize = 64 * 1024;
const TRANSFER_STREAM_POLL: Duration = Duration::from_millis(150);

async fn stream_transfer(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(task_id): Path<String>,
) -> Result<Response, ApiError> {
    state.sweep_transfer_parts(Some(&task_id));
    let task = state
        .transfers
        .get(&task_id)
        .ok_or_else(|| ApiError::not_found("transfer", "stream", &task_id))?;
    if matches!(
        task.state,
        TransferState::Failed | TransferState::Cancelled | TransferState::IntegrityFailed
    ) {
        return Err(ApiError::conflict(
            "transfer",
            "stream",
            format!(
                "transfer task ended in state {}",
                transfer_state_name(task.state)
            ),
        ));
    }
    let range = parse_byte_range(&headers);
    let (start, end_exclusive) = match range {
        None => (0u64, None),
        Some((start, end)) => (start, end.map(|end| end.saturating_add(1))),
    };
    if start > state.node.config().storage_limit_bytes {
        return Err(ApiError::bad_request(
            "transfer",
            "stream",
            "range start beyond storage limit",
        ));
    }
    let part_path = state
        .repo_dir
        .join("transfer-parts")
        .join(format!("{task_id}.part"));
    let (tx, rx) = futures::channel::mpsc::channel::<Result<Vec<u8>, std::io::Error>>(8);
    tokio::spawn(part_stream_pump(
        state.clone(),
        task_id,
        part_path,
        start,
        end_exclusive,
        tx,
    ));
    let mut builder = Response::builder()
        .header("accept-ranges", "bytes")
        .header("content-type", "application/octet-stream")
        .header("x-jimmusic-stream-source", "transfer-part");
    if let Some((start, end)) = range {
        let last = end.map_or_else(|| "*".to_string(), |end| end.to_string());
        builder = builder
            .status(StatusCode::PARTIAL_CONTENT)
            .header("content-range", format!("bytes {start}-{last}/*"));
        if let Some(end) = end {
            builder = builder.header("content-length", (end - start + 1).to_string());
        }
    }
    builder
        .body(Body::from_stream(rx))
        .map_err(|error| ApiError::bad_request("transfer", "stream", error))
}

/// 把 part 文件按块推入通道：跟随增长、尊重范围、终结状态后收尾。
#[allow(clippy::too_many_arguments)]
async fn part_stream_pump(
    state: Arc<AppState>,
    task_id: String,
    part_path: std::path::PathBuf,
    start: u64,
    end_exclusive: Option<u64>,
    mut tx: futures::channel::mpsc::Sender<Result<Vec<u8>, std::io::Error>>,
) {
    if let Err(error) =
        part_stream_loop(&state, &task_id, &part_path, start, end_exclusive, &mut tx).await
    {
        let _ = tx.send(Err(error)).await;
    }
}

async fn part_stream_loop(
    state: &AppState,
    task_id: &str,
    part_path: &std::path::Path,
    mut offset: u64,
    end_exclusive: Option<u64>,
    tx: &mut futures::channel::mpsc::Sender<Result<Vec<u8>, std::io::Error>>,
) -> std::io::Result<()> {
    let mut drained = false;
    loop {
        if end_exclusive.is_some_and(|end| offset >= end) {
            return Ok(());
        }
        let mut file = match tokio::fs::File::open(part_path).await {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if transfer_stream_active(state, task_id) {
                    // part 尚未创建或已重建：等待写入者。
                    tokio::time::sleep(TRANSFER_STREAM_POLL).await;
                    continue;
                }
                return Ok(()); // 任务终结且文件已清理：正常结束
            }
            Err(error) => return Err(error),
        };
        file.seek(SeekFrom::Start(offset)).await?;
        let mut buffer = vec![0u8; TRANSFER_STREAM_CHUNK];
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            if transfer_stream_active(state, task_id) {
                // 下载前沿：文件仍在增长，继续轮询。
                tokio::time::sleep(TRANSFER_STREAM_POLL).await;
                continue;
            }
            if !drained {
                // 任务终结：文件不再增长，短暂等待写入者收尾后做最后读取。
                drained = true;
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
            return Ok(());
        }
        buffer.truncate(read);
        if end_exclusive.is_some_and(|end| offset.saturating_add(read as u64) > end) {
            buffer.truncate((end_exclusive.unwrap() - offset) as usize);
        }
        offset = offset.saturating_add(buffer.len() as u64);
        if tx.send(Ok(buffer)).await.is_err() {
            return Ok(()); // 客户端已断开
        }
    }
}

/// 任务是否仍可能让 part 文件继续增长。
fn transfer_stream_active(state: &AppState, task_id: &str) -> bool {
    state.transfers.get(task_id).is_some_and(|task| {
        matches!(
            task.state,
            TransferState::Queued
                | TransferState::Resolving
                | TransferState::Transferring
                | TransferState::Paused
                | TransferState::Verifying
                | TransferState::Committing
        )
    })
}

/// 解析单范围 `bytes=start-end` / `bytes=start-`；无效或多范围返回 None（整段）。
fn parse_byte_range(headers: &HeaderMap) -> Option<(u64, Option<u64>)> {
    let value = headers.get(RANGE)?.to_str().ok()?;
    let spec = value.strip_prefix("bytes=")?.trim();
    let mut parts = spec.split(',');
    let part = parts.next()?.trim();
    if parts.next().is_some() {
        return None;
    }
    let (start_text, end_text) = part.split_once('-')?;
    let start: u64 = start_text.parse().ok()?;
    let end = if end_text.is_empty() {
        None
    } else {
        Some(end_text.parse().ok()?)
    };
    if end.is_some_and(|end| end < start) {
        return None;
    }
    Some((start, end))
}

fn transfer_state_name(state: TransferState) -> &'static str {
    match state {
        TransferState::Queued => "queued",
        TransferState::Resolving => "resolving",
        TransferState::Transferring => "transferring",
        TransferState::Paused => "paused",
        TransferState::Verifying => "verifying",
        TransferState::Committing => "committing",
        TransferState::Completed => "completed",
        TransferState::Failed => "failed",
        TransferState::Cancelled => "cancelled",
        TransferState::IntegrityFailed => "integrity_failed",
    }
}

async fn register_identity(
    State(state): State<Arc<AppState>>,
    Json(identity): Json<PublisherIdentityV1>,
) -> ApiResult<serde_json::Value> {
    let cid = state
        .publications
        .register_identity(identity, &state.node)
        .map_err(|error| ApiError::bad_request("publication", "register_identity", error))?;
    Ok(Json(serde_json::json!({"identity_cid": cid})))
}

#[derive(Debug, Deserialize)]
struct IdentityGenerateRequest {
    display_name: String,
    passphrase: String,
}

#[derive(Debug, Deserialize)]
struct IdentityBundleRequest {
    display_name: String,
    passphrase: String,
    bundle: EncryptedIdentityBundleV1,
}

#[derive(Debug, Deserialize)]
struct IdentityRevokeRequest {
    display_name: String,
    passphrase: String,
    bundle: EncryptedIdentityBundleV1,
    #[serde(default)]
    revoked_at: Option<i64>,
}

async fn generate_identity(
    State(state): State<Arc<AppState>>,
    Json(request): Json<IdentityGenerateRequest>,
) -> ApiResult<serde_json::Value> {
    let timestamp = now();
    let vault = PublisherIdentityVault::generate(request.display_name, timestamp)
        .map_err(|error| ApiError::bad_request("identity", "generate", error))?;
    let bundle = vault
        .export(&request.passphrase)
        .map_err(|error| ApiError::bad_request("identity", "export", error))?;
    identity_response(&state, vault.identity().clone(), bundle)
}

async fn import_identity(
    State(state): State<Arc<AppState>>,
    Json(request): Json<IdentityBundleRequest>,
) -> ApiResult<serde_json::Value> {
    let vault = PublisherIdentityVault::import(
        &request.bundle,
        &request.passphrase,
        request.display_name,
        now(),
    )
    .map_err(|error| ApiError::bad_request("identity", "import", error))?;
    identity_response(&state, vault.identity().clone(), request.bundle)
}

async fn rotate_identity(
    State(state): State<Arc<AppState>>,
    Json(request): Json<IdentityBundleRequest>,
) -> ApiResult<serde_json::Value> {
    let timestamp = now();
    let current = PublisherIdentityVault::import(
        &request.bundle,
        &request.passphrase,
        request.display_name.clone(),
        timestamp,
    )
    .map_err(|error| ApiError::bad_request("identity", "unlock", error))?;
    let next = current
        .rotate(request.display_name, timestamp)
        .map_err(|error| ApiError::bad_request("identity", "rotate", error))?;
    let bundle = next
        .export(&request.passphrase)
        .map_err(|error| ApiError::bad_request("identity", "export", error))?;
    identity_response(&state, next.identity().clone(), bundle)
}

async fn revoke_identity(
    State(state): State<Arc<AppState>>,
    Json(request): Json<IdentityRevokeRequest>,
) -> ApiResult<serde_json::Value> {
    let timestamp = request.revoked_at.unwrap_or_else(now);
    let vault = PublisherIdentityVault::import(
        &request.bundle,
        &request.passphrase,
        request.display_name,
        timestamp,
    )
    .map_err(|error| ApiError::bad_request("identity", "unlock", error))?;
    let identity = vault.revoked_identity(timestamp);
    let bundle = vault
        .export_with_identity(&request.passphrase, identity.clone())
        .map_err(|error| ApiError::bad_request("identity", "export_revocation", error))?;
    identity_response(&state, identity, bundle)
}

fn identity_response(
    state: &AppState,
    identity: PublisherIdentityV1,
    bundle: EncryptedIdentityBundleV1,
) -> ApiResult<serde_json::Value> {
    let cid = state
        .publications
        .register_identity(identity.clone(), &state.node)
        .map_err(|error| ApiError::bad_request("identity", "register", error))?;
    Ok(Json(serde_json::json!({
        "identity_cid": cid,
        "identity": identity,
        "encrypted_bundle": bundle,
    })))
}

#[derive(Debug, Deserialize)]
struct PublishRequest {
    #[serde(default)]
    request_id: Option<String>,
    manifest: MusicManifestV1,
    event: PublicationEventV1,
}

async fn publish(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<PublishRequest>,
) -> ApiResult<app_core::publication_service::PublicationReceipt> {
    let request_id = request_id(&headers, request.request_id.as_deref())?;
    let fingerprint = crate::state::sha256_hex(
        &serde_json::to_vec(&(&request.manifest, &request.event)).unwrap_or_default(),
    );
    let (receipt, replayed) = state
        .idempotency
        .execute("publication.publish", &request_id, &fingerprint, || {
            state
                .publications
                .publish(request.manifest, request.event, &state.node)
                .map_err(|error| error.to_string())
        })
        .map_err(|error| map_idempotency("publication", "publish", error))?;
    if !replayed {
        publish_publication_event(&state, &receipt);
        // DST-009：发布后把 Manifest CID 推送给配置的第三方 Pin 服务。
        if let Some(manifest_cid) = receipt.manifest_cid.clone() {
            spawn_remote_pins(&state, manifest_cid);
        }
    }
    Ok(Json(receipt))
}

#[derive(Debug, Deserialize)]
struct SignPublicationRequest {
    #[serde(default)]
    request_id: Option<String>,
    display_name: String,
    passphrase: String,
    bundle: EncryptedIdentityBundleV1,
    operation: PublicationEventType,
    #[serde(default)]
    manifest: Option<MusicManifestV1>,
    #[serde(default)]
    target_cid: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SignedPublicationResponse {
    identity_cid: String,
    receipt: app_core::publication_service::PublicationReceipt,
    #[serde(skip_serializing_if = "Option::is_none")]
    signed_manifest: Option<MusicManifestV1>,
    signed_event: PublicationEventV1,
}

/// Unlocks an encrypted publisher key only for the duration of this request,
/// constructs the current feed link, signs canonical objects and commits them.
async fn sign_publication(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<SignPublicationRequest>,
) -> ApiResult<SignedPublicationResponse> {
    let request_id = request_id(&headers, request.request_id.as_deref())?;
    let fingerprint = crate::state::sha256_hex(
        &serde_json::to_vec(&(
            &request.bundle,
            &request.display_name,
            &request.operation,
            &request.manifest,
            &request.target_cid,
            &request.reason,
            &request.timestamp,
        ))
        .unwrap_or_default(),
    );
    let timestamp = request.timestamp.unwrap_or_else(now);
    let vault = PublisherIdentityVault::import(
        &request.bundle,
        &request.passphrase,
        request.display_name,
        timestamp,
    )
    .map_err(|error| ApiError::bad_request("publication", "unlock_identity", error))?;
    let (response, replayed) = state
        .idempotency
        .execute("publication.sign", &request_id, &fingerprint, || {
            if vault.identity().revoked_at.is_some() {
                return Err("publisher identity is revoked".into());
            }
            let identity_cid = state
                .publications
                .register_identity(vault.identity().clone(), &state.node)
                .map_err(|error| error.to_string())?;
            let feed = state.publications.feed(&vault.identity().publisher_id);
            let sequence = feed.len() as u64;
            let previous_event_cid = feed.last().map(|entry| entry.cid.clone());
            let mut signed_manifest = match request.operation {
                PublicationEventType::Publish | PublicationEventType::Update => {
                    let mut manifest = request
                        .manifest
                        .ok_or_else(|| "publish/update requires a manifest".to_string())?;
                    manifest.publisher_identity_cid = identity_cid.clone();
                    manifest.publisher_signature = None;
                    manifest.publisher_signature = Some(
                        vault.sign_hex(
                            &manifest
                                .unsigned_bytes()
                                .map_err(|error| error.to_string())?,
                        ),
                    );
                    Some(manifest)
                }
                PublicationEventType::Tombstone => {
                    if request.manifest.is_some() {
                        return Err("tombstone must not include a manifest".into());
                    }
                    None
                }
            };
            let manifest_cid = signed_manifest
                .as_ref()
                .map(cid_v1_for)
                .transpose()
                .map_err(|error| error.to_string())?;
            let target_cid = if request.operation == PublicationEventType::Tombstone {
                Some(
                    request
                        .target_cid
                        .filter(|cid| !cid.trim().is_empty())
                        .ok_or_else(|| "tombstone requires target_cid".to_string())?,
                )
            } else {
                None
            };
            let mut event = PublicationEventV1 {
                schema_version: SCHEMA_V1,
                event_type: request.operation,
                publisher_id: vault.identity().publisher_id.clone(),
                sequence,
                previous_event_cid,
                manifest_cid,
                target_cid,
                timestamp,
                reason: request.reason.filter(|reason| !reason.trim().is_empty()),
                signature: None,
            };
            event.signature =
                Some(vault.sign_hex(&event.unsigned_bytes().map_err(|error| error.to_string())?));
            let response_manifest = signed_manifest.clone();
            let receipt = match request.operation {
                PublicationEventType::Publish | PublicationEventType::Update => state
                    .publications
                    .publish(
                        signed_manifest.take().expect("manifest constructed above"),
                        event.clone(),
                        &state.node,
                    )
                    .map_err(|error| error.to_string())?,
                PublicationEventType::Tombstone => state
                    .publications
                    .tombstone(event.clone(), &identity_cid, &state.node)
                    .map_err(|error| error.to_string())?,
            };
            Ok(SignedPublicationResponse {
                identity_cid,
                receipt,
                signed_manifest: response_manifest,
                signed_event: event,
            })
        })
        .map_err(|error| map_idempotency("publication", "sign", error))?;
    if !replayed {
        publish_publication_event(&state, &response.receipt);
    }
    Ok(Json(response))
}

async fn tombstone(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(identity_cid): Path<String>,
    Json(event): Json<PublicationEventV1>,
) -> ApiResult<app_core::publication_service::PublicationReceipt> {
    let request_id = request_id(&headers, None)?;
    let fingerprint =
        crate::state::sha256_hex(&serde_json::to_vec(&(&identity_cid, &event)).unwrap_or_default());
    let (receipt, replayed) = state
        .idempotency
        .execute("publication.tombstone", &request_id, &fingerprint, || {
            state
                .publications
                .tombstone(event, &identity_cid, &state.node)
                .map_err(|error| error.to_string())
        })
        .map_err(|error| map_idempotency("publication", "tombstone", error))?;
    if !replayed {
        publish_publication_event(&state, &receipt);
    }
    Ok(Json(receipt))
}

fn publish_publication_event(
    state: &AppState,
    receipt: &app_core::publication_service::PublicationReceipt,
) {
    state.events.publish(Event::PublicationChanged {
        publisher_id: receipt.publisher_id.clone(),
        event_cid: receipt.event_cid.clone(),
        sequence: receipt.sequence,
    });
}

#[derive(Debug, Deserialize)]
struct AddSourceRequest {
    manifest: CommunitySourceManifestV1,
    maintainer_public_key: String,
    #[serde(default)]
    trust_order: u32,
}

#[derive(Debug, Deserialize)]
struct ImportSourceRequest {
    locator: String,
    maintainer_public_key: String,
    #[serde(default)]
    trust_order: u32,
}

async fn list_sources(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<app_core::community_service::CommunitySourceRecord>> {
    Json(state.community.list_sources())
}

async fn add_source(
    State(state): State<Arc<AppState>>,
    Json(request): Json<AddSourceRequest>,
) -> ApiResult<app_core::community_service::CommunitySourceRecord> {
    let record = state
        .community
        .add_source(
            request.manifest,
            request.maintainer_public_key,
            &state.node,
            request.trust_order,
        )
        .map_err(|error| ApiError::bad_request("community", "add_source", error))?;
    state.events.publish(Event::CommunitySourceChanged {
        source_id: record.manifest.source_id.clone(),
        state: "added".into(),
    });
    Ok(Json(record))
}

async fn import_source(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ImportSourceRequest>,
) -> ApiResult<app_core::community_service::CommunitySourceRecord> {
    let locator = parse_community_locator(&request.locator)
        .ok_or_else(|| ApiError::bad_request("community", "import", "unsupported locator"))?;
    let bytes = match &locator {
        CommunityLocator::Cid(cid) => fetch_dag_object(&state, cid).await?,
        CommunityLocator::Ipns(name) => {
            let path = format!("/ipns/{name}");
            let bytes = if let Some(embedded) = state.embedded_node().await {
                match embedded.resolve_ipns_cid(name).await {
                    Ok(cid) => embedded.get_block(&cid).await.map_err(|error| {
                        ApiError::new(
                            StatusCode::BAD_GATEWAY,
                            "fetch_failed",
                            "community",
                            "fetch_resolved_ipns",
                            error,
                            true,
                        )
                    })?,
                    Err(_) => state.ipfs.cat(&path).await.map_err(|error| {
                        ApiError::new(
                            StatusCode::BAD_GATEWAY,
                            "fetch_failed",
                            "community",
                            "resolve_ipns",
                            error,
                            true,
                        )
                    })?,
                }
            } else {
                state.ipfs.cat(&path).await.map_err(|error| {
                    ApiError::new(
                        StatusCode::BAD_GATEWAY,
                        "fetch_failed",
                        "community",
                        "resolve_ipns",
                        error,
                        true,
                    )
                })?
            };
            if bytes.len() > jimmusic_protocol::ObjectLimits::default().max_bytes {
                return Err(ApiError::bad_request(
                    "community",
                    "resolve_ipns",
                    "community manifest exceeds protocol size limit",
                ));
            }
            bytes
        }
    };
    let manifest: CommunitySourceManifestV1 = jimmusic_protocol::decode_dag_cbor(&bytes)
        .map_err(|error| ApiError::bad_request("community", "decode_manifest", error))?;
    let record = state
        .community
        .add_source(
            manifest,
            request.maintainer_public_key,
            &state.node,
            request.trust_order,
        )
        .map_err(|error| ApiError::bad_request("community", "import", error))?;
    state.events.publish(Event::CommunitySourceChanged {
        source_id: record.manifest.source_id.clone(),
        state: "imported".into(),
    });
    Ok(Json(record))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CommunityLocator {
    Cid(String),
    Ipns(String),
}

fn parse_community_locator(value: &str) -> Option<CommunityLocator> {
    let value = value.trim();
    let target = if let Some(value) = value.strip_prefix("ipfs://") {
        CommunityLocator::Cid(value.trim_start_matches('/').to_string())
    } else if let Some(value) = value.strip_prefix("/ipfs/") {
        CommunityLocator::Cid(value.to_string())
    } else if let Some(value) = value.strip_prefix("ipns://") {
        CommunityLocator::Ipns(value.trim_start_matches('/').to_string())
    } else if let Some(value) = value.strip_prefix("/ipns/") {
        CommunityLocator::Ipns(value.to_string())
    } else if value.starts_with("jimmusic://") {
        let url = reqwest::Url::parse(value).ok()?;
        if url.host_str()? != "community" || url.query().is_some() || url.fragment().is_some() {
            return None;
        }
        let identifier = url.path().trim_start_matches('/');
        if let Some(name) = identifier.strip_prefix("ipns/") {
            CommunityLocator::Ipns(name.to_string())
        } else {
            CommunityLocator::Cid(identifier.to_string())
        }
    } else {
        CommunityLocator::Cid(value.to_string())
    };
    let identifier = match &target {
        CommunityLocator::Cid(value) | CommunityLocator::Ipns(value) => value,
    };
    if identifier.is_empty()
        || identifier.len() > 512
        || !identifier
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return None;
    }
    Some(target)
}

#[derive(Debug, Deserialize)]
struct SourceSwitches {
    catalog_enabled: bool,
    policy_enabled: bool,
}

async fn update_source(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(switches): Json<SourceSwitches>,
) -> ApiResult<app_core::community_service::CommunitySourceRecord> {
    let record = state
        .community
        .set_enabled(&id, switches.catalog_enabled, switches.policy_enabled)
        .map_err(|error| ApiError::bad_request("community", "set_enabled", error))?;
    state.events.publish(Event::CommunitySourceChanged {
        source_id: id,
        state: "configuration_changed".into(),
    });
    Ok(Json(record))
}

async fn remove_source(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    state
        .community
        .remove_source(&id)
        .map_err(|error| ApiError::bad_request("community", "remove", error))?;
    state.events.publish(Event::CommunitySourceChanged {
        source_id: id.clone(),
        state: "removed".into(),
    });
    Ok(Json(serde_json::json!({"removed": id})))
}

async fn refresh_source(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    let source = state
        .community
        .list_sources()
        .into_iter()
        .find(|source| source.manifest.source_id == id)
        .ok_or_else(|| ApiError::not_found("community", "refresh", &id))?;
    let mut catalog_ingested = 0usize;
    if source.catalog_enabled {
        if let Some(head) = source.manifest.catalog_head.as_deref() {
            let mut events = Vec::new();
            let mut current = Some(head.to_string());
            while let Some(cid) = current {
                if events.len() >= 10_000 {
                    return Err(ApiError::bad_request(
                        "community",
                        "refresh_catalog",
                        "catalog chain exceeds 10,000 event limit",
                    ));
                }
                let bytes = fetch_dag_object(&state, &cid).await?;
                let event: CatalogEventV1 = jimmusic_protocol::decode_dag_cbor(&bytes)
                    .map_err(|error| ApiError::bad_request("community", "decode_catalog", error))?;
                if source
                    .last_catalog_sequence
                    .is_some_and(|last| event.sequence <= last)
                {
                    break;
                }
                current = event.previous_event_cid.clone();
                events.push(event);
            }
            events.reverse();
            for event in events {
                state
                    .community
                    .ingest_catalog(&id, event, &state.node)
                    .map_err(|error| ApiError::bad_request("community", "ingest_catalog", error))?;
                catalog_ingested += 1;
            }
        }
    }
    let mut policy_ingested = 0usize;
    if source.policy_enabled {
        if let Some(head) = source.manifest.policy_head.as_deref() {
            let mut events = Vec::new();
            let mut current = Some(head.to_string());
            while let Some(cid) = current {
                if events.len() >= 10_000 {
                    return Err(ApiError::bad_request(
                        "community",
                        "refresh_policy",
                        "policy chain exceeds 10,000 event limit",
                    ));
                }
                let bytes = fetch_dag_object(&state, &cid).await?;
                let event: jimmusic_protocol::PolicyEventV1 =
                    jimmusic_protocol::decode_dag_cbor(&bytes).map_err(|error| {
                        ApiError::bad_request("community", "decode_policy", error)
                    })?;
                if source
                    .last_policy_sequence
                    .is_some_and(|last| event.sequence <= last)
                {
                    break;
                }
                current = event.previous_event_cid.clone();
                events.push(event);
            }
            events.reverse();
            for event in events {
                state
                    .community
                    .ingest_policy(&id, event, &state.node)
                    .map_err(|error| ApiError::bad_request("community", "ingest_policy", error))?;
                policy_ingested += 1;
            }
        }
    }
    state.events.publish(Event::CommunitySourceChanged {
        source_id: id.clone(),
        state: "refreshed".into(),
    });
    apply_policy_revocations(&state);
    Ok(Json(serde_json::json!({
        "source_id": id,
        "catalog_ingested": catalog_ingested,
        "policy_ingested": policy_ingested,
        "status": "refreshed",
    })))
}

async fn fetch_dag_object(state: &AppState, cid: &str) -> Result<Vec<u8>, ApiError> {
    if let Ok(bytes) = state.node.cat(cid) {
        return Ok(bytes);
    }
    if let Some(embedded) = state.embedded_node().await {
        if let Ok(bytes) = embedded.get_block(cid).await {
            if bytes.len() > jimmusic_protocol::ObjectLimits::default().max_bytes {
                return Err(ApiError::bad_request(
                    "community",
                    "fetch_feed_object",
                    "feed object exceeds protocol size limit",
                ));
            }
            state
                .node
                .put_verified(cid, jimmusic_protocol::DAG_CBOR_CODEC, &bytes, false, false)
                .map_err(|error| ApiError::bad_request("community", "verify_feed_object", error))?;
            return Ok(bytes);
        }
    }
    let bytes = state.ipfs.cat(cid).await.map_err(|error| {
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            "fetch_failed",
            "community",
            "fetch_feed_object",
            error,
            true,
        )
    })?;
    if bytes.len() > jimmusic_protocol::ObjectLimits::default().max_bytes {
        return Err(ApiError::bad_request(
            "community",
            "fetch_feed_object",
            "feed object exceeds protocol size limit",
        ));
    }
    state
        .node
        .put_verified(cid, jimmusic_protocol::DAG_CBOR_CODEC, &bytes, false, false)
        .map_err(|error| ApiError::bad_request("community", "verify_feed_object", error))?;
    Ok(bytes)
}

async fn ingest_catalog(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(event): Json<CatalogEventV1>,
) -> ApiResult<serde_json::Value> {
    let cid = state
        .community
        .ingest_catalog(&id, event, &state.node)
        .map_err(|error| ApiError::bad_request("community", "ingest_catalog", error))?;
    state.events.publish(Event::CommunitySourceChanged {
        source_id: id,
        state: "catalog_ingested".into(),
    });
    Ok(Json(serde_json::json!({"event_cid": cid})))
}

async fn ingest_policy(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(event): Json<jimmusic_protocol::PolicyEventV1>,
) -> ApiResult<serde_json::Value> {
    let target = event.target.clone();
    let decision = serde_json::to_value(event.action)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "changed".into());
    let cid = state
        .community
        .ingest_policy(&id, event, &state.node)
        .map_err(|error| ApiError::bad_request("community", "ingest_policy", error))?;
    state
        .events
        .publish(Event::PolicyChanged { target, decision });
    apply_policy_revocations(&state);
    Ok(Json(serde_json::json!({"event_cid": cid})))
}

async fn apply_maintainer_key_event(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(event): Json<MaintainerKeyEventV1>,
) -> ApiResult<serde_json::Value> {
    let action = format!("{:?}", event.action).to_lowercase();
    let cid = state
        .community
        .apply_maintainer_key_event(&id, event, &state.node)
        .map_err(|error| ApiError::bad_request("community", "maintainer_key", error))?;
    state.events.publish(Event::CommunitySourceChanged {
        source_id: id,
        state: format!("maintainer_key_{action}"),
    });
    Ok(Json(
        serde_json::json!({"event_cid": cid, "action": action}),
    ))
}

/// 快照响应（未压缩形态）的字节上限；超出时拒绝并提示使用 gzip 传输。
const MAX_SNAPSHOT_BYTES: usize = 32 * 1024 * 1024;

/// 社区源 Catalog/Policy 快照（COM-012）。
///
/// 快照本身是"每目标最新未过期事件"的紧凑形态并锚定到签名事件链头；
/// 传输层支持 gzip：请求带 `Accept-Encoding: gzip` 时压缩返回。两种形态都携带
/// 未压缩字节的 SHA-256（`x-snapshot-sha256`）与长度（`x-snapshot-bytes`），
/// 压缩形态另有 `x-snapshot-compressed-bytes`，供客户端校验完整性。
async fn source_snapshot(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let timestamp = now();
    let catalog = state
        .community
        .snapshot_catalog(&id, timestamp)
        .map_err(|_| ApiError::not_found("community", "snapshot", &id))?;
    let policy = state
        .community
        .snapshot_policy(&id, timestamp)
        .map_err(|_| ApiError::not_found("community", "snapshot", &id))?;
    let plain = serde_json::to_vec(&serde_json::json!({
        "source_id": id,
        "catalog": catalog,
        "policy": policy,
    }))
    .map_err(|error| ApiError::bad_request("community", "snapshot", error))?;
    if plain.len() > MAX_SNAPSHOT_BYTES {
        return Err(ApiError::payload_too_large(
            "community",
            "snapshot",
            format!(
                "snapshot exceeds {MAX_SNAPSHOT_BYTES} bytes uncompressed; retry with `Accept-Encoding: gzip`"
            ),
        ));
    }
    let digest = crate::state::sha256_hex(&plain);
    let builder = Response::builder()
        .header("content-type", "application/json")
        .header("x-snapshot-sha256", digest)
        .header("x-snapshot-bytes", plain.len().to_string());
    let wants_gzip = headers
        .get(ACCEPT_ENCODING)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|part| part.trim().eq_ignore_ascii_case("gzip"))
        });
    if wants_gzip {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(&plain)
            .map_err(|error| ApiError::bad_request("community", "snapshot", error))?;
        let compressed = encoder
            .finish()
            .map_err(|error| ApiError::bad_request("community", "snapshot", error))?;
        return builder
            .header(CONTENT_ENCODING, "gzip")
            .header("x-snapshot-compressed-bytes", compressed.len().to_string())
            .body(Body::from(compressed))
            .map_err(|error| ApiError::bad_request("community", "snapshot", error));
    }
    builder
        .body(Body::from(plain))
        .map_err(|error| ApiError::bad_request("community", "snapshot", error))
}

#[derive(Debug, Deserialize)]
struct QueueModerationReportRequest {
    report: ModerationReportV1,
    #[serde(default)]
    submit_now: bool,
    #[serde(default)]
    encrypt_for_recipient: bool,
}

async fn list_moderation_reports(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<app_core::community_service::ModerationReportRecord>> {
    Json(state.community.list_moderation_reports())
}

async fn queue_moderation_report(
    State(state): State<Arc<AppState>>,
    Json(request): Json<QueueModerationReportRequest>,
) -> ApiResult<app_core::community_service::ModerationReportRecord> {
    let mut report = request.report;
    let source_id = report.recipient_source_id.clone();
    let report_id = report.report_id.clone();
    if request.encrypt_for_recipient {
        if report.encrypted_envelope.is_some() {
            return Err(ApiError::bad_request(
                "community",
                "encrypt_report",
                "report already contains an encrypted envelope",
            ));
        }
        let recipient_key = state
            .community
            .list_sources()
            .into_iter()
            .find(|source| source.manifest.source_id == source_id)
            .and_then(|source| source.manifest.report_encryption_public_key)
            .ok_or_else(|| {
                ApiError::bad_request(
                    "community",
                    "encrypt_report",
                    "recipient source does not publish an X25519 report key",
                )
            })?;
        report.encrypted_envelope = Some(
            app_core::crypto::encrypt_moderation_report(&recipient_key, &report)
                .map_err(|error| ApiError::bad_request("community", "encrypt_report", error))?,
        );
    }
    let mut record = state
        .community
        .queue_moderation_report(report, &state.node)
        .map_err(|error| ApiError::bad_request("community", "queue_report", error))?;
    state.events.publish(Event::CommunitySourceChanged {
        source_id,
        state: "moderation_report_queued".into(),
    });
    if request.submit_now {
        record = deliver_moderation_report(&state, &report_id).await?;
    }
    Ok(Json(record))
}

async fn retry_moderation_report(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<app_core::community_service::ModerationReportRecord> {
    Ok(Json(deliver_moderation_report(&state, &id).await?))
}

async fn deliver_moderation_report(
    state: &AppState,
    report_id: &str,
) -> Result<app_core::community_service::ModerationReportRecord, ApiError> {
    let _delivery = state.moderation_scheduler.lock().await;
    let record = state
        .community
        .moderation_report(report_id)
        .map_err(|error| ApiError::not_found("community", "report", &error.to_string()))?;
    if record.status == app_core::community_service::ModerationReportStatus::Submitted {
        return Ok(record);
    }
    let source = state
        .community
        .list_sources()
        .into_iter()
        .find(|source| source.manifest.source_id == record.report.recipient_source_id)
        .ok_or_else(|| {
            ApiError::not_found(
                "community",
                "report_recipient",
                &record.report.recipient_source_id,
            )
        })?;
    let endpoint = source.manifest.report_endpoint.as_deref();
    let endpoint = endpoint.filter(|value| report_endpoint_allowed(value));
    let payload = if let Some(envelope) = record.report.encrypted_envelope.as_deref() {
        serde_json::json!({
            "schema_version": SCHEMA_V1,
            "report_id": record.report.report_id,
            "recipient_source_id": record.report.recipient_source_id,
            "created_at": record.report.created_at,
            "encrypted_envelope": envelope,
        })
    } else {
        serde_json::to_value(&record.report).expect("moderation report is serializable")
    };
    let result = match endpoint {
        Some(endpoint) => {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .map_err(|error| ApiError::bad_request("community", "report_client", error))?;
            client
                .post(endpoint)
                .json(&payload)
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .map(|_| ())
                .map_err(|error| error.to_string())
        }
        None => Err("recipient has no allowed HTTPS report endpoint".into()),
    };
    let (succeeded, error) = match result {
        Ok(()) => (true, None),
        Err(error) => (false, Some(error.chars().take(1_000).collect())),
    };
    state
        .community
        .record_report_attempt(report_id, succeeded, now(), error)
        .map_err(|error| ApiError::bad_request("community", "record_report_attempt", error))
}

pub(crate) async fn retry_due_moderation_reports(state: Arc<AppState>) {
    for report in state.community.due_moderation_reports(now()) {
        if let Err(error) = deliver_moderation_report(&state, &report.report.report_id).await {
            tracing::warn!(
                report_id = %report.report.report_id,
                error = %error.body.message,
                "automatic moderation report retry failed"
            );
        }
    }
}

fn report_endpoint_allowed(value: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(value) else {
        return false;
    };
    if !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    if url.scheme() == "https" {
        return true;
    }
    if url.scheme() != "http" {
        return false;
    }
    url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    })
}

async fn policy_decision(
    State(state): State<Arc<AppState>>,
    Path(target): Path<String>,
) -> Json<app_core::community_service::PolicyDecision> {
    Json(state.community.policy_decision(&target, now()))
}

#[derive(Debug, Deserialize, Default)]
struct SearchQuery {
    #[serde(default)]
    q: String,
}

async fn search(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SearchQuery>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "local": state.library.search(&query.q),
        "community": state.community.search_catalog(&query.q, now()),
    }))
}

async fn list_plugins(State(state): State<Arc<AppState>>) -> Json<Vec<crate::PluginRuntimeRecord>> {
    Json(state.lifecycle.list())
}

async fn get_plugin(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<crate::PluginRuntimeRecord> {
    state
        .lifecycle
        .get(&id)
        .map(Json)
        .ok_or_else(|| ApiError::not_found("plugin", "get", &id))
}

#[derive(Debug, Deserialize)]
struct InstallPluginRequest {
    request_id: Option<String>,
    manifest: PluginManifestV1,
    public_key: String,
    #[serde(default)]
    artifact_location: Option<String>,
    #[serde(default)]
    granted_permissions: BTreeSet<PluginPermission>,
    #[serde(default)]
    allow_community_native: bool,
}

async fn install_plugin(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<InstallPluginRequest>,
) -> ApiResult<serde_json::Value> {
    let request_id = request_id(&headers, request.request_id.as_deref())?;
    let platform = std::env::consts::OS.to_string();
    let architecture = std::env::consts::ARCH.to_string();
    let install_context = InstallContext {
        request_id,
        platform: platform.clone(),
        architecture: architecture.clone(),
        core_version: env!("CARGO_PKG_VERSION").into(),
        public_key: request.public_key.clone(),
        granted_permissions: request.granted_permissions.clone(),
        allow_community_native: request.allow_community_native,
    };
    state
        .lifecycle
        .preflight(&request.manifest, &install_context)
        .map_err(|error| ApiError::conflict("plugin", "preflight", error))?;
    let artifact = request
        .manifest
        .compatible_artifact(&platform, &architecture)
        .ok_or_else(|| {
            ApiError::conflict(
                "plugin",
                "compatibility",
                format!("no artifact for {platform}/{architecture}"),
            )
        })?;
    let location = request
        .artifact_location
        .unwrap_or_else(|| format!("ipfs://{}", artifact.artifact_cid));
    let bytes = download_artifact(&state, &location, artifact.byte_length).await?;
    let outcome = state
        .lifecycle
        .install(request.manifest, &bytes, install_context)
        .map_err(|error| ApiError::bad_request("plugin", "install", error))?;
    if !outcome.idempotent_replay {
        publish_plugin_event(&state, &outcome.record);
    }
    Ok(Json(serde_json::json!({
        "plugin": outcome.record,
        "idempotent_replay": outcome.idempotent_replay,
    })))
}

async fn download_artifact(
    state: &AppState,
    location: &str,
    expected_length: u64,
) -> Result<Vec<u8>, ApiError> {
    const MAX_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024;
    if expected_length > MAX_ARTIFACT_BYTES {
        return Err(ApiError::bad_request(
            "plugin",
            "download",
            "artifact exceeds size limit",
        ));
    }
    if let Some(cid) = location.strip_prefix("ipfs://") {
        if let Ok(bytes) = state.node.cat(cid) {
            return Ok(bytes);
        }
        if let Some(embedded) = state.embedded_node().await {
            if let Ok(bytes) = embedded.get_block(cid).await {
                if bytes.len() as u64 != expected_length {
                    return Err(ApiError::bad_request(
                        "plugin",
                        "download",
                        "P2P artifact byte length does not match its manifest",
                    ));
                }
                return Ok(bytes);
            }
        }
        return state.ipfs.cat(cid).await.map_err(|error| {
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                "fetch_failed",
                "plugin",
                "download",
                error,
                true,
            )
        });
    }
    if !(location.starts_with("https://") || location.starts_with("http://127.0.0.1")) {
        return Err(ApiError::bad_request(
            "plugin",
            "download",
            "only HTTPS or loopback HTTP artifact locations are allowed",
        ));
    }
    let response = reqwest::get(location).await.map_err(|error| {
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            "fetch_failed",
            "plugin",
            "download",
            error,
            true,
        )
    })?;
    if !response.status().is_success() {
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "fetch_failed",
            "plugin",
            "download",
            format!("artifact server returned {}", response.status()),
            true,
        ));
    }
    let bytes = response.bytes().await.map_err(|error| {
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            "fetch_failed",
            "plugin",
            "download",
            error,
            true,
        )
    })?;
    if bytes.len() as u64 > MAX_ARTIFACT_BYTES {
        return Err(ApiError::bad_request(
            "plugin",
            "download",
            "artifact exceeds size limit",
        ));
    }
    Ok(bytes.to_vec())
}

async fn enable_plugin(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<crate::PluginRuntimeRecord> {
    let record = state
        .lifecycle
        .enable(&id)
        .map_err(|error| ApiError::conflict("plugin", "enable", error))?;
    if let Err(error) = state.wasm.activate(&record) {
        let _ = state.lifecycle.disable(&id);
        return Err(ApiError::conflict("plugin", "wasm_activate", error));
    }
    publish_plugin_event(&state, &record);
    Ok(Json(record))
}

async fn disable_plugin(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<crate::PluginRuntimeRecord> {
    state.wasm.deactivate(&id);
    let record = state
        .lifecycle
        .disable(&id)
        .map_err(|error| ApiError::conflict("plugin", "disable", error))?;
    publish_plugin_event(&state, &record);
    Ok(Json(record))
}

async fn rollback_plugin(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<crate::PluginRuntimeRecord> {
    state.wasm.deactivate(&id);
    let record = state
        .lifecycle
        .rollback(&id)
        .map_err(|error| ApiError::conflict("plugin", "rollback", error))?;
    publish_plugin_event(&state, &record);
    Ok(Json(record))
}

async fn uninstall_plugin(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    state.wasm.deactivate(&id);
    state
        .lifecycle
        .uninstall(&id)
        .map_err(|error| ApiError::bad_request("plugin", "uninstall", error))?;
    state.events.publish(Event::PluginChanged {
        plugin_id: id.clone(),
        state: "uninstalled".into(),
        version: None,
    });
    Ok(Json(serde_json::json!({"uninstalled": id})))
}

async fn get_plugin_config(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    let plugin = state
        .lifecycle
        .get(&id)
        .ok_or_else(|| ApiError::not_found("plugin", "config", &id))?;
    Ok(Json(serde_json::json!({
        "schema_cid": plugin.configuration_schema_cid,
        "state_schema_version": plugin.state_schema_version,
        "configuration": plugin.configuration,
    })))
}

async fn set_plugin_config(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(configuration): Json<serde_json::Value>,
) -> ApiResult<crate::PluginRuntimeRecord> {
    let plugin = state
        .lifecycle
        .get(&id)
        .ok_or_else(|| ApiError::not_found("plugin", "configure", &id))?;
    let schema_bytes = state
        .node
        .cat(&plugin.configuration_schema_cid)
        .map_err(|error| ApiError::bad_request("plugin", "load_configuration_schema", error))?;
    let schema: serde_json::Value = serde_json::from_slice(&schema_bytes)
        .map_err(|error| ApiError::bad_request("plugin", "parse_configuration_schema", error))?;
    validate_schema_value(&schema, &configuration, "$")
        .map_err(|error| ApiError::bad_request("plugin", "validate_configuration", error))?;
    let record = state
        .lifecycle
        .configure(&id, configuration)
        .map_err(|error| ApiError::bad_request("plugin", "configure", error))?;
    publish_plugin_event(&state, &record);
    Ok(Json(record))
}

fn validate_schema_value(
    schema: &serde_json::Value,
    value: &serde_json::Value,
    path: &str,
) -> Result<(), String> {
    if let Some(allowed) = schema.get("enum").and_then(serde_json::Value::as_array) {
        if !allowed.contains(value) {
            return Err(format!("{path} is not one of the allowed enum values"));
        }
    }
    if let Some(kind) = schema.get("type").and_then(serde_json::Value::as_str) {
        let valid = match kind {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
            "number" => value.is_number(),
            "boolean" => value.is_boolean(),
            "null" => value.is_null(),
            unsupported => {
                return Err(format!(
                    "{path} uses unsupported schema type `{unsupported}`"
                ))
            }
        };
        if !valid {
            return Err(format!("{path} must be {kind}"));
        }
    }
    if let Some(object) = value.as_object() {
        let properties = schema
            .get("properties")
            .and_then(serde_json::Value::as_object);
        if let Some(required) = schema.get("required").and_then(serde_json::Value::as_array) {
            for field in required.iter().filter_map(serde_json::Value::as_str) {
                if !object.contains_key(field) {
                    return Err(format!("{path}.{field} is required"));
                }
            }
        }
        if schema.get("additionalProperties") == Some(&serde_json::Value::Bool(false)) {
            for field in object.keys() {
                if !properties.is_some_and(|properties| properties.contains_key(field)) {
                    return Err(format!("{path}.{field} is not declared by the schema"));
                }
            }
        }
        if let Some(properties) = properties {
            for (field, child_schema) in properties {
                if let Some(child) = object.get(field) {
                    validate_schema_value(child_schema, child, &format!("{path}.{field}"))?;
                }
            }
        }
    }
    if let Some(array) = value.as_array() {
        if let Some(items) = schema.get("items") {
            for (index, child) in array.iter().enumerate() {
                validate_schema_value(items, child, &format!("{path}[{index}]"))?;
            }
        }
    }
    if let Some(number) = value.as_f64() {
        if schema
            .get("minimum")
            .and_then(serde_json::Value::as_f64)
            .is_some_and(|minimum| number < minimum)
        {
            return Err(format!("{path} is below minimum"));
        }
        if schema
            .get("maximum")
            .and_then(serde_json::Value::as_f64)
            .is_some_and(|maximum| number > maximum)
        {
            return Err(format!("{path} is above maximum"));
        }
    }
    if let Some(text) = value.as_str() {
        let length = text.chars().count() as u64;
        if schema
            .get("minLength")
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|minimum| length < minimum)
        {
            return Err(format!("{path} is shorter than minLength"));
        }
        if schema
            .get("maxLength")
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|maximum| length > maximum)
        {
            return Err(format!("{path} is longer than maxLength"));
        }
    }
    Ok(())
}

async fn revoke_permission(
    State(state): State<Arc<AppState>>,
    Path((id, permission)): Path<(String, String)>,
) -> ApiResult<crate::PluginRuntimeRecord> {
    let permission =
        serde_json::from_value::<PluginPermission>(serde_json::Value::String(permission))
            .map_err(|error| ApiError::bad_request("plugin", "revoke_permission", error))?;
    state.wasm.revoke_permission(&id, permission);
    let record = state
        .lifecycle
        .revoke_permission(&id, permission)
        .map_err(|error| ApiError::bad_request("plugin", "revoke_permission", error))?;
    publish_plugin_event(&state, &record);
    Ok(Json(record))
}

fn publish_plugin_event(state: &AppState, record: &crate::PluginRuntimeRecord) {
    let lifecycle_state = serde_json::to_value(record.lifecycle_state)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "changed".into());
    state.events.publish(Event::PluginChanged {
        plugin_id: record.plugin_id.clone(),
        state: lifecycle_state,
        version: record.active_version.clone(),
    });
}

async fn get_audio_graph(State(state): State<Arc<AppState>>) -> Json<AudioGraphSpecV1> {
    Json(state.audio_graph.active_graph().spec.clone())
}

async fn put_audio_graph(
    State(state): State<Arc<AppState>>,
    Json(spec): Json<AudioGraphSpecV1>,
) -> ApiResult<app_core::audio_graph::AudioPathSnapshot> {
    let candidate = state
        .audio_graph
        .validate_and_compile(spec)
        .map_err(|error| ApiError::bad_request("audio_graph", "compile", error))?;
    state.audio_graph.commit(candidate);
    let snapshot = state.audio_graph.audio_path();
    state.events.publish(Event::AudioGraphChanged {
        graph_id: snapshot.graph_id.clone(),
        generation: snapshot.generation,
    });
    Ok(Json(snapshot))
}

async fn audio_path(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "path": state.audio_graph.audio_path(),
        "bit_perfect": state.audio_graph.bit_perfect_status(None),
    }))
}

async fn audio_stats(
    State(state): State<Arc<AppState>>,
) -> Json<app_core::audio_graph::AudioGraphStatsSnapshot> {
    Json(state.audio_graph.stats())
}

async fn library_tracks(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SearchQuery>,
) -> Json<Vec<app_core::library_service::LibraryTrackV1>> {
    Json(state.library.search(&query.q))
}

#[derive(Debug, Deserialize)]
struct ImportLocalTrackRequest {
    #[serde(default)]
    request_id: Option<String>,
    track: app_core::Track,
}

async fn import_local_track(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<ImportLocalTrackRequest>,
) -> ApiResult<app_core::library_service::LibraryTrackV1> {
    let request_id = request_id(&headers, request.request_id.as_deref())?;
    let fingerprint =
        crate::state::sha256_hex(&serde_json::to_vec(&request.track).unwrap_or_default());
    let (track, _) = state
        .idempotency
        .execute("library.import_local", &request_id, &fingerprint, || {
            state
                .library
                .import_local(request.track, now())
                .map_err(|error| error.to_string())
        })
        .map_err(|error| map_idempotency("library", "import_local", error))?;
    Ok(Json(track))
}

#[derive(Debug, Deserialize)]
struct FavoriteRequest {
    #[serde(default)]
    request_id: Option<String>,
    favorite: bool,
}

async fn set_favorite(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<FavoriteRequest>,
) -> ApiResult<app_core::library_service::LibraryTrackV1> {
    let request_id = request_id(&headers, request.request_id.as_deref())?;
    let fingerprint =
        crate::state::sha256_hex(&serde_json::to_vec(&(&id, request.favorite)).unwrap_or_default());
    let (track, replayed) = state
        .idempotency
        .execute("library.favorite", &request_id, &fingerprint, || {
            state
                .library
                .set_favorite(&id, request.favorite)
                .map_err(|error| error.to_string())?;
            state
                .library
                .track(&id)
                .ok_or_else(|| format!("track `{id}` does not exist"))
        })
        .map_err(|error| map_idempotency("library", "favorite", error))?;
    if request.favorite && !replayed && state.node.config().assist_pin_favorites {
        // 收藏协助 Pin（DST-009）：本地已有对象直接 Pin，否则建立幂等
        // Pin 传输任务（服从网络类别策略），并推送给第三方 Pin 服务。
        for source in &track.sources {
            let Some(cid) = source.content_cid.clone() else {
                continue;
            };
            if state.node.pin(&cid).is_err() {
                let Ok(task) = state.transfers.create_with_priority(
                    &format!("assist-pin-{id}-{cid}"),
                    TransferKind::Pin,
                    cid.clone(),
                    None,
                    NetworkPolicyV1 {
                        wifi_only: false,
                        cellular_limit_bytes: None,
                        max_concurrency: 2,
                    },
                    0,
                ) else {
                    continue;
                };
                if task.state == jimmusic_protocol::TransferState::Queued {
                    crate::transfer_runner::spawn(state.clone(), task.task_id);
                }
            }
            spawn_remote_pins(&state, cid);
        }
    }
    Ok(Json(track))
}

async fn import_manifest(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(cid): Path<String>,
    Json(manifest): Json<MusicManifestV1>,
) -> ApiResult<app_core::library_service::LibraryTrackV1> {
    let request_id = request_id(&headers, None)?;
    let verified_cid = state
        .publications
        .verify_manifest(&manifest)
        .map_err(|error| ApiError::bad_request("library", "verify_manifest", error))?;
    if verified_cid != cid {
        return Err(ApiError::conflict(
            "library",
            "import_manifest",
            format!("path CID `{cid}` does not match canonical manifest CID `{verified_cid}`"),
        ));
    }
    let fingerprint =
        crate::state::sha256_hex(&serde_json::to_vec(&(&cid, &manifest)).unwrap_or_default());
    let (track, _) = state
        .idempotency
        .execute("library.import_manifest", &request_id, &fingerprint, || {
            state
                .library
                .import_manifest(cid, &manifest, now())
                .map_err(|error| error.to_string())
        })
        .map_err(|error| map_idempotency("library", "import_manifest", error))?;
    Ok(Json(track))
}

#[derive(Debug, Deserialize)]
struct ScanLibraryRequest {
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    directory: Option<String>,
    #[serde(default)]
    set_as_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LibraryScanResponse {
    directory: String,
    discovered: usize,
    imported: Vec<app_core::library_service::LibraryTrackV1>,
    missing_sources: usize,
}

async fn scan_library(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<ScanLibraryRequest>,
) -> ApiResult<LibraryScanResponse> {
    let request_id = request_id(&headers, request.request_id.as_deref())?;
    let directory = request
        .directory
        .filter(|directory| !directory.trim().is_empty())
        .or_else(|| state.library.music_directory())
        .ok_or_else(|| {
            ApiError::bad_request(
                "library",
                "scan",
                "directory is required until a music directory is configured",
            )
        })?;
    let directory = FilePath::from(directory);
    if !directory.is_absolute() {
        return Err(ApiError::bad_request(
            "library",
            "scan",
            "music directory must be an absolute path",
        ));
    }
    let fingerprint = crate::state::sha256_hex(
        &serde_json::to_vec(&(directory.to_string_lossy().as_ref(), request.set_as_default))
            .unwrap_or_default(),
    );
    let scan_state = state.clone();
    let response = tokio::task::spawn_blocking(move || {
        scan_state
            .idempotency
            .execute("library.scan", &request_id, &fingerprint, || {
                if request.set_as_default {
                    scan_state
                        .library
                        .set_music_directory(&directory)
                        .map_err(|error| error.to_string())?;
                }
                let (discovered, truncated) =
                    app_core::MediaLibrary::new().scan_bounded(&directory, 100_000);
                if truncated {
                    return Err("scan exceeds the 100,000 track safety limit".into());
                }
                let mut imported = Vec::with_capacity(discovered.len());
                let timestamp = now();
                for track in discovered {
                    imported.push(
                        scan_state
                            .library
                            .import_local(track, timestamp)
                            .map_err(|error| error.to_string())?,
                    );
                }
                let missing_sources = scan_state
                    .library
                    .refresh_availability(timestamp)
                    .map_err(|error| error.to_string())?;
                Ok(LibraryScanResponse {
                    directory: directory.to_string_lossy().into_owned(),
                    discovered: imported.len(),
                    imported,
                    missing_sources,
                })
            })
    })
    .await
    .map_err(|error| ApiError::bad_request("library", "scan_worker", error))?
    .map_err(|error| map_idempotency("library", "scan", error))?
    .0;
    Ok(Json(response))
}

async fn refresh_availability(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<serde_json::Value> {
    let request_id = request_id(&headers, None)?;
    let fingerprint = crate::state::sha256_hex(b"refresh-availability-v1");
    let (response, _) = state
        .idempotency
        .execute(
            "library.refresh_availability",
            &request_id,
            &fingerprint,
            || {
                let missing = state
                    .library
                    .refresh_availability(now())
                    .map_err(|error| error.to_string())?;
                Ok(serde_json::json!({"missing_sources": missing}))
            },
        )
        .map_err(|error| map_idempotency("library", "refresh_availability", error))?;
    Ok(Json(response))
}

async fn get_music_directory(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "directory": state.library.music_directory(),
    }))
}

#[derive(Debug, Deserialize)]
struct MusicDirectoryRequest {
    #[serde(default)]
    request_id: Option<String>,
    directory: String,
}

async fn set_music_directory(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<MusicDirectoryRequest>,
) -> ApiResult<serde_json::Value> {
    let request_id = request_id(&headers, request.request_id.as_deref())?;
    let directory = FilePath::from(request.directory.trim());
    if !directory.is_absolute() {
        return Err(ApiError::bad_request(
            "library",
            "set_music_directory",
            "music directory must be an absolute path",
        ));
    }
    let normalized = directory.to_string_lossy().into_owned();
    let fingerprint = crate::state::sha256_hex(normalized.as_bytes());
    let (response, _) = state
        .idempotency
        .execute(
            "library.set_music_directory",
            &request_id,
            &fingerprint,
            || {
                state
                    .library
                    .set_music_directory(&directory)
                    .map_err(|error| error.to_string())?;
                Ok(serde_json::json!({"directory": normalized}))
            },
        )
        .map_err(|error| map_idempotency("library", "set_music_directory", error))?;
    Ok(Json(response))
}

async fn list_playlists(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<app_core::library_service::PlaylistV1>> {
    Json(state.library.playlists())
}

#[derive(Debug, Deserialize)]
struct CreatePlaylistRequest {
    #[serde(default)]
    request_id: Option<String>,
    name: String,
}

async fn create_playlist(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<CreatePlaylistRequest>,
) -> ApiResult<app_core::library_service::PlaylistV1> {
    let request_id = request_id(&headers, request.request_id.as_deref())?;
    let name = request.name.trim().to_string();
    let fingerprint = crate::state::sha256_hex(name.as_bytes());
    let (playlist, _) = state
        .idempotency
        .execute("library.create_playlist", &request_id, &fingerprint, || {
            state
                .library
                .create_playlist(&name, now())
                .map_err(|error| error.to_string())
        })
        .map_err(|error| map_idempotency("library", "create_playlist", error))?;
    Ok(Json(playlist))
}

#[derive(Debug, Deserialize)]
struct UpdatePlaylistRequest {
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    track_ids: Option<Vec<String>>,
}

async fn update_playlist(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<UpdatePlaylistRequest>,
) -> ApiResult<app_core::library_service::PlaylistV1> {
    let request_id = request_id(&headers, request.request_id.as_deref())?;
    let fingerprint = crate::state::sha256_hex(
        &serde_json::to_vec(&(&id, &request.name, &request.track_ids)).unwrap_or_default(),
    );
    let (playlist, _) = state
        .idempotency
        .execute("library.update_playlist", &request_id, &fingerprint, || {
            state
                .library
                .update_playlist(&id, request.name, request.track_ids, now())
                .map_err(|error| error.to_string())
        })
        .map_err(|error| map_idempotency("library", "update_playlist", error))?;
    Ok(Json(playlist))
}

async fn remove_playlist(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    let request_id = request_id(&headers, None)?;
    let fingerprint = crate::state::sha256_hex(id.as_bytes());
    let (response, _) = state
        .idempotency
        .execute("library.remove_playlist", &request_id, &fingerprint, || {
            state
                .library
                .remove_playlist(&id)
                .map_err(|error| error.to_string())?;
            Ok(serde_json::json!({"removed": id}))
        })
        .map_err(|error| map_idempotency("library", "remove_playlist", error))?;
    Ok(Json(response))
}

async fn get_session(State(state): State<Arc<AppState>>) -> Json<PlaybackSessionV1> {
    Json(state.library.session())
}

async fn save_session(
    State(state): State<Arc<AppState>>,
    Json(session): Json<PlaybackSessionV1>,
) -> ApiResult<PlaybackSessionV1> {
    state
        .library
        .save_session(session)
        .map_err(|error| ApiError::bad_request("library", "save_session", error))?;
    Ok(Json(state.library.session()))
}

fn request_id(headers: &HeaderMap, body: Option<&str>) -> Result<String, ApiError> {
    body.map(str::to_owned)
        .or_else(|| {
            headers
                .get("idempotency-key")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        })
        .filter(|value| !value.trim().is_empty() && value.len() <= 200)
        .ok_or_else(|| {
            ApiError::bad_request(
                "api",
                "idempotency",
                "request_id or Idempotency-Key is required",
            )
        })
}

fn map_idempotency(
    subsystem: &str,
    operation: &str,
    error: crate::idempotency::IdempotencyError,
) -> ApiError {
    if matches!(error, crate::idempotency::IdempotencyError::Conflict) {
        ApiError::conflict(subsystem, operation, error)
    } else {
        ApiError::bad_request(subsystem, operation, error)
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(i64::MAX as u64) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use ed25519_dalek::{Signer, SigningKey};
    use http_body_util::BodyExt;
    use std::collections::BTreeMap;
    use tower::ServiceExt;

    fn state() -> (Arc<AppState>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(
            dir.path().to_string_lossy().into_owned(),
            "http://127.0.0.1:5001".into(),
        )
        .unwrap();
        (Arc::new(state), dir)
    }

    async fn call(
        app: Router<Arc<AppState>>,
        state: Arc<AppState>,
        method: &str,
        uri: &str,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let response = app
            .with_state(state)
            .oneshot(
                axum::http::Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn v1_health_and_node_status_are_versioned() {
        let (state, _dir) = state();
        let (status, health) = call(
            routes(),
            state.clone(),
            "GET",
            "/health",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(health["schema_version"], 1);
        let (status, node) = call(
            routes(),
            state,
            "GET",
            "/node/status",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(node["lifecycle_state"], "running");
    }

    #[tokio::test]
    async fn diagnostics_are_versioned_and_redact_local_media_paths() {
        let (state, _dir) = state();
        let secret_path = "/private/music/diagnostic-secret.wav";
        state
            .library
            .import_local(
                app_core::Track {
                    path: secret_path.into(),
                    title: "Secret title".into(),
                    artist: None,
                    album: None,
                    duration: None,
                    sample_rate: None,
                    channels: None,
                },
                1,
            )
            .unwrap();

        let (status, report) = call(
            routes(),
            state,
            "GET",
            "/diagnostics",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(report["schema_version"], 1);
        assert_eq!(report["safe_to_share"], true);
        assert_eq!(report["library"]["track_count"], 1);
        assert!(!report.to_string().contains(secret_path));
    }

    #[tokio::test]
    async fn transfer_requires_idempotency_and_replays_same_task() {
        let (state, _dir) = state();
        let body = serde_json::json!({
            "request_id": "req-1",
            "kind": "download",
            "target_cid": "bafytarget",
            "network_policy": {"wifi_only": false, "max_concurrency": 2}
        });
        let (status, first) =
            call(routes(), state.clone(), "POST", "/transfers", body.clone()).await;
        assert_eq!(status, StatusCode::OK);
        let (_, second) = call(routes(), state, "POST", "/transfers", body).await;
        assert_eq!(first["task_id"], second["task_id"]);
    }

    fn streaming_task(
        state: &Arc<AppState>,
        request_id: &str,
        target: &str,
    ) -> jimmusic_protocol::TransferTaskV1 {
        let task = state
            .transfers
            .create(
                request_id,
                TransferKind::Download,
                target.into(),
                None,
                NetworkPolicyV1 {
                    wifi_only: false,
                    cellular_limit_bytes: None,
                    max_concurrency: 2,
                },
            )
            .unwrap();
        state
            .transfers
            .record_progress(&task.task_id, 0, None, 0, vec!["test".into()])
            .unwrap();
        task
    }

    #[tokio::test]
    async fn transfer_stream_follows_growth_and_ends_on_terminal_state() {
        let (state, _dir) = state();
        let task = streaming_task(&state, "stream-req-1", "bafy-stream-target");
        let parts = state.repo_dir.join("transfer-parts");
        std::fs::create_dir_all(&parts).unwrap();
        let part = parts.join(format!("{}.part", task.task_id));
        std::fs::write(&part, b"0123456789").unwrap();

        let response = routes()
            .with_state(state.clone())
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri(format!("/transfers/{}/stream", task.task_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["accept-ranges"], "bytes");
        assert_eq!(
            response.headers()["x-jimmusic-stream-source"],
            "transfer-part"
        );
        let mut body = response.into_body().into_data_stream();

        let first = tokio::time::timeout(Duration::from_secs(3), body.next())
            .await
            .expect("first chunk")
            .expect("stream open")
            .expect("chunk ok");
        assert_eq!(&first[..], b"0123456789");

        // 文件增长：流应跟随输出新增字节。
        std::fs::write(&part, b"0123456789abcdefghij").unwrap();
        let second = tokio::time::timeout(Duration::from_secs(3), body.next())
            .await
            .expect("second chunk")
            .expect("stream open")
            .expect("chunk ok");
        assert_eq!(&second[..], b"abcdefghij");

        // 任务终结（失败）：已落盘字节服务完后流结束。
        state
            .transfers
            .fail(
                &task.task_id,
                TransferState::Failed,
                ErrorEnvelopeV1 {
                    schema_version: SCHEMA_V1,
                    code: "test_failure".into(),
                    message: "injected".into(),
                    subsystem: "transfer".into(),
                    operation: "test".into(),
                    retryable: false,
                    unsupported_reason: None,
                    details: BTreeMap::new(),
                    request_id: None,
                    causes: Vec::new(),
                },
            )
            .unwrap();
        let end = tokio::time::timeout(Duration::from_secs(3), body.next())
            .await
            .expect("stream should end");
        assert!(end.is_none(), "terminal state should close the stream");
    }

    #[tokio::test]
    async fn transfer_stream_honors_single_byte_range() {
        use http_body_util::BodyExt;

        let (state, _dir) = state();
        let task = streaming_task(&state, "stream-req-2", "bafy-stream-range");
        let parts = state.repo_dir.join("transfer-parts");
        std::fs::create_dir_all(&parts).unwrap();
        std::fs::write(
            parts.join(format!("{}.part", task.task_id)),
            b"0123456789abcdef",
        )
        .unwrap();

        let response = routes()
            .with_state(state.clone())
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri(format!("/transfers/{}/stream", task.task_id))
                    .header("range", "bytes=2-5")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.headers()["content-range"], "bytes 2-5/*");
        assert_eq!(response.headers()["content-length"], "4");
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&bytes[..], b"2345");
    }

    #[tokio::test]
    async fn transfer_stream_rejects_terminally_failed_tasks() {
        let (state, _dir) = state();
        let task = streaming_task(&state, "stream-req-3", "bafy-stream-dead");
        state
            .transfers
            .fail(
                &task.task_id,
                TransferState::Failed,
                ErrorEnvelopeV1 {
                    schema_version: SCHEMA_V1,
                    code: "test_failure".into(),
                    message: "injected".into(),
                    subsystem: "transfer".into(),
                    operation: "test".into(),
                    retryable: false,
                    unsupported_reason: None,
                    details: BTreeMap::new(),
                    request_id: None,
                    causes: Vec::new(),
                },
            )
            .unwrap();
        let (status, body) = call(
            routes(),
            state,
            "GET",
            &format!("/transfers/{}/stream", task.task_id),
            serde_json::json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["subsystem"], "transfer");
        assert!(body["message"].as_str().unwrap().contains("failed"));
    }

    #[tokio::test]
    async fn transfer_streams_from_gateway_and_commits_verified_cid() {
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let bytes = b"streamed object".to_vec();
        let cid = jimmusic_protocol::cid_v1_for_bytes(app_core::node_service::RAW_CODEC, &bytes);
        Mock::given(method("POST"))
            .and(path("/api/v0/cat"))
            .and(query_param("arg", cid.clone()))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes.clone()))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let state = Arc::new(
            AppState::new(dir.path().to_string_lossy().into_owned(), server.uri()).unwrap(),
        );
        let body = serde_json::json!({
            "request_id": "stream-1",
            "kind": "download",
            "target_cid": cid.clone(),
            "network_policy": {"wifi_only": false, "max_concurrency": 2}
        });
        let (status, task) = call(routes(), state.clone(), "POST", "/transfers", body).await;
        assert_eq!(status, StatusCode::OK);
        let task_id = task["task_id"].as_str().unwrap();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let current = state.transfers.get(task_id).unwrap();
            if current.state == jimmusic_protocol::TransferState::Completed {
                break;
            }
            assert_ne!(
                current.state,
                jimmusic_protocol::TransferState::IntegrityFailed
            );
            assert!(tokio::time::Instant::now() < deadline, "transfer timed out");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(state.node.cat(&cid).unwrap(), bytes);
    }

    #[tokio::test]
    async fn identity_generation_rejects_weak_export_passphrase() {
        let (state, _dir) = state();
        let (status, error) = call(
            routes(),
            state,
            "POST",
            "/identities/generate",
            serde_json::json!({"display_name": "Artist", "passphrase": "short"}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(error["subsystem"], "identity");
    }

    #[tokio::test]
    async fn encrypted_identity_signs_pins_and_idempotently_replays_publication() {
        let (state, _dir) = state();
        let passphrase = "correct horse battery";
        let (status, identity) = call(
            routes(),
            state.clone(),
            "POST",
            "/identities/generate",
            serde_json::json!({"display_name": "Artist", "passphrase": passphrase}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            identity["encrypted_bundle"]["identity"]["publisher_id"],
            identity["identity"]["publisher_id"]
        );
        let request = serde_json::json!({
            "request_id": "signed-publication-1",
            "display_name": "Artist",
            "passphrase": passphrase,
            "bundle": identity["encrypted_bundle"].clone(),
            "operation": "publish",
            "manifest": {
                "schema_version": 1,
                "work_id": "work-1",
                "release_id": "release-1",
                "title": "A real track",
                "artists": ["Artist"],
                "album": "Album",
                "duration_ms": 1000,
                "language": "en",
                "license": {
                    "identifier": "CC-BY-4.0",
                    "allows_redistribution": true
                },
                "content_labels": ["clean"],
                "renditions": [{
                    "rendition_id": "original",
                    "content_cid": "bafycontent",
                    "container": "flac",
                    "codec": "flac",
                    "sample_rate": 44100,
                    "bit_depth": 24,
                    "channels": 2,
                    "channel_layout": "stereo",
                    "duration_ms": 1000,
                    "byte_length": 10,
                    "lossless": true,
                    "original": true,
                    "streamable": true
                }],
                "publisher_identity_cid": "filled-by-signer",
                "created_at": 1,
                "updated_at": 1
            }
        });
        let (status, first) = call(
            routes(),
            state.clone(),
            "POST",
            "/publications/sign",
            request.clone(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{first}");
        assert_eq!(first["receipt"]["pinned"], true);
        assert_eq!(first["receipt"]["sequence"], 0);
        assert!(first["signed_manifest"]["publisher_signature"]
            .as_str()
            .is_some_and(|value| !value.is_empty()));
        assert!(first["signed_event"]["signature"]
            .as_str()
            .is_some_and(|value| !value.is_empty()));
        let (status, replay) = call(
            routes(),
            state.clone(),
            "POST",
            "/publications/sign",
            request,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{replay}");
        assert_eq!(first, replay);
        let publisher_id = first["receipt"]["publisher_id"].as_str().unwrap();
        assert_eq!(state.publications.feed(publisher_id).len(), 1);
    }

    #[tokio::test]
    async fn playlist_creation_is_persistent_and_idempotent() {
        let (state, _dir) = state();
        let body = serde_json::json!({"request_id": "playlist-1", "name": "Offline"});
        let (status, first) = call(
            routes(),
            state.clone(),
            "POST",
            "/library/playlists",
            body.clone(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{first}");
        let (_, replay) = call(routes(), state.clone(), "POST", "/library/playlists", body).await;
        assert_eq!(first, replay);
        let (status, playlists) = call(
            routes(),
            state,
            "GET",
            "/library/playlists",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(playlists.as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn invalid_audio_graph_does_not_replace_active_snapshot() {
        let (state, _dir) = state();
        let before = state.audio_graph.active_graph().generation;
        let mut invalid = state.audio_graph.active_graph().spec.clone();
        invalid.edges.push(jimmusic_protocol::AudioEdgeSpecV1 {
            from_node: "output".into(),
            from_port: "missing".into(),
            to_node: "decoder".into(),
            to_port: "missing".into(),
        });
        let (status, _) = call(
            routes(),
            state.clone(),
            "PUT",
            "/audio/graph",
            serde_json::to_value(invalid).unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(state.audio_graph.active_graph().generation, before);
    }

    #[tokio::test]
    async fn encrypted_moderation_report_is_queued_then_delivered_without_plaintext() {
        use wiremock::matchers::{body_partial_json, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/reports"))
            .and(body_partial_json(serde_json::json!({
                "report_id": "report-1"
            })))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        let (state, _dir) = state();
        let maintainer = SigningKey::from_bytes(&[21; 32]);
        let mut source = CommunitySourceManifestV1 {
            schema_version: SCHEMA_V1,
            source_id: "community.example".into(),
            name: "Example".into(),
            description: "test source".into(),
            languages: vec!["en".into()],
            maintainer_identity_cid: "bafy-maintainer".into(),
            catalog_head: None,
            policy_head: None,
            supported_schemas: vec![SCHEMA_V1],
            report_endpoint: Some(format!("{}/reports", server.uri())),
            report_encryption_public_key: Some("2a".repeat(32)),
            updated_at: 1,
            signature: None,
        };
        source.signature = Some(hex::encode(
            maintainer
                .sign(&source.unsigned_bytes().unwrap())
                .to_bytes(),
        ));
        state
            .community
            .add_source(
                source,
                hex::encode(maintainer.verifying_key().to_bytes()),
                &state.node,
                0,
            )
            .unwrap();

        let reporter = SigningKey::from_bytes(&[22; 32]);
        let mut report = ModerationReportV1 {
            schema_version: SCHEMA_V1,
            report_id: "report-1".into(),
            target: "bafy-target".into(),
            reason_code: "safety".into(),
            description: "private detail must not leave in plaintext".into(),
            evidence_cids: vec!["bafy-evidence".into()],
            reporter_identity: None,
            reporter_public_key: hex::encode(reporter.verifying_key().to_bytes()),
            anonymous: true,
            recipient_source_id: "community.example".into(),
            created_at: 2,
            signature: None,
            encrypted_envelope: None,
        };
        report.signature = Some(hex::encode(
            reporter.sign(&report.unsigned_bytes().unwrap()).to_bytes(),
        ));
        let (status, delivered) = call(
            routes(),
            state,
            "POST",
            "/moderation-reports",
            serde_json::json!({
                "report": report,
                "encrypt_for_recipient": true,
                "submit_now": true
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{delivered}");
        assert_eq!(
            delivered["status"],
            serde_json::Value::String("submitted".into())
        );
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let body = String::from_utf8_lossy(&requests[0].body);
        assert!(body.contains("encrypted_envelope"));
        assert!(!body.contains("private detail"));
        assert!(!body.contains("bafy-evidence"));
    }

    fn node_config_body(network_class: Option<&str>) -> serde_json::Value {
        serde_json::json!({
            "storage_limit_bytes": 20 * 1024 * 1024 * 1024u64,
            "cache_limit_bytes": 2 * 1024 * 1024 * 1024u64,
            "max_concurrent_transfers": 3,
            "upload_limit_bytes_per_second": null,
            "download_limit_bytes_per_second": null,
            "metered_network_allowed": false,
            "network_class": network_class,
        })
    }

    #[tokio::test]
    async fn network_class_pauses_wifi_only_transfers_and_restores_them() {
        let (state, _dir) = state();
        // 先声明蜂窝网络：wifi_only 任务创建后应处于网络策略暂停，不进入队列执行。
        let (status, config) = call(
            routes(),
            state.clone(),
            "PUT",
            "/node/config",
            node_config_body(Some("cellular")),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{config}");
        assert_eq!(config["network_class"], "cellular");

        let (status, task) = call(
            routes(),
            state.clone(),
            "POST",
            "/transfers",
            serde_json::json!({
                "request_id": "req-wifi-only",
                "kind": "fetch",
                "target_cid": "bafy-wifi-target",
                "network_policy": {"wifi_only": true, "max_concurrency": 2}
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{task}");
        assert_eq!(task["state"], "paused");
        assert_eq!(task["paused_by_network"], true);
        assert_eq!(task["error"]["code"], "paused_wifi_only");
        assert_eq!(task["error"]["subsystem"], "transfer");

        // 回到 Wi-Fi：任务自动恢复排队（不再是网络暂停）。
        let (status, _) = call(
            routes(),
            state.clone(),
            "PUT",
            "/node/config",
            node_config_body(Some("wifi")),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, tasks) = call(
            routes(),
            state.clone(),
            "GET",
            "/transfers",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let resumed = &tasks.as_array().unwrap()[0];
        assert_eq!(resumed["paused_by_network"], false);
        assert_ne!(resumed["state"], "paused");
    }

    #[tokio::test]
    async fn upload_rate_limit_is_explicitly_rejected_as_unsupported() {
        let (state, _dir) = state();
        let mut body = node_config_body(None);
        body["upload_limit_bytes_per_second"] = serde_json::json!(1024);
        let (status, response) = call(routes(), state, "PUT", "/node/config", body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(response["code"], "unsupported");
        assert!(response["unsupported_reason"]
            .as_str()
            .unwrap()
            .contains("Bitswap"));
    }

    #[tokio::test]
    async fn policy_revoke_auto_disables_installed_plugin() {
        use app_core::node_service::RAW_CODEC;
        use jimmusic_protocol::{
            cid_v1_for, cid_v1_for_bytes, PluginArtifactV1, PluginRuntime, PolicyAction,
        };
        use std::collections::BTreeSet;

        let (state, _dir) = state();
        // 安装官方签名插件。
        let artifact = b"plugin-artifact-bytes".to_vec();
        let publisher = SigningKey::from_bytes(&[24; 32]);
        let mut manifest = PluginManifestV1 {
            schema_version: SCHEMA_V1,
            plugin_id: "org.example.revoketest".into(),
            name: "RevokeTest".into(),
            version: "1.0.0".into(),
            publisher: "org.example".into(),
            plugin_kind: "audio_output".into(),
            interface_versions: BTreeMap::from([("audio_output".into(), "2".into())]),
            minimum_core_version: "2.0.0".into(),
            maximum_core_version: "2.9.9".into(),
            artifacts: vec![PluginArtifactV1 {
                artifact_cid: cid_v1_for_bytes(RAW_CODEC, &artifact),
                platform: "linux".into(),
                architecture: "x86_64".into(),
                runtime: PluginRuntime::Native,
                entrypoint: "librevoke.so".into(),
                byte_length: artifact.len() as u64,
                sha256: crate::state::sha256_hex(&artifact),
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
            publisher
                .sign(&manifest.unsigned_bytes().unwrap())
                .to_bytes(),
        ));
        let manifest_cid = cid_v1_for(&manifest).unwrap();
        let outcome = state
            .lifecycle
            .install(
                manifest,
                &artifact,
                InstallContext {
                    request_id: "revoke-test-install".into(),
                    platform: "linux".into(),
                    architecture: "x86_64".into(),
                    core_version: "2.0.0".into(),
                    public_key: hex::encode(publisher.verifying_key().to_bytes()),
                    granted_permissions: BTreeSet::from([PluginPermission::AudioDevice]),
                    allow_community_native: true,
                },
            )
            .unwrap();
        assert!(!outcome.idempotent_replay);
        assert_eq!(
            state
                .lifecycle
                .get("org.example.revoketest")
                .unwrap()
                .lifecycle_state,
            jimmusic_protocol::PluginLifecycleState::Installed
        );

        // 社区源发布 Revoke 策略事件。
        let maintainer = SigningKey::from_bytes(&[25; 32]);
        let mut source = CommunitySourceManifestV1 {
            schema_version: SCHEMA_V1,
            source_id: "policy.example".into(),
            name: "Policy".into(),
            description: "revocation test source".into(),
            languages: vec!["en".into()],
            maintainer_identity_cid: "bafy-maintainer".into(),
            catalog_head: None,
            policy_head: None,
            supported_schemas: vec![SCHEMA_V1],
            report_endpoint: None,
            report_encryption_public_key: None,
            updated_at: 1,
            signature: None,
        };
        source.signature = Some(hex::encode(
            maintainer
                .sign(&source.unsigned_bytes().unwrap())
                .to_bytes(),
        ));
        state
            .community
            .add_source(
                source,
                hex::encode(maintainer.verifying_key().to_bytes()),
                &state.node,
                0,
            )
            .unwrap();

        let mut revoke = jimmusic_protocol::PolicyEventV1 {
            schema_version: SCHEMA_V1,
            action: PolicyAction::Revoke,
            target_type: "cid".into(),
            target: manifest_cid.clone(),
            reason_code: "security".into(),
            description: "compromised release".into(),
            evidence_cids: Vec::new(),
            scope: Vec::new(),
            issued_at: 2,
            expires_at: None,
            sequence: 0,
            previous_event_cid: None,
            signature: None,
        };
        revoke.signature = Some(hex::encode(
            maintainer
                .sign(&revoke.unsigned_bytes().unwrap())
                .to_bytes(),
        ));
        let (status, body) = call(
            routes(),
            state.clone(),
            "POST",
            "/community-sources/policy.example/policy-events",
            serde_json::to_value(revoke).unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");

        // PLG-009：撤销事件摄入后已安装插件被自动停用并进入 Revoked 状态。
        let record = state.lifecycle.get("org.example.revoketest").unwrap();
        assert_eq!(
            record.lifecycle_state,
            jimmusic_protocol::PluginLifecycleState::Revoked
        );
        assert!(record
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("revoked")));
    }

    /// 注册身份并导入一个 rendition 内容 CID 为 [content_cid] 的签名 Manifest。
    async fn import_assist_track(
        state: &Arc<AppState>,
        key: &SigningKey,
        content_cid: &str,
    ) -> String {
        use jimmusic_protocol::LicenseDeclaration;
        let public_key = hex::encode(key.verifying_key().to_bytes());
        let identity = PublisherIdentityV1 {
            schema_version: SCHEMA_V1,
            publisher_id: app_core::publication_service::publisher_id_from_public_key(&public_key),
            public_key,
            display_name: "AssistPin Publisher".into(),
            created_at: 1,
            previous_key: None,
            rotation_proof: None,
            revoked_at: None,
            revocation_proof: None,
        };
        let identity_cid = state
            .publications
            .register_identity(identity, &state.node)
            .unwrap();
        let mut manifest = MusicManifestV1 {
            schema_version: SCHEMA_V1,
            work_id: "assist-work".into(),
            release_id: "assist-release".into(),
            title: "Assist".into(),
            artists: vec!["Artist".into()],
            album: "Album".into(),
            track_number: Some(1),
            disc_number: Some(1),
            duration_ms: 1_000,
            language: "en".into(),
            genres: Vec::new(),
            tags: Vec::new(),
            cover_cid: None,
            lyrics_cid: None,
            credits: BTreeMap::new(),
            license: LicenseDeclaration {
                identifier: "CC-BY-4.0".into(),
                rights_statement: None,
                allows_redistribution: true,
            },
            content_labels: vec!["clean".into()],
            renditions: vec![jimmusic_protocol::MusicRenditionV1 {
                rendition_id: "original".into(),
                content_cid: content_cid.into(),
                container: "flac".into(),
                codec: "flac".into(),
                profile: String::new(),
                sample_rate: 44_100,
                bit_depth: 24,
                channels: 2,
                channel_layout: "stereo".into(),
                duration_ms: 1_000,
                byte_length: 10,
                lossless: true,
                original: true,
                streamable: true,
            }],
            publisher_identity_cid: identity_cid,
            created_at: 2,
            updated_at: 2,
            publisher_signature: None,
        };
        manifest.publisher_signature = Some(hex::encode(
            key.sign(&manifest.unsigned_bytes().unwrap()).to_bytes(),
        ));
        let manifest_cid = cid_v1_for(&manifest).unwrap();
        // import_manifest 只从 Idempotency-Key 头取 request_id。
        let response = routes()
            .with_state(state.clone())
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(format!("/library/manifests/{manifest_cid}"))
                    .header("content-type", "application/json")
                    .header("idempotency-key", "assist-import")
                    .body(Body::from(serde_json::to_vec(&manifest).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let track: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(status, StatusCode::OK, "{track}");
        track["track_id"].as_str().unwrap().to_string()
    }

    #[tokio::test]
    async fn favorite_assist_pin_creates_local_pin_and_remote_push() {
        use jimmusic_protocol::cid_v1_for_bytes;
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let content_cid =
            cid_v1_for_bytes(app_core::node_service::RAW_CODEC, b"assist-pin-content");
        let pin_service = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v0/pin/add"))
            .and(query_param("arg", content_cid.as_str()))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"Pins": [content_cid]})),
            )
            .expect(1)
            .mount(&pin_service)
            .await;

        let (state, _dir) = state();
        // 本地 CAS 已有内容对象（本地 Pin 路径）。
        state
            .node
            .put_verified(
                &content_cid,
                app_core::node_service::RAW_CODEC,
                b"assist-pin-content",
                false,
                false,
            )
            .unwrap();
        let mut config = node_config_body(None);
        config["assist_pin_favorites"] = serde_json::json!(true);
        config["pin_services"] = serde_json::json!([pin_service.uri()]);
        let (status, _) = call(routes(), state.clone(), "PUT", "/node/config", config).await;
        assert_eq!(status, StatusCode::OK);

        let key = SigningKey::from_bytes(&[26; 32]);
        let track_id = import_assist_track(&state, &key, &content_cid).await;

        let (status, _) = call(
            routes(),
            state.clone(),
            "PUT",
            &format!("/library/tracks/{track_id}/favorite"),
            serde_json::json!({"favorite": true, "request_id": "assist-fav"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, pins) = call(
            routes(),
            state.clone(),
            "GET",
            "/pins",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            pins.as_array().unwrap().iter().any(|entry| {
                entry["cid"] == content_cid && entry["health"]["local_pin"] == true
            }),
            "{pins}"
        );

        // 第三方 pin/add 收到推送。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let requests = loop {
            if let Some(requests) = pin_service.received_requests().await {
                if !requests.is_empty() {
                    break requests;
                }
            }
            if std::time::Instant::now() >= deadline {
                panic!("remote pin service did not receive pin/add");
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        };
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].url.path(), "/api/v0/pin/add");
        assert_eq!(requests[0].url.query_pairs().count(), 1);
        assert_eq!(
            requests[0].url.query_pairs().next(),
            Some((
                std::borrow::Cow::Borrowed("arg"),
                content_cid.clone().into()
            ))
        );
    }

    #[tokio::test]
    async fn favorite_without_assist_pin_does_not_pin() {
        use jimmusic_protocol::cid_v1_for_bytes;
        let (state, _dir) = state();
        let content_cid = cid_v1_for_bytes(app_core::node_service::RAW_CODEC, b"no-assist-content");
        state
            .node
            .put_verified(
                &content_cid,
                app_core::node_service::RAW_CODEC,
                b"no-assist-content",
                false,
                false,
            )
            .unwrap();
        let key = SigningKey::from_bytes(&[27; 32]);
        let track_id = import_assist_track(&state, &key, &content_cid).await;

        let (status, _) = call(
            routes(),
            state.clone(),
            "PUT",
            &format!("/library/tracks/{track_id}/favorite"),
            serde_json::json!({"favorite": true, "request_id": "assist-fav"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, pins) = call(routes(), state, "GET", "/pins", serde_json::json!({})).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            pins.as_array()
                .unwrap()
                .iter()
                .all(|entry| entry["cid"] != content_cid),
            "assist pin must not run when disabled: {pins}"
        );
    }

    #[tokio::test]
    async fn snapshot_endpoint_supports_gzip_with_verifiable_digest() {
        use flate2::read::GzDecoder;
        use std::io::Read;

        let (state, _dir) = state();
        let maintainer = SigningKey::from_bytes(&[23; 32]);
        let mut source = CommunitySourceManifestV1 {
            schema_version: SCHEMA_V1,
            source_id: "snapshot.example".into(),
            name: "Snapshot".into(),
            description: "compression test source".into(),
            languages: vec!["en".into()],
            maintainer_identity_cid: "bafy-maintainer".into(),
            catalog_head: None,
            policy_head: None,
            supported_schemas: vec![SCHEMA_V1],
            report_endpoint: None,
            report_encryption_public_key: None,
            updated_at: 1,
            signature: None,
        };
        source.signature = Some(hex::encode(
            maintainer
                .sign(&source.unsigned_bytes().unwrap())
                .to_bytes(),
        ));
        state
            .community
            .add_source(
                source,
                hex::encode(maintainer.verifying_key().to_bytes()),
                &state.node,
                0,
            )
            .unwrap();

        // 写入较大 Catalog Feed，确保 gzip 传输形态确实更小。
        let mut previous: Option<String> = None;
        for index in 0..80u64 {
            let mut event = CatalogEventV1 {
                schema_version: SCHEMA_V1,
                action: jimmusic_protocol::CatalogAction::Include,
                target_type: "music_manifest".into(),
                target_cid: format!("bafy-target-{index}"),
                categories: vec!["music".into(), "ambient".into()],
                tags: vec!["chill".into(), "lossless".into()],
                annotation: Some(
                    "A deliberately verbose annotation for feed compression coverage. ".repeat(6),
                ),
                sequence: index,
                previous_event_cid: previous.clone(),
                expires_at: None,
                issued_at: 2,
                signature: None,
            };
            event.signature = Some(hex::encode(
                maintainer.sign(&event.unsigned_bytes().unwrap()).to_bytes(),
            ));
            let cid = state
                .community
                .ingest_catalog("snapshot.example", event, &state.node)
                .unwrap();
            previous = Some(cid);
        }

        // 压缩请求：Content-Encoding gzip，摘要头可复验，解压后内容一致。
        let response = routes()
            .with_state(state.clone())
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/community-sources/snapshot.example/snapshot")
                    .header("accept-encoding", "gzip")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response_headers = response.headers().clone();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(response_headers["content-encoding"], "gzip");
        let digest = response_headers["x-snapshot-sha256"].to_str().unwrap();
        let mut plain = Vec::new();
        GzDecoder::new(&bytes[..]).read_to_end(&mut plain).unwrap();
        assert_eq!(crate::state::sha256_hex(&plain), digest);
        assert_eq!(
            response_headers["x-snapshot-bytes"].to_str().unwrap(),
            plain.len().to_string()
        );
        assert!(
            bytes.len() < plain.len(),
            "gzip 传输应小于未压缩快照（{} >= {}）",
            bytes.len(),
            plain.len()
        );
        let snapshot: serde_json::Value = serde_json::from_slice(&plain).unwrap();
        assert_eq!(snapshot["source_id"], "snapshot.example");
        assert_eq!(snapshot["catalog"]["entries"].as_array().unwrap().len(), 80);

        // 未请求压缩时保持普通 JSON 响应。
        let (status, json) = call(
            routes(),
            state,
            "GET",
            "/community-sources/snapshot.example/snapshot",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["source_id"], "snapshot.example");
        assert_eq!(json["catalog"]["entries"].as_array().unwrap().len(), 80);
    }

    #[test]
    fn moderation_endpoint_requires_https_except_loopback_development() {
        assert!(report_endpoint_allowed("https://reports.example.test/v1"));
        assert!(report_endpoint_allowed("http://127.0.0.1:8080/report"));
        assert!(report_endpoint_allowed("http://localhost/report"));
        assert!(!report_endpoint_allowed("http://example.test/report"));
        assert!(!report_endpoint_allowed("file:///tmp/report"));
        assert!(!report_endpoint_allowed(
            "https://user:password@example.test/report"
        ));
    }

    #[test]
    fn community_locator_accepts_cid_ipns_and_qr_uri_forms() {
        assert_eq!(
            parse_community_locator("ipfs://bafyManifest"),
            Some(CommunityLocator::Cid("bafyManifest".into()))
        );
        assert_eq!(
            parse_community_locator("/ipns/community.example"),
            Some(CommunityLocator::Ipns("community.example".into()))
        );
        assert_eq!(
            parse_community_locator("jimmusic://community/bafyManifest"),
            Some(CommunityLocator::Cid("bafyManifest".into()))
        );
        assert_eq!(
            parse_community_locator("jimmusic://community/ipns/community.example"),
            Some(CommunityLocator::Ipns("community.example".into()))
        );
        assert!(parse_community_locator("https://example.test/manifest").is_none());
        assert!(parse_community_locator("ipns://../../secret").is_none());
    }

    #[test]
    fn declarative_configuration_schema_rejects_unknown_and_out_of_range_values() {
        let schema = serde_json::json!({
            "type": "object",
            "required": ["gain"],
            "additionalProperties": false,
            "properties": {
                "gain": {"type": "number", "minimum": 0, "maximum": 1},
                "mode": {"type": "string", "enum": ["clean", "warm"]}
            }
        });
        assert!(validate_schema_value(
            &schema,
            &serde_json::json!({"gain": 0.5, "mode": "warm"}),
            "$"
        )
        .is_ok());
        assert!(validate_schema_value(&schema, &serde_json::json!({"gain": 2}), "$").is_err());
        assert!(validate_schema_value(
            &schema,
            &serde_json::json!({"gain": 0.5, "script": "evil"}),
            "$"
        )
        .is_err());
    }
}
