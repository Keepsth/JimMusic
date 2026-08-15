//! web-audio 输出插件的端到端 FFI 测试：经 [`OutputPlugin`] 加载真实 `web-audio`
//! 动态库，校验其为 `AudioOutput` 类型、解析能力，并走通「解码 PCM → 输出插件」。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use app_core::engine::FfiSink;
use app_core::output::OutputPlugin;
use app_core::{Event, EventBus, PlaybackEngine};
use plugin_abi::output::{OutputOpenParams, PcmFormat};

fn locate_web_audio() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let deps = exe.parent()?;
    let debug = deps.parent()?;
    for name in ["libweb_audio.so", "libweb_audio.dylib", "web_audio.dll"] {
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
async fn web_audio_plugin_loads_and_plays() {
    let Some(path) = locate_web_audio() else {
        eprintln!("web-audio cdylib not built; skipping web-audio FFI test");
        return;
    };

    // 1. 加载并校验类型与能力。
    let plugin = Arc::new(OutputPlugin::load(&path).expect("load web-audio"));
    assert_eq!(plugin.name(), "web-audio");
    let caps = plugin.capabilities();
    assert_eq!(caps.backend, "web-audio");
    assert_eq!(caps.platform, "web");
    assert!(caps.features.low_latency);

    // 2. 打开输出流并走通播放。
    let params = OutputOpenParams {
        sample_rate: 44_100,
        channels: 1,
        format: PcmFormat::I16Interleaved as i32,
        buffer_frames: 512,
    };
    let stream = plugin.open(params).expect("open stream");
    let sink: Arc<dyn app_core::PcmSink> = Arc::new(FfiSink::new(stream));

    let bus = EventBus::new(256);
    let mut rx = bus.subscribe();
    let engine = PlaybackEngine::new(bus);
    engine.set_output(sink).await;

    let samples: Vec<i16> = (0..4096).map(|i| (i % 1000) as i16).collect();
    engine
        .play_pcm("web-track".into(), 44_100, 1, samples, 4096.0 / 44_100.0)
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
