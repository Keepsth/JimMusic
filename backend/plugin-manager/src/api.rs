//! RESTful API 路由与处理器。

use std::sync::Arc;

use axum::{
    body::{to_bytes, Body},
    extract::{Path, Query, State},
    http::{Method, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

#[cfg(test)]
use crate::state::PluginSource;
use crate::state::{sha256_hex, AppState, PluginRecord};

/// 构建 axum 路由。写操作（安装/卸载/升级）受节点认证保护。
pub fn build_router(state: AppState) -> Router {
    let state = Arc::new(state);
    if tokio::runtime::Handle::try_current().is_ok() {
        for task in state
            .transfers
            .list()
            .into_iter()
            .filter(|task| task.state == jimmusic_protocol::TransferState::Queued)
        {
            crate::transfer_runner::spawn(state.clone(), task.task_id);
        }
        let retry_state = state.clone();
        tokio::spawn(async move {
            loop {
                crate::api_v1::retry_due_moderation_reports(retry_state.clone()).await;
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            }
        });
    }

    // 公开只读路由。
    let public = Router::new()
        .route("/health", get(health))
        .route("/plugins", get(list_plugins))
        .route("/plugins/search", get(search_plugins))
        .route("/plugins/{name}", get(get_plugin))
        .route("/outputs", get(list_outputs));

    // 受保护写路由：包一层认证中间件。
    let protected = Router::new()
        .route("/plugins/install", post(deprecated_plugin_mutation))
        .route(
            "/plugins/{name}",
            axum::routing::delete(deprecated_plugin_mutation),
        )
        .route("/plugins/{name}/upgrade", post(deprecated_plugin_mutation))
        .route("/outputs/{name}/activate", post(activate_output))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth));

    let v1 = crate::api_v1::routes()
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_idempotency,
        ))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth));

    public.merge(protected).nest("/v1", v1).with_state(state)
}

/// Enforce and persistently replay every mutating v1 request. Individual
/// services still keep their domain idempotency checks; this transport layer
/// closes gaps for configuration, lifecycle and library mutations.
async fn require_idempotency(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS) {
        return next.run(req).await;
    }
    const BODY_LIMIT: usize = 8 * 1024 * 1024;
    let (parts, body) = req.into_parts();
    let bytes = match to_bytes(body, BODY_LIMIT).await {
        Ok(bytes) => bytes,
        Err(error) => {
            return v1_middleware_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "request_too_large",
                error.to_string(),
            )
        }
    };
    let body_request_id = serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|value| {
            value
                .get("request_id")
                .and_then(str_value)
                .map(str::to_owned)
        });
    let request_id = parts
        .headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .or(body_request_id)
        .filter(|value| !value.trim().is_empty() && value.len() <= 200);
    let Some(request_id) = request_id else {
        return v1_middleware_error(
            StatusCode::BAD_REQUEST,
            "idempotency_key_required",
            "mutating v1 requests require request_id or Idempotency-Key",
        );
    };
    let scope = format!("{} {}", parts.method, parts.uri.path());
    let mut fingerprint_bytes = scope.as_bytes().to_vec();
    if let Some(query) = parts.uri.query() {
        fingerprint_bytes.extend_from_slice(query.as_bytes());
    }
    fingerprint_bytes.extend_from_slice(&bytes);
    let fingerprint = sha256_hex(&fingerprint_bytes);
    let _guard = state.idempotency.lock_http().await;
    match state
        .idempotency
        .lookup_http(&scope, &request_id, &fingerprint)
    {
        Ok(Some(replay)) => {
            let status =
                StatusCode::from_u16(replay.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            return (status, Json(replay.body)).into_response();
        }
        Ok(None) => {}
        Err(crate::idempotency::IdempotencyError::Conflict) => {
            return v1_middleware_error(
                StatusCode::CONFLICT,
                "idempotency_conflict",
                "Idempotency-Key was already used for a different request",
            )
        }
        Err(error) => {
            return v1_middleware_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "idempotency_unavailable",
                error.to_string(),
            )
        }
    }
    let response = next
        .run(Request::from_parts(parts, Body::from(bytes)))
        .await;
    if !response.status().is_success() {
        return response;
    }
    let (response_parts, response_body) = response.into_parts();
    let response_bytes = match to_bytes(response_body, BODY_LIMIT).await {
        Ok(bytes) => bytes,
        Err(error) => {
            return v1_middleware_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "response_too_large",
                error.to_string(),
            )
        }
    };
    if let Ok(body) = serde_json::from_slice::<serde_json::Value>(&response_bytes) {
        if let Err(error) = state.idempotency.store_http(
            &scope,
            &request_id,
            &fingerprint,
            crate::idempotency::HttpReplay {
                status: response_parts.status.as_u16(),
                body,
            },
        ) {
            return v1_middleware_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "idempotency_unavailable",
                error.to_string(),
            );
        }
    }
    Response::from_parts(response_parts, Body::from(response_bytes))
}

