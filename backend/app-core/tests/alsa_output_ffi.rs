//! alsa-output 输出插件的端到端 FFI 测试：经 [`OutputPlugin`] 加载真实 `alsa-output`
//! 动态库，校验其为 `AudioOutput` 类型、解析能力；`open` 在无音频设备环境下应优雅失败。

use std::path::PathBuf;
use std::sync::Arc;

use app_core::output::OutputPlugin;
use plugin_abi::output::{OutputOpenParams, PcmFormat};

fn locate_alsa_output() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let deps = exe.parent()?;
    let debug = deps.parent()?;
    for name in [
        "libalsa_output.so",
        "libalsa_output.dylib",
        "alsa_output.dll",
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

#[test]
fn alsa_output_plugin_loads_and_reports_capabilities() {
    let Some(path) = locate_alsa_output() else {
        eprintln!("alsa-output cdylib not built; skipping alsa-output FFI test");
        return;
    };

    // 1. 加载并校验类型与能力（不依赖真实设备）。
    let plugin = Arc::new(OutputPlugin::load(&path).expect("load alsa-output"));
    assert_eq!(plugin.name(), "alsa-output");
    let caps = plugin.capabilities();
    assert_eq!(caps.backend, "alsa");
    assert_eq!(caps.platform, "linux");
    assert!(caps.features.low_latency);

    // 2. open：无有效音频设备时应返回错误（不 panic）；有设备时走通基本控制。
    let params = OutputOpenParams {
        sample_rate: 44_100,
        channels: 2,
        format: PcmFormat::I16Interleaved as i32,
        buffer_frames: 512,
    };
    if let Ok(stream) = plugin.open(params) {
        assert_eq!(stream.buffered_frames(), 0);
        let _ = stream.play();
        let _ = stream.pause();
        let _ = stream.stop();
    }
}
