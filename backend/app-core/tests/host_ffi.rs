//! 端到端 FFI 桥测试：经 C ABI 宿主符号（`jimmusic_host_*`）走通
//! 「加载输出插件 → 播放文件 → 状态/进度回传 → 停止」完整链路。
//!
//! 依赖 `null-output` 动态库已被构建（`cargo build --workspace` 会构建它）。
//! 若找不到库则跳过。

use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

use app_core::host::{
    jimmusic_host_play_file, jimmusic_host_set_event_callback, jimmusic_host_set_output,
    jimmusic_host_state, jimmusic_host_stop, EVENT_PLAYING, EVENT_PROGRESS, EVENT_STOPPED,
};

/// 定位 `null-output` 动态库路径（与 output_ffi.rs 同款推导逻辑）。
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

/// 生成一个 1 秒、8kHz、单声道 WAV（用于真实解码播放）。
fn write_wav(path: &std::path::Path) {
    use std::io::Write;
    let sr = 8000u32;
    let n = 8000usize;
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

// 全局事件记录（C 回调无上下文，用静态原子记录事件类型）。
static SEEN_PLAYING: AtomicI32 = AtomicI32::new(0);
static SEEN_PROGRESS: AtomicI32 = AtomicI32::new(0);
static SEEN_STOPPED: AtomicI32 = AtomicI32::new(0);

extern "C" fn on_event(event_type: i32, _value: f64) {
    match event_type {
        EVENT_PLAYING => SEEN_PLAYING.store(1, Ordering::SeqCst),
        EVENT_PROGRESS => SEEN_PROGRESS.store(1, Ordering::SeqCst),
        EVENT_STOPPED => SEEN_STOPPED.store(1, Ordering::SeqCst),
        _ => {}
    }
}

#[test]
fn host_bridge_plays_file_through_null_output() {
    let Some(out_path) = locate_null_output() else {
        eprintln!("null-output cdylib not built; skipping host bridge test");
        return;
    };

    // 1. 加载输出插件。
    let out_c = std::ffi::CString::new(out_path.to_string_lossy().into_owned()).unwrap();
    assert_eq!(
        jimmusic_host_set_output(out_c.as_ptr()),
        0,
        "set_output should succeed"
    );

    // 2. 注册事件回调。
    assert_eq!(jimmusic_host_set_event_callback(Some(on_event)), 0);

    // 3. 播放 WAV（异步解码，立即返回）。
    let tmp = std::env::temp_dir().join("jimmusic_host_test.wav");
    write_wav(&tmp);
    let track = std::ffi::CString::new("host-track").unwrap();
    let path = std::ffi::CString::new(tmp.to_string_lossy().into_owned()).unwrap();
    assert_eq!(jimmusic_host_play_file(track.as_ptr(), path.as_ptr()), 0);

    // 4. 等待播放完成（收到停止事件）。
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if SEEN_STOPPED.load(Ordering::SeqCst) == 1 {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    // 5. 断言：收到播放/进度/（完成映射的）停止事件。
    assert_eq!(
        SEEN_PLAYING.load(Ordering::SeqCst),
        1,
        "should receive Played event"
    );
    assert_eq!(
        SEEN_PROGRESS.load(Ordering::SeqCst),
        1,
        "should receive Progress events"
    );
    assert_eq!(
        SEEN_STOPPED.load(Ordering::SeqCst),
        1,
        "should receive Stopped event"
    );

    // 6. Player 单曲队列会自动循环，这里显式停止后断言状态复位。
    jimmusic_host_stop();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(jimmusic_host_state(), 0, "final state should be stopped");

    // 7. 取消回调，避免进程退出时持有野指针。
    jimmusic_host_set_event_callback(None);

    let _ = std::fs::remove_file(&tmp);
}