fn str_value(value: &serde_json::Value) -> Option<&str> {
    value.as_str()
}

fn v1_middleware_error(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
    (
        status,
        Json(jimmusic_protocol::ErrorEnvelopeV1 {
            schema_version: jimmusic_protocol::SCHEMA_V1,
            code: code.into(),
            message: message.into(),
            subsystem: "api".into(),
            operation: "idempotency".into(),
            retryable: false,
            unsupported_reason: None,
            details: Default::default(),
            request_id: None,
            causes: Vec::new(),
        }),
    )
        .into_response()
}

/// 1.x 的直接动态库写接口无法表达 2.x 的 Manifest、权限与回滚事务，明确退役，
/// 防止客户端绕过 `/v1/plugins/install` 的安全边界。
async fn deprecated_plugin_mutation() -> Response {
    (
        StatusCode::GONE,
        Json(ErrorBody {
            error: "legacy plugin mutation API is retired; use /v1/plugins/install with a signed manifest".into(),
        }),
    )
        .into_response()
}

/// 认证中间件：校验 `Authorization: Bearer <token>`。
async fn require_auth(
    State(state): State<Arc<AppState>>,
    req: axum::http::Request<axum::body::Body>,
    next: Next,
) -> Response {
    let header = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    if state.authorize_header(header) {
        next.run(req).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(ErrorBody {
                error: "unauthorized: missing or invalid API token".into(),
            }),
        )
            .into_response()
    }
}

/// 统一错误响应。
#[derive(Debug, Serialize)]
struct ErrorBody {
    error: String,
}

impl IntoResponse for ErrorBody {
    fn into_response(self) -> Response {
        (StatusCode::BAD_REQUEST, Json(self)).into_response()
    }
}

fn not_found(msg: impl Into<String>) -> Response {
    (StatusCode::NOT_FOUND, Json(ErrorBody { error: msg.into() })).into_response()
}

/// 健康检查。
async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

/// 列举已安装插件（可按 `?kind=` 过滤，如 `?kind=output`）。
async fn list_plugins(
    State(state): State<Arc<AppState>>,
    Query(query): Query<KindQuery>,
) -> Json<Vec<PluginRecord>> {
    Json(state.list_by_kind(query.kind.as_deref()))
}

/// 列举查询参数。
#[derive(Debug, Deserialize, Default)]
struct KindQuery {
    /// 可选插件种类过滤（如 `output` / `decoder`）。
    kind: Option<String>,
}

/// 列举音频输出插件及其激活状态。
async fn list_outputs(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let outputs: Vec<serde_json::Value> = state
        .list_by_kind(Some("output"))
        .into_iter()
        .map(|r| {
            serde_json::json!({
                "name": r.name,
                "version": r.version,
                "active": state.active_output().as_deref() == Some(r.name.as_str()),
            })
        })
        .collect();
    Json(serde_json::json!({
        "active": state.active_output(),
        "outputs": outputs,
    }))
}

/// 激活（切换）音频输出插件：校验目标为 output 类型后置为当前输出。
async fn activate_output(State(state): State<Arc<AppState>>, Path(name): Path<String>) -> Response {
    match state.activate_output(&name) {
        Ok(()) => Json(serde_json::json!({ "activated": name })).into_response(),
        Err(e) => not_found(e),
    }
}

/// 查询单个插件元数据。
async fn get_plugin(State(state): State<Arc<AppState>>, Path(name): Path<String>) -> Response {
    match state.get_record(&name) {
        Some(rec) => Json(rec).into_response(),
        None => not_found(format!("plugin `{name}` not found")),
    }
}

/// 搜索查询参数。
#[derive(Debug, Deserialize)]
struct SearchQuery {
    q: String,
}

