//! JimMusic 插件管理器（Plugin Manager）。
//!
//! 提供 RESTful 接口用于插件的列举、下载、安装、卸载与升级，并支持 IPFS 源
//! （基于 CID 的查询与下载，优先 IPFS、回退 HTTP 镜像），以及本地仓库缓存、元数据
//! 版本号与签名校验。
//!
//! 对外主要由 [`AppState`]（共享状态）、[`build_router`]（axum 路由）与
//! [`serve`]（启动服务）构成。

mod api;
mod api_v1;
mod auth;
mod idempotency;
mod lifecycle;
mod node;
mod signature;
mod state;
mod transfer_runner;
mod wasm_sandbox;

pub use api::build_router;
pub use auth::{authorize_token, bearer_from_header, constant_time_eq};
pub use lifecycle::{
    PluginInstallOutcome, PluginLifecycleError, PluginLifecycleService, PluginRuntimeRecord,
};
pub use node::NodeIdentity;
pub use signature::{verify as verify_signature, SignatureError};
pub use state::{AppState, PluginRecord, PluginSource};
pub use wasm_sandbox::{
    CapabilityHandle, CapabilityHost, CapabilityOperation, CapabilityRequest,
    DenyAllCapabilityHost, SandboxLimits, WasmPluginInstance, WasmPluginSupervisor,
    WasmSandboxError, WasmSandboxRuntime,
};

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// 启动插件管理器 HTTP 服务并阻塞直到关闭信号到达。
pub async fn serve(
    addr: impl Into<SocketAddr>,
    repo_dir: impl Into<String>,
    ipfs_gateway: impl Into<String>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let repo_dir = repo_dir.into();
    let mut state = AppState::new(repo_dir.clone(), ipfs_gateway.into())?;
    state.start_embedded_node().await?;
    // 控制面始终要求认证。未显式配置时生成并持久化仅本机可读的随机令牌。
    let token = match std::env::var("JIMMUSIC_API_TOKEN") {
        Ok(token) if !token.trim().is_empty() => token,
        _ => load_or_create_control_token(Path::new(&repo_dir))?,
    };
    state = state.with_api_token(Some(token));
    let reliability = state.reliability.clone();
    let router = build_router(state);

    let listener = match tokio::net::TcpListener::bind(addr.into()).await {
        Ok(listener) => listener,
        Err(error) => {
            let _ = reliability.finish_clean();
            return Err(error.into());
        }
    };
    tracing::info!("plugin-manager listening on {}", listener.local_addr()?);

    let result = axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await;
    reliability.finish_clean()?;
    result.map_err(Into::into)
}

fn load_or_create_control_token(
    repo_dir: &Path,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    std::fs::create_dir_all(repo_dir)?;
    let path = repo_dir.join("control-token");
    if path.exists() {
        let token = std::fs::read_to_string(&path)?;
        let token = token.trim().to_string();
        if token.len() < 32 {
            return Err(format!("control token at {} is invalid", path.display()).into());
        }
        return Ok(token);
    }
    let mut random = [0u8; 32];
    getrandom::fill(&mut random)?;
    let token = hex::encode(random);
    write_private_file(&path, token.as_bytes())?;
    tracing::info!(path = %path.display(), "created local control token");
    Ok(token)
}

fn write_private_file(path: &PathBuf, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// 监听 Ctrl-C / SIGTERM 以优雅关闭。
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl-C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received, exiting");
}
