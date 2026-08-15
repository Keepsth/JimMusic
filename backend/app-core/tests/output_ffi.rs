//! 端到端 FFI 测试：加载真实 `null-output` 动态库，经 [`OutputPlugin`] → [`FfiSink`] →
//! [`PlaybackEngine`] 走通「解码 PCM → 有界缓冲 → 输出插件」完整链路。
//!
//! 依赖 `null-output` 动态库已被构建（`cargo build --workspace` 或
//! `cargo test --workspace` 会构建它）。若找不到库则跳过（不影响单 crate 测试）。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use app_core::engine::{FfiSink, PlaybackEngine};
use app_core::output::OutputPlugin;
use app_core::Event;
use plugin_abi::output::{OutputOpenParams, PcmFormat};

/// 定位 `null-output` 动态库路径；不存在则返回 `None`（测试跳过）。
///
/// 不依赖 `CARGO_TARGET_DIR`（其不会在测试进程运行时可靠注入），而是从测试二进制
/// 自身位置推导目标目录：测试二进制位于 `<target>/debug/deps/`。优先加载顶层构建
/// 产物，避免增量构建留下的旧依赖副本掩盖当前 ABI。
fn locate_null_output() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let deps = exe.parent()?; // <target>/debug/deps
    let debug = deps.parent()?; // <target>/debug
    for name in [
        "libnull_output.so",
        "libnull_output.dylib",
        "null_output.dll",
    ] {
        let top = debug.join(name);
        if top.exists() {
            return Some(top);
        }
        let dep = deps.join(name);
        if dep.exists() {
            return Some(dep);
        }
    }
    None
}

#[tokio::test]
async fn output_plugin_loads_and_plays_through_engine() {
    let Some(path) = locate_null_output() else {
        eprintln!("null-output cdylib not built; skipping end-to-end FFI test");
        return;
    };

    // 1. 加载输出插件并校验能力。
    let plugin = Arc::new(OutputPlugin::load(&path).expect("load output plugin"));
    assert_eq!(plugin.name(), "null-output");
    let caps = plugin.capabilities();
    assert_eq!(caps.backend, "null");
    assert_eq!(caps.platform, "any");
    assert!(!caps.sample_rates.is_empty());
    assert!(caps.features.low_latency);

    // 2. 打开输出流。
    let params = OutputOpenParams {
        sample_rate: 44_100,
        channels: 2,
        format: PcmFormat::I16Interleaved as i32,
        buffer_frames: 512,
    };
    let stream = plugin.open(params).expect("open stream");
    let sink: Arc<dyn app_core::PcmSink> = Arc::new(FfiSink::new(stream));

    // 3. 经播放引擎播放一段 PCM，验证事件与完成。
    let bus = app_core::EventBus::new(256);
    let mut rx = bus.subscribe();
    let engine = PlaybackEngine::new(bus);
    engine.set_output(sink).await;

    // 2 声道 × 4096 帧 = 8192 样本。
    let samples: Vec<i16> = (0..8192).map(|i| (i % 1000) as i16).collect();
    engine
        .play_pcm("ffi-track".into(), 44_100, 2, samples, 4096.0 / 44_100.0)
        .await;

    assert!(matches!(rx.recv().await.unwrap(), Event::Played { .. }));

    let mut completed = false;
    while let Ok(ev) = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
        match ev {
            Ok(Event::Completed { .. }) => {
                completed = true;
                break;
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    assert!(completed, "playback should complete and publish Completed");
    assert_eq!(engine.state().await, app_core::EngineState::Stopped);
}