/// 搜索插件：当前原型仅按名字子串过滤本地记录（远程搜索需接入 IPFS 目录）。
async fn search_plugins(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SearchQuery>,
) -> Json<Vec<PluginRecord>> {
    let needle = query.q.to_lowercase();
    Json(
        state
            .list_records()
            .into_iter()
            .filter(|r| r.name.to_lowercase().contains(&needle))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    /// 返回一个使用唯一临时目录的状态（`TempDir` 随测试结束自动清理，避免并行竞态）。
    fn test_state() -> (AppState, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(
            dir.path().to_string_lossy().into_owned(),
            "http://127.0.0.1:5001".into(),
        )
        .unwrap();
        (state, dir)
    }

    /// 向路由发起一次请求并返回 (status, body bytes)。
    async fn call(app: &Router, method: &str, uri: &str, body: String) -> (StatusCode, Vec<u8>) {
        let req = axum::http::Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec();
        (status, bytes)
    }

    async fn call_with_key(
        app: &Router,
        method: &str,
        uri: &str,
        body: String,
        key: &str,
    ) -> (StatusCode, Vec<u8>) {
        let req = axum::http::Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .header("idempotency-key", key)
            .body(axum::body::Body::from(body))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec();
        (status, bytes)
    }

    fn make_record(name: &str) -> PluginRecord {
        PluginRecord {
            name: name.into(),
            version: "1.0.0".into(),
            author: "a".into(),
            kind: "decoder".into(),
            source: PluginSource::Local,
            location: None,
            sha256: None,
            lib_path: None,
        }
    }

    fn make_output_record(name: &str) -> PluginRecord {
        PluginRecord {
            name: name.into(),
            version: "1.0.0".into(),
            author: "a".into(),
            kind: "output".into(),
            source: PluginSource::Local,
            location: None,
            sha256: None,
            lib_path: None,
        }
    }

    #[tokio::test]
    async fn health_returns_ok() {
        let (state, _dir) = test_state();
        let app = build_router(state);
        // 直接调用 health 处理器。
        let Json(v) = health().await;
        assert_eq!(v["status"], "ok");
        let _ = app;
    }

    #[tokio::test]
    async fn every_v1_mutation_requires_and_replays_an_idempotency_key() {
        let (state, _dir) = test_state();
        let app = build_router(state);
        let body = serde_json::json!({"name": "Road trip"}).to_string();
        let (status, error) = call(&app, "POST", "/v1/library/playlists", body.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&error).unwrap()["code"],
            "idempotency_key_required"
        );

        let (status, first) = call_with_key(
            &app,
            "POST",
            "/v1/library/playlists",
            body.clone(),
            "playlist-request",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, replay) = call_with_key(
            &app,
            "POST",
            "/v1/library/playlists",
            body,
            "playlist-request",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&first).unwrap(),
            serde_json::from_slice::<serde_json::Value>(&replay).unwrap()
        );

        let (status, _) = call_with_key(
            &app,
            "POST",
            "/v1/library/playlists",
            serde_json::json!({"name": "Different"}).to_string(),
            "playlist-request",
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn list_plugins_via_router() {
        let (state, _dir) = test_state();
        state.upsert_record(make_record("demo"));
        let app = build_router(state);

        let (status, body) = call(&app, "GET", "/plugins", String::new()).await;
        assert_eq!(status, StatusCode::OK);
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn get_plugin_via_router() {
        let (state, _dir) = test_state();
        state.upsert_record(make_record("demo"));
        let app = build_router(state);

        let (status, _) = call(&app, "GET", "/plugins/demo", String::new()).await;
        assert_eq!(status, StatusCode::OK);

        let (status, _) = call(&app, "GET", "/plugins/missing", String::new()).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn search_plugins_via_router() {
        let (state, _dir) = test_state();
        state.upsert_record(make_record("foo-bar"));
        state.upsert_record(make_record("baz"));
        let app = build_router(state);

        let (status, body) = call(&app, "GET", "/plugins/search?q=foo", String::new()).await;
        assert_eq!(status, StatusCode::OK);
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 1);
        assert_eq!(json[0]["name"], "foo-bar");
    }

    #[tokio::test]
    async fn legacy_uninstall_is_retired_without_mutating_state() {
        let (state, _dir) = test_state();
        state.upsert_record(make_record("demo"));
        let app = build_router(state.clone());

        let (status, _) = call(&app, "DELETE", "/plugins/demo", String::new()).await;
        assert_eq!(status, StatusCode::GONE);
        assert!(state.get_record("demo").is_some());
    }

    #[tokio::test]
    async fn install_requires_auth_when_configured() {
        let (state, _dir) = test_state();
        let state = state.with_api_token(Some("s3cret".into()));
        let app = build_router(state);

        let body = r#"{"name":"demo","version":"1.0.0","source":"http","location":"http://x"}"#;
        // 无 Authorization 头 → 401。
        let (status, _) = call(&app, "POST", "/plugins/install", body.to_string()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn legacy_install_is_retired() {
        let (state, _dir) = test_state();
        let app = build_router(state.clone());
        let body = serde_json::json!({
            "name": "demo",
            "version": "1.0.0",
            "source": "http",
            "location": "https://example.invalid/plugin.so",
        })
        .to_string();

        let (status, resp) = call(&app, "POST", "/plugins/install", body).await;
        assert_eq!(status, StatusCode::GONE);
        assert!(String::from_utf8(resp)
            .unwrap()
            .contains("/v1/plugins/install"));
        assert!(state.get_record("demo").is_none());
    }

    #[test]
    fn sha256_file_matches_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.bin");
        std::fs::write(&p, b"hello").unwrap();
        assert_eq!(crate::state::sha256_file(&p).unwrap(), sha256_hex(b"hello"));
    }

    #[tokio::test]
    async fn legacy_upgrade_is_retired() {
        let (state, _dir) = test_state();
        state.upsert_record(make_record("demo"));
        let app = build_router(state.clone());

        let body = serde_json::json!({
            "version": "2.0.0",
            "source": "http",
            "location": "https://example.invalid/plugin.so",
        })
        .to_string();
        let (status, _) = call(&app, "POST", "/plugins/demo/upgrade", body).await;
        assert_eq!(status, StatusCode::GONE);
        assert_eq!(state.get_record("demo").unwrap().version, "1.0.0");
    }

    #[tokio::test]
    async fn legacy_upgrade_does_not_disclose_plugin_presence() {
        let (state, _dir) = test_state();
        let app = build_router(state);
        let body = r#"{"version":"2.0.0"}"#;
        let (status, _) = call(&app, "POST", "/plugins/missing/upgrade", body.to_string()).await;
        assert_eq!(status, StatusCode::GONE);
    }

    #[tokio::test]
    async fn list_plugins_filters_by_kind() {
        let (state, _dir) = test_state();
        state.upsert_record(make_record("dec"));
        state.upsert_record(make_output_record("out"));
        let app = build_router(state);

        // 默认列举全部。
        let (status, body) = call(&app, "GET", "/plugins", String::new()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body)
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            2
        );

        // 按 kind=output 过滤。
        let (status, body) = call(&app, "GET", "/plugins?kind=output", String::new()).await;
        assert_eq!(status, StatusCode::OK);
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["name"], "out");
    }

    #[tokio::test]
    async fn list_outputs_reports_active_state() {
        let (state, _dir) = test_state();
        state.upsert_record(make_record("dec"));
        state.upsert_record(make_output_record("null-out"));
        let app = build_router(state.clone());

        let (status, body) = call(&app, "GET", "/outputs", String::new()).await;
        assert_eq!(status, StatusCode::OK);
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["active"], serde_json::Value::Null);
        assert_eq!(json["outputs"].as_array().unwrap().len(), 1);
        assert_eq!(json["outputs"][0]["name"], "null-out");
        assert_eq!(json["outputs"][0]["active"], false);
    }

    #[tokio::test]
    async fn activate_output_switches_active() {
        let (state, _dir) = test_state();
        state.upsert_record(make_output_record("alsa"));
        state.upsert_record(make_output_record("null-out"));
        let app = build_router(state.clone());

        // 激活 alsa。
        let (status, _) = call(&app, "POST", "/outputs/alsa/activate", String::new()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(state.active_output().as_deref(), Some("alsa"));

        // 切换到 null-out。
        let (status, body) = call(&app, "POST", "/outputs/null-out/activate", String::new()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["activated"],
            "null-out"
        );
        assert_eq!(state.active_output().as_deref(), Some("null-out"));
    }

    #[tokio::test]
    async fn activate_non_output_fails() {
        let (state, _dir) = test_state();
        state.upsert_record(make_record("dec"));
        let app = build_router(state);

        let (status, _) = call(&app, "POST", "/outputs/dec/activate", String::new()).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // 不存在的插件同样 404。
        let (status, _) = call(&app, "POST", "/outputs/missing/activate", String::new()).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn legacy_output_install_is_retired() {
        let (state, _dir) = test_state();
        let app = build_router(state.clone());
        let body = serde_json::json!({
            "name": "null-out",
            "version": "1.0.0",
            "kind": "output",
            "source": "http",
            "location": "https://example.invalid/out.so",
        })
        .to_string();

        let (status, _) = call(&app, "POST", "/plugins/install", body).await;
        assert_eq!(status, StatusCode::GONE);
        assert!(state.get_record("null-out").is_none());
    }
}
