//! host 队列接口的端到端 FFI 测试：`set_queue` + `play_track` → 自动切歌 → `current_index`
//! 推进，验证桥层已接入 Player 的统一队列/自动切歌能力。

use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

use app_core::host::{
    jimmusic_host_current_index, jimmusic_host_play_track, jimmusic_host_set_crossfade,
    jimmusic_host_set_output, jimmusic_host_set_queue, jimmusic_host_state, jimmusic_host_stop,
};

fn locate_null_output() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let deps = exe.parent()?;
    let debug = deps.parent()?;
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

fn write_wav(path: &std::path::Path) {
    use std::io::Write;
    let sr = 8000u32;
    let n = 800usize;
    let data_len = (n * 2) as u32;
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(b"RIFF").unwrap();
    f.write_all(&(36 + data_len).to_le_bytes()).unwrap();
    f.write_all(b"WAVE").unwrap();
    f.write_all(b"fmt ").unwrap();
    f.write_all(&16u32.to_le_bytes()).unwrap();
    f.write_all(&1u16.to_le_bytes()).unwrap();
    f.write_all(&1u16.to_le_bytes()).unwrap();
    f.write_all(&sr.to_le_bytes()).unwrap();
    f.write_all(&(sr * 2).to_le_bytes()).unwrap();
    f.write_all(&2u16.to_le_bytes()).unwrap();
    f.write_all(&16u16.to_le_bytes()).unwrap();
    f.write_all(b"data").unwrap();
    f.write_all(&data_len.to_le_bytes()).unwrap();
    for i in 0..n {
        let s = ((i as f64).sin() * 1000.0) as i16;
        f.write_all(&s.to_le_bytes()).unwrap();
    }
}

static SEEN_PLAYING: AtomicI32 = AtomicI32::new(0);

extern "C" fn on_event(event_type: i32, _value: f64) {
    // 1 = playing
    if event_type == 1 {
        SEEN_PLAYING.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn host_queue_auto_advances() {
    let Some(out_path) = locate_null_output() else {
        eprintln!("null-output cdylib not built; skipping host queue test");
        return;
    };

    // 1. 加载输出插件。
    let out_c = std::ffi::CString::new(out_path.to_string_lossy().into_owned()).unwrap();
    assert_eq!(jimmusic_host_set_output(out_c.as_ptr()), 0);
    app_core::host::jimmusic_host_set_event_callback(Some(on_event));

    // 2. 两个 WAV + 队列。
    let tmp = std::env::temp_dir();
    let wav1 = tmp.join("jimmusic_hq_a.wav");
    let wav2 = tmp.join("jimmusic_hq_b.wav");
    write_wav(&wav1);
    write_wav(&wav2);

    let paths = serde_json::json!([wav1.to_string_lossy(), wav2.to_string_lossy()]).to_string();
    let path_c = std::ffi::CString::new(paths).unwrap();
    assert_eq!(jimmusic_host_set_queue(path_c.as_ptr()), 0);
    assert_eq!(jimmusic_host_set_crossfade(25, 0), 0);
    assert_eq!(
        jimmusic_host_set_crossfade(25, 7),
        plugin_abi::ErrorCode::InvalidArgument.as_i32()
    );

    // 3. 播放第一首，等待自动切到第二首（短音频自动切歌很快）。
    assert_eq!(jimmusic_host_play_track(0), 0);

    // 4. 等待自动切歌（current_index 变 1）。
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if jimmusic_host_current_index() == 1 {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        SEEN_PLAYING.load(Ordering::SeqCst) >= 2,
        "initial playback and the real crossfade boundary should both be observable"
    );
    assert_eq!(
        jimmusic_host_current_index(),
        1,
        "should auto-advance to index 1"
    );

    // 5. 停止循环播放。
    jimmusic_host_stop();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(jimmusic_host_state(), 0, "final state should be stopped");

    app_core::host::jimmusic_host_set_event_callback(None);
    let _ = std::fs::remove_file(&wav1);
    let _ = std::fs::remove_file(&wav2);
}
