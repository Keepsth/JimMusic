//! plugin-manager 可执行入口：启动 HTTP 服务。

use std::env;

#[tokio::main]
async fn main() {
    app_core::init_logging();

    let addr = parse_addr();
    let repo_dir = env::var("JIMMUSIC_REPO_DIR").unwrap_or_else(|_| "./repo".into());
    let ipfs_gateway =
        env::var("JIMMUSIC_IPFS_GATEWAY").unwrap_or_else(|_| "http://127.0.0.1:5001".into());

    if let Err(e) = plugin_manager::serve(addr, repo_dir, ipfs_gateway).await {
        tracing::error!("plugin-manager exited with error: {e}");
        std::process::exit(1);
    }
}

fn parse_addr() -> std::net::SocketAddr {
    env::var("JIMMUSIC_BIND_ADDR")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| "127.0.0.1:8787".parse().expect("valid default addr"))
}
