//! JimMusic 应用程序核心（App Core）。
//!
//! 该 crate 提供跨平台音乐播放器的核心能力：
//! - **插件管理**：[`plugin::PluginManager`] 负责动态发现、加载、卸载 `.so/.dll/.dylib`
//!   插件，并基于 [`plugin_abi`] 的统一 C ABI 调用插件能力；
//! - **消息总线**：[`event::EventBus`] 基于 Tokio 广播通道，用于播放/暂停/进度等事件的
//!   异步分发；
//! - **日志与错误处理**：[`init_logging`] 配置可定级日志，[`CoreError`] 提供统一错误码；
//! - **IPFS 接入**：[`ipfs::IpfsClient`] 通过 HTTP API 并发执行 CID 查询、数据下载与
//!   流式传输，并支持本地缓存策略。
//!
//! 所有异步能力均基于 Tokio，整体设计支持跨平台（Android/iOS/HarmonyOS/桌面）。

pub mod audio;
pub mod audio_graph;
pub mod bridge;
pub mod cache;
pub mod community_service;
pub mod crypto;
pub mod engine;
pub mod error;
pub mod event;
pub mod host;
pub mod identity;
pub mod ipfs;
pub mod library_service;
pub mod media;
pub mod node_service;
pub mod output;
pub mod p2p_node;
pub mod player;
pub mod plugin;
pub mod publication_service;
pub mod reliability;
pub mod storage;
pub mod streaming;
#[doc(hidden)]
pub mod test_contracts;
pub mod transfer_service;

pub use audio::{PcmChunk, PcmQueue, PcmQueueClosed};
pub use bridge::{event_to_op, forward_event_to_ui};
pub use cache::{ContentCache, LruCache};
pub use engine::{EngineState, FfiSink, PcmSink, PlaybackEngine};
pub use error::{CoreError, CoreResult};
pub use event::{Event, EventBus};
pub use ipfs::{sha256_hex, IpfsClient};
pub use media::{MediaLibrary, Track};
pub use output::{OutputError, OutputPlugin, OutputSessionFormat, OutputSessionInfo, OutputStream};
pub use player::{PlaybackState, Player};
pub use plugin::{LoadedPlugin, PluginManager, PluginMeta};
pub use streaming::{download_and_decode, StreamError};

// 便捷再导出插件 ABI 的核心类型，便于上游（如示例、plugin-manager）使用。
pub use plugin_abi::{ErrorCode, PluginInfo, PluginKind, ABI_VERSION};

/// 初始化全局日志。使用 `RUST_LOG` 环境变量控制级别，默认 `info`。
pub fn init_logging() {
    use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_target(true))
        .init();
}
