//! 整合后的 `Player` 端到端测试：经真实输出插件验证
//! 「播放 → 自然完成 → 自动切下一首」链路。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use app_core::engine::FfiSink;
use app_core::output::OutputPlugin;
use app_core::player::Player;
use app_core::{Event, EventBus, Track};
use plugin_abi::output::{OutputOpenParams, PcmFormat};

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

/// 生成一个约 0.1 秒、8kHz、单声道 WAV。
fn write_wav(path: &std::path::Path) {
    use std::io::Write;
    let sr = 8000u32;
    let n = 800usize; // 0.1s
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

fn make_track(path: &str, title: &str) -> Track {
    Track {
        path: path.to_string(),
        title: title.to_string(),
        artist: None,
        album: None,
        duration: Some(0.1),
        sample_rate: Some(8000),
        channels: Some(1),
    }
}

#[tokio::test]
async fn player_real_playback_auto_advances() {
    let Some(out_path) = locate_null_output() else {
        eprintln!("null-output cdylib not built; skipping player integration test");
        return;
    };

    // 1. 加载输出插件并设为 Player 的输出。
    let plugin = Arc::new(OutputPlugin::load(&out_path).expect("load null-output"));
    let params = OutputOpenParams {
        sample_rate: 8000,
        channels: 1,
        format: PcmFormat::I16Interleaved as i32,
        buffer_frames: 512,
    };
    let stream = plugin.open(params).expect("open stream");
    let sink: Arc<dyn app_core::PcmSink> = Arc::new(FfiSink::new(stream));

    let bus = EventBus::new(256);
    let mut rx = bus.subscribe();
    let player = Player::new(bus);
    player.set_output(sink).await;

    // 2. 两个 WAV 文件 + 队列。
    let tmp = std::env::temp_dir();
    let wav1 = tmp.join("jimmusic_player_a.wav");
    let wav2 = tmp.join("jimmusic_player_b.wav");
    write_wav(&wav1);
    write_wav(&wav2);
    player
        .set_queue(vec![
            make_track(&wav1.to_string_lossy(), "a"),
            make_track(&wav2.to_string_lossy(), "b"),
        ])
        .await;

    // 3. 播放第一首，等待同一输出会话内的 gapless 边界与最终 Completed。
    player.play_track(0).await;

    // 事件序列：Played(a) -> Progress* -> TrackTransitioned(a,b)
    // -> Played(b) -> Progress* -> Completed(b)。
    let mut saw_played_a = false;
    let mut saw_completed = false;
    let mut saw_played_b = false;

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline && !(saw_played_a && saw_completed && saw_played_b) {
        let ev = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await;
        let Ok(Ok(ev)) = ev else { break };
        match ev {
            Event::Played { track_id } => {
                if track_id == wav1.to_string_lossy() {
                    saw_played_a = true;
                } else if track_id == wav2.to_string_lossy() {
                    saw_played_b = true;
                }
            }
            Event::Completed { .. } => saw_completed = true,
            _ => {}
        }
    }

    assert!(saw_played_a, "should receive Played for track a");
    assert!(
        saw_completed,
        "should receive Completed when track a finishes"
    );
    assert!(saw_played_b, "should auto-advance to Played for track b");
    assert_eq!(player.current_track().await.unwrap().title, "b");

    // 4. 停止（阻止循环播放）。
    player.stop().await;

    let _ = std::fs::remove_file(&wav1);
    let _ = std::fs::remove_file(&wav2);
}
