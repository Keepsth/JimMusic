//! 播放器宿主桥（C ABI）：把 [`crate::Player`]（统一播放引擎）封装为可供前端
//! （Flutter 经 dart:ffi，或其它 FFI 语言）调用的 `extern "C"` 符号。
//!
//! 这是「打通前后端桥」的核心：前端不再直接解码/出声，而是经本桥下发播放指令
//! （set_queue / play_track / next / previous / pause / resume / stop / seek /
//! set_output），由 Core 的统一播放引擎（队列 + 自动切歌 + 输出插件）驱动真正出声；
//! 播放状态与进度通过可注册的 C 回调回传前端。
//!
//! 符号约定（`#[no_mangle] extern "C"`）：
//!
//! | 符号                          | 语义                                                     |
//! |-------------------------------|----------------------------------------------------------|
//! | `jimmusic_host_set_event_callback` | 注册事件回调 `fn(event_type, value)`              |
//! | `jimmusic_host_set_output`    | 加载音频输出插件并设为当前输出（路径为动态库文件）      |
//! | `jimmusic_host_output_session` | 读取已打开设备会话的协商证据 JSON                 |
//! | `jimmusic_host_set_queue`     | 设置播放队列（JSON 路径数组）                            |
//! | `jimmusic_host_play_track`    | 播放队列第 index 首                                      |
//! | `jimmusic_host_play_file`     | 便捷：设置单曲队列并播放（track_id + 路径）              |
//! | `jimmusic_host_next/previous` | 切上一首/下一首（循环）                                  |
//! | `jimmusic_host_current_index` | 当前曲目索引                                             |
//! | `jimmusic_host_set_crossfade` | 设为 gapless(0 ms) 或双时间线 crossfade                  |
//! | `jimmusic_host_pause/resume/stop/seek` | 播放控制                                     |
//! | `jimmusic_host_position`      | 当前播放位置（秒）                                       |
//! | `jimmusic_host_duration`      | 当前曲目时长（秒）                                       |
//! | `jimmusic_host_state`         | 当前状态：0=停止 / 1=播放 / 2=暂停                       |
//!
//! 事件回调约定（`event_type`）：
//! - `0`（停止）/ `1`（播放）/ `2`（暂停）/ `3`（进度）；
//! - `value`：进度事件时为播放比例（0.0~1.0），其余事件为 0.0。
//!
//! 所有指令函数立即返回 [`plugin_abi::ErrorCode`] 判别值（`Ok=0`）；耗时播放（解码）
//! 在后台 Tokio 任务中执行，不阻塞调用线程。查询函数（`position`/`state`）同步返回。
//!
//! 该模块通过全局单例维护引擎与运行时，跨多次 FFI 调用共享同一播放会话。

use std::ffi::{c_char, CStr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use tokio::runtime::Runtime;

use crate::engine::{FfiSink, PcmSink};
use crate::event::{Event, EventBus};
use crate::media::Track;
use crate::output::{OutputError, OutputPlugin};
use crate::p2p_node::{load_or_create_identity, EmbeddedIpfsNode, EmbeddedNodeConfig};
use crate::player::Player;
use plugin_abi::output::{OutputOpenParams, PcmFormat};
use plugin_abi::ErrorCode;

/// 事件类型常量（前端 dart:ffi 侧需保持同名同值）。
pub const EVENT_STOPPED: i32 = 0;
pub const EVENT_PLAYING: i32 = 1;
pub const EVENT_PAUSED: i32 = 2;
pub const EVENT_PROGRESS: i32 = 3;
pub const EVENT_ERROR: i32 = 4;

/// 事件回调签名：`(event_type, value)`，value 对进度为播放比例（0~1）。
pub type EventCallback = extern "C" fn(event_type: i32, value: f64);

/// 宿主全局状态：共享的统一播放引擎（Player）与事件总线。
struct HostState {
    player: Arc<Player>,
    #[allow(dead_code)]
    bus: EventBus,
}

static RUNTIME: OnceLock<Runtime> = OnceLock::new();
static HOST: OnceLock<HostState> = OnceLock::new();
static CALLBACK: Mutex<Option<EventCallback>> = Mutex::new(None);

/// 播放状态快照（供查询函数同步返回，避免 block_on）。
static SNAP_STATE: AtomicI32 = AtomicI32::new(EVENT_STOPPED);
/// 最近进度比例（f64 位模式）。
static SNAP_RATIO: AtomicU64 = AtomicU64::new(0);
/// 当前曲目时长（秒，f64 位模式）。
static SNAP_DURATION: AtomicU64 = AtomicU64::new(0);
/// 最近一次结构化播放错误（JSON；供 UI 主动读取）。
static LAST_ERROR: Mutex<String> = Mutex::new(String::new());
/// 当前已打开输出句柄的会话证据 JSON。
static OUTPUT_SESSION: Mutex<String> = Mutex::new(String::new());
/// App-embedded native IPFS node, shared by desktop and mobile FFI clients.
static EMBEDDED_NODE: Mutex<Option<Arc<EmbeddedIpfsNode>>> = Mutex::new(None);
/// 0=stopped, 1=foreground, 2=background best effort, 3=failed.
static NODE_LIFECYCLE: AtomicI32 = AtomicI32::new(0);
static NODE_LAST_ERROR: Mutex<String> = Mutex::new(String::new());
/// A two-call FFI read must return one coherent status snapshot.
static NODE_STATUS_CACHE: Mutex<String> = Mutex::new(String::new());

fn runtime() -> &'static Runtime {
    RUNTIME.get_or_init(|| Runtime::new().expect("create tokio runtime"))
}

fn host() -> &'static HostState {
    HOST.get_or_init(|| {
        let bus = EventBus::new(256);
        let player = Player::new(bus.clone());
        // 订阅事件总线，把播放事件转发给前端回调。
        let mut rx = bus.subscribe();
        let _task = runtime().spawn(async move {
            while let Ok(ev) = rx.recv().await {
                on_event(ev);
            }
        });
        HostState { player, bus }
    })
}

/// 将核心事件映射为回调参数并更新状态快照。
fn on_event(ev: Event) {
    let (ty, value) = match &ev {
        Event::Played { .. } => {
            SNAP_STATE.store(EVENT_PLAYING, Ordering::SeqCst);
            SNAP_RATIO.store(0.0f64.to_bits(), Ordering::SeqCst);
            (EVENT_PLAYING, 0.0)
        }
        Event::Paused { .. } => {
            SNAP_STATE.store(EVENT_PAUSED, Ordering::SeqCst);
            (EVENT_PAUSED, current_ratio())
        }
        Event::Stopped => {
            SNAP_STATE.store(EVENT_STOPPED, Ordering::SeqCst);
            SNAP_RATIO.store(0.0f64.to_bits(), Ordering::SeqCst);
            (EVENT_STOPPED, 0.0)
        }
        Event::Completed { .. } => {
            SNAP_STATE.store(EVENT_STOPPED, Ordering::SeqCst);
            SNAP_RATIO.store(0.0f64.to_bits(), Ordering::SeqCst);
            (EVENT_STOPPED, 0.0)
        }
        Event::TrackTransitioned { duration_secs, .. } => {
            SNAP_STATE.store(EVENT_PLAYING, Ordering::SeqCst);
            SNAP_RATIO.store(0.0f64.to_bits(), Ordering::SeqCst);
            SNAP_DURATION.store(duration_secs.to_bits(), Ordering::SeqCst);
            (EVENT_PLAYING, 0.0)
        }
        Event::Progress { position, .. } => {
            SNAP_RATIO.store(position.to_bits(), Ordering::SeqCst);
            // 进度回调统一以「秒」对外，前端无需自行换算时长。
            (EVENT_PROGRESS, *position * current_duration())
        }
        Event::PlaybackFailed { error, .. } => {
            SNAP_STATE.store(EVENT_STOPPED, Ordering::SeqCst);
            *LAST_ERROR.lock().expect("last error lock poisoned") = serde_json::json!({
                "source": error.source,
                "stage": error.stage,
                "code": error.code,
                "retryable": error.retryable,
                "suggestion": error.suggestion,
            })
            .to_string();
            (EVENT_ERROR, 0.0)
        }
        _ => return,
    };

    if let Some(cb) = *CALLBACK.lock().expect("callback lock poisoned") {
        cb(ty, value);
    }
}

fn current_ratio() -> f64 {
    f64::from_bits(SNAP_RATIO.load(Ordering::SeqCst))
}

fn current_duration() -> f64 {
    f64::from_bits(SNAP_DURATION.load(Ordering::SeqCst))
}

/// 将 `*const c_char` 安全转为 [`String`]（空指针返回空串）。
fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: 调用方保证 p 为 NUL 结尾字节串或空指针。
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

fn node_failure(error: impl ToString) -> i32 {
    *NODE_LAST_ERROR
        .lock()
        .expect("node last error lock poisoned") = error.to_string();
    NODE_LIFECYCLE.store(3, Ordering::SeqCst);
    ErrorCode::InvokeFailed.as_i32()
}

fn embedded_node_status_json() -> String {
    let node = EMBEDDED_NODE
        .lock()
        .expect("embedded node lock poisoned")
        .clone();
    let lifecycle = NODE_LIFECYCLE.load(Ordering::SeqCst);
    let lifecycle_state = match lifecycle {
        1 => "foreground",
        2 => "background_degraded",
        3 => "failed",
        _ => "stopped",
    };
    let last_error = NODE_LAST_ERROR
        .lock()
        .expect("node last error lock poisoned")
        .clone();
    let mut value = serde_json::json!({
        "schema_version": 1,
        "implementation": "rust-ipfs",
        "lifecycle_state": lifecycle_state,
        "peer_id": null,
        "listen_addresses": [],
        "peers": [],
        "connected_peers": 0,
        "transports": [],
        "routing_status": "stopped",
        "bytes_up": 0,
        "bytes_down": 0,
        "storage": "filesystem",
        "persists_after_app_close": false,
        "limitations": [
            "background networking is best effort and follows platform suspension rules",
            "the node stops when the application process closes"
        ],
        "last_error": if last_error.is_empty() { None } else { Some(last_error) },
    });
    if let Some(node) = node {
        match runtime().block_on(node.status()) {
            Ok(status) => {
                value["peer_id"] = serde_json::Value::String(status.peer_id);
                value["listen_addresses"] = serde_json::json!(status.listen_addresses);
                value["connected_peers"] = serde_json::json!(status.connected_peers.len());
                value["peers"] = serde_json::json!(status.connected_peers);
                value["transports"] = serde_json::json!(status.transports);
                value["routing_status"] = serde_json::Value::String(status.routing_status);
                value["bytes_up"] = serde_json::json!(status.bytes_up);
                value["bytes_down"] = serde_json::json!(status.bytes_down);
            }
            Err(error) => {
                value["lifecycle_state"] = serde_json::Value::String("failed".into());
                value["last_error"] = serde_json::Value::String(error.to_string());
            }
        }
    }
    value.to_string()
}

/// 将输出插件错误映射为跨 FFI 错误码判别值。
fn output_code(e: &OutputError) -> i32 {
    match e {
        OutputError::Load(..) => ErrorCode::LoadFailed.as_i32(),
        OutputError::SymbolNotFound(_) => ErrorCode::SymbolNotFound.as_i32(),
        OutputError::AbiMismatch { .. } => ErrorCode::AbiMismatch.as_i32(),
        _ => ErrorCode::InvokeFailed.as_i32(),
    }
}

/// 从动态库路径加载输出插件并打开一条输出流，包装为 [`PcmSink`]。
fn load_sink(path: &str) -> Result<Arc<dyn PcmSink>, i32> {
    let plugin = Arc::new(OutputPlugin::load(path).map_err(|e| output_code(&e))?);
    let caps = plugin.capabilities();
    let sample_rate = caps.sample_rates.first().copied().unwrap_or(44_100);
    let channels = caps.channels.first().copied().unwrap_or(2);
    let params = OutputOpenParams {
        sample_rate,
        channels,
        format: PcmFormat::I16Interleaved as i32,
        buffer_frames: 1024,
    };
    let stream = plugin.open(params).map_err(|e| output_code(&e))?;
    *OUTPUT_SESSION.lock().expect("output session lock poisoned") =
        serde_json::to_string(stream.session_info()).map_err(|_| ErrorCode::Unknown.as_i32())?;
    Ok(Arc::new(FfiSink::new(stream)))
}

/// 从文件路径构造一条 [`Track`]（标题取文件名，时长读元数据）。
fn track_from_path(path: &str) -> Track {
    let title = std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();
    let duration = symphonia_decoder::read_metadata(std::path::Path::new(path))
        .ok()
        .and_then(|m| m.duration);
    Track {
        path: path.to_string(),
        title,
        artist: None,
        album: None,
        duration,
        sample_rate: None,
        channels: None,
    }
}

// ---------------------------------------------------------------------------
// 导出符号（C ABI）。
// ---------------------------------------------------------------------------

/// 注册事件回调（传 `None`/空指针可取消）。
#[no_mangle]
pub extern "C" fn jimmusic_host_set_event_callback(cb: Option<EventCallback>) -> i32 {
    *CALLBACK.lock().expect("callback lock poisoned") = cb;
    ErrorCode::Ok.as_i32()
}

/// 加载并激活音频输出插件（`path` 为输出插件动态库路径）。
#[no_mangle]
pub extern "C" fn jimmusic_host_set_output(path: *const c_char) -> i32 {
    let path = cstr(path);
    if path.is_empty() {
        return ErrorCode::InvalidArgument.as_i32();
    }
    match load_sink(&path) {
        Ok(sink) => {
            let player = host().player.clone();
            runtime().block_on(async move { player.set_output(sink).await });
            ErrorCode::Ok.as_i32()
        }
        Err(code) => code,
    }
}

/// 设置播放队列（`paths_json` 为 JSON 字符串数组，如 `["/a.mp3","/b.mp3"]`）。
#[no_mangle]
pub extern "C" fn jimmusic_host_set_queue(paths_json: *const c_char) -> i32 {
    let json = cstr(paths_json);
    if json.is_empty() {
        return ErrorCode::InvalidArgument.as_i32();
    }
    let paths: Vec<String> = match serde_json::from_str(&json) {
        Ok(p) => p,
        Err(_) => return ErrorCode::InvalidArgument.as_i32(),
    };
    let tracks: Vec<Track> = paths.iter().map(|p| track_from_path(p)).collect();
    let player = host().player.clone();
    runtime().block_on(async move { player.set_queue(tracks).await });
    ErrorCode::Ok.as_i32()
}

/// 播放队列第 `index` 首（异步解码，立即返回）。
#[no_mangle]
pub extern "C" fn jimmusic_host_play_track(index: i32) -> i32 {
    if index < 0 {
        return ErrorCode::InvalidArgument.as_i32();
    }
    let player = host().player.clone();
    let _task = runtime().spawn(async move {
        player.play_track(index as usize).await;
        SNAP_DURATION.store(player.duration_secs().await.to_bits(), Ordering::SeqCst);
    });
    ErrorCode::Ok.as_i32()
}

/// Configures playlist transition. `duration_ms == 0` is gapless; positive
/// values enable crossfade. `curve`: 0=linear, 1=equal-power.
#[no_mangle]
pub extern "C" fn jimmusic_host_set_crossfade(duration_ms: u32, curve: i32) -> i32 {
    if curve != 0 && curve != 1 {
        return ErrorCode::InvalidArgument.as_i32();
    }
    let player = host().player.clone();
    runtime().block_on(async move {
        player
            .set_crossfade(duration_ms as f64 / 1_000.0, curve == 1)
            .await;
    });
    ErrorCode::Ok.as_i32()
}

/// 便捷：设置单曲队列并播放（向后兼容单曲播放）。
#[no_mangle]
pub extern "C" fn jimmusic_host_play_file(track_id: *const c_char, path: *const c_char) -> i32 {
    let _track_id = cstr(track_id);
    let path = cstr(path);
    if path.is_empty() {
        return ErrorCode::InvalidArgument.as_i32();
    }

    // 读取元数据时长，供 position（秒）换算。
    if let Ok(meta) = symphonia_decoder::read_metadata(std::path::Path::new(&path)) {
        if let Some(d) = meta.duration {
            SNAP_DURATION.store(d.to_bits(), Ordering::SeqCst);
        }
    }

    let player = host().player.clone();
    let track = track_from_path(&path);
    let _task = runtime().spawn(async move {
        player.set_queue(vec![track]).await;
        player.play_track(0).await;
    });
    ErrorCode::Ok.as_i32()
}

/// 下一首（循环）。
#[no_mangle]
pub extern "C" fn jimmusic_host_next() -> i32 {
    let player = host().player.clone();
    let _task = runtime().spawn(async move { player.next().await });
    ErrorCode::Ok.as_i32()
}

/// 上一首（循环）。
#[no_mangle]
pub extern "C" fn jimmusic_host_previous() -> i32 {
    let player = host().player.clone();
    let _task = runtime().spawn(async move { player.previous().await });
    ErrorCode::Ok.as_i32()
}

/// 当前曲目索引（队列为空时返回 0）。
#[no_mangle]
pub extern "C" fn jimmusic_host_current_index() -> i32 {
    let player = host().player.clone();
    runtime().block_on(async move { player.current_index().await }) as i32
}

/// 暂停。
#[no_mangle]
pub extern "C" fn jimmusic_host_pause() -> i32 {
    let player = host().player.clone();
    let _task = runtime().spawn(async move { player.pause().await });
    ErrorCode::Ok.as_i32()
}

/// 恢复播放。
#[no_mangle]
pub extern "C" fn jimmusic_host_resume() -> i32 {
    let player = host().player.clone();
    let _task = runtime().spawn(async move { player.resume().await });
    ErrorCode::Ok.as_i32()
}

/// 停止。
#[no_mangle]
pub extern "C" fn jimmusic_host_stop() -> i32 {
    let player = host().player.clone();
    let _task = runtime().spawn(async move { player.stop().await });
    ErrorCode::Ok.as_i32()
}

/// 跳转到指定位置（秒）。
#[no_mangle]
pub extern "C" fn jimmusic_host_seek(position_secs: f64) -> i32 {
    let player = host().player.clone();
    let _task = runtime().spawn(async move { player.seek(position_secs).await });
    ErrorCode::Ok.as_i32()
}

/// 当前播放位置（秒，基于最近进度比例 × 时长）。
#[no_mangle]
pub extern "C" fn jimmusic_host_position() -> f64 {
    current_ratio() * current_duration()
}

/// 当前曲目时长（秒）。
#[no_mangle]
pub extern "C" fn jimmusic_host_duration() -> f64 {
    current_duration()
}

/// 当前播放状态（0=停止 / 1=播放 / 2=暂停）。
#[no_mangle]
pub extern "C" fn jimmusic_host_state() -> i32 {
    SNAP_STATE.load(Ordering::SeqCst)
}

/// 读取最近一次结构化错误 JSON。返回所需字节数；容量足够时写入 UTF-8（不含 NUL）。
/// `buffer` 为空可用于只查询长度。
///
/// # Safety
/// 当 `buffer` 非空时，调用方必须保证它指向至少 `capacity` 个可写字节，并在调用期间
/// 保持有效。返回值大于 `capacity` 时函数不会写入。
#[no_mangle]
pub unsafe extern "C" fn jimmusic_host_last_error(buffer: *mut u8, capacity: usize) -> usize {
    let error = LAST_ERROR.lock().expect("last error lock poisoned");
    let bytes = error.as_bytes();
    if !buffer.is_null() && capacity >= bytes.len() {
        // SAFETY: 调用方承诺 buffer 至少有 capacity 字节；上方已核对写入长度。
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, bytes.len()) };
    }
    bytes.len()
}

/// 读取当前已打开输出会话的证据 JSON。返回所需字节数；容量足够时写入 UTF-8。
///
/// # Safety
/// 当 `buffer` 非空时，调用方必须保证它指向至少 `capacity` 个可写字节。
#[no_mangle]
pub unsafe extern "C" fn jimmusic_host_output_session(buffer: *mut u8, capacity: usize) -> usize {
    let session = OUTPUT_SESSION.lock().expect("output session lock poisoned");
    let bytes = session.as_bytes();
    if !buffer.is_null() && capacity >= bytes.len() {
        // SAFETY: 调用方承诺 buffer 至少有 capacity 字节，且上方已校验长度。
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, bytes.len()) };
    }
    bytes.len()
}

/// Starts the app-embedded native IPFS node in `root`. The node owns a stable
/// private identity at `root/node-key.pb` and a persistent repository below
/// `root/ipfs-repository`.
#[no_mangle]
pub extern "C" fn jimmusic_node_start(root: *const c_char) -> i32 {
    let root = cstr(root);
    if root.is_empty() {
        return ErrorCode::InvalidArgument.as_i32();
    }
    let mut node = EMBEDDED_NODE.lock().expect("embedded node lock poisoned");
    if node.is_some() {
        NODE_LIFECYCLE.store(1, Ordering::SeqCst);
        return ErrorCode::Ok.as_i32();
    }
    let root = PathBuf::from(root);
    let identity = match load_or_create_identity(&root.join("node-key.pb")) {
        Ok(identity) => identity,
        Err(error) => return node_failure(error),
    };
    let config = EmbeddedNodeConfig::native(root.join("ipfs-repository"), identity);
    match runtime().block_on(EmbeddedIpfsNode::start(config)) {
        Ok(started) => {
            *node = Some(Arc::new(started));
            NODE_LAST_ERROR
                .lock()
                .expect("node last error lock poisoned")
                .clear();
            NODE_LIFECYCLE.store(1, Ordering::SeqCst);
            ErrorCode::Ok.as_i32()
        }
        Err(error) => node_failure(error),
    }
}

/// Marks whether the application is in the foreground. Background operation is
/// deliberately best effort; mobile operating systems may suspend the process.
#[no_mangle]
pub extern "C" fn jimmusic_node_set_foreground(foreground: i32) -> i32 {
    if foreground != 0 && foreground != 1 {
        return ErrorCode::InvalidArgument.as_i32();
    }
    if EMBEDDED_NODE
        .lock()
        .expect("embedded node lock poisoned")
        .is_none()
    {
        return ErrorCode::NotInitialized.as_i32();
    }
    NODE_LIFECYCLE.store(if foreground == 1 { 1 } else { 2 }, Ordering::SeqCst);
    ErrorCode::Ok.as_i32()
}

/// Directly dials one libp2p multiaddress without using an HTTP gateway.
#[no_mangle]
pub extern "C" fn jimmusic_node_connect(address: *const c_char) -> i32 {
    let address = cstr(address);
    if address.is_empty() {
        return ErrorCode::InvalidArgument.as_i32();
    }
    let node = EMBEDDED_NODE
        .lock()
        .expect("embedded node lock poisoned")
        .clone();
    let Some(node) = node else {
        return ErrorCode::NotInitialized.as_i32();
    };
    match runtime().block_on(node.connect(&address)) {
        Ok(()) => ErrorCode::Ok.as_i32(),
        Err(error) => node_failure(error),
    }
}

/// Stops the app-embedded node. It is safe to call repeatedly during teardown.
#[no_mangle]
pub extern "C" fn jimmusic_node_stop() -> i32 {
    let node = EMBEDDED_NODE
        .lock()
        .expect("embedded node lock poisoned")
        .take();
    if let Some(node) = node {
        match Arc::try_unwrap(node) {
            Ok(node) => runtime().block_on(node.shutdown_owned()),
            Err(node) => runtime().block_on(node.shutdown()),
        }
    }
    NODE_LIFECYCLE.store(0, Ordering::SeqCst);
    ErrorCode::Ok.as_i32()
}

/// Reads one coherent JSON status snapshot. A null buffer refreshes the cache
/// and returns the required byte count; the following call copies that snapshot.
///
/// # Safety
/// When `buffer` is non-null it must point to at least `capacity` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn jimmusic_node_status(buffer: *mut u8, capacity: usize) -> usize {
    let mut cache = NODE_STATUS_CACHE
        .lock()
        .expect("node status cache lock poisoned");
    if buffer.is_null() || capacity == 0 || cache.is_empty() {
        *cache = embedded_node_status_json();
    }
    let bytes = cache.as_bytes();
    if !buffer.is_null() && capacity >= bytes.len() {
        // SAFETY: the caller contract and capacity check guarantee a valid destination.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, bytes.len()) };
    }
    bytes.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cstr_handles_null_and_valid() {
        assert_eq!(cstr(std::ptr::null()), "");
        let s = std::ffi::CString::new("hello").unwrap();
        assert_eq!(cstr(s.as_ptr()), "hello");
    }

    #[test]
    fn output_code_maps_variants() {
        assert_eq!(
            output_code(&OutputError::Load("p".into(), "e".into())),
            ErrorCode::LoadFailed.as_i32()
        );
        assert_eq!(
            output_code(&OutputError::SymbolNotFound("s".into())),
            ErrorCode::SymbolNotFound.as_i32()
        );
        assert_eq!(
            output_code(&OutputError::Operation(7)),
            ErrorCode::InvokeFailed.as_i32()
        );
    }

    #[test]
    fn load_sink_rejects_missing_path() {
        assert!(matches!(
            load_sink("/nonexistent/output.so"),
            Err(code) if code == ErrorCode::LoadFailed.as_i32()
        ));
    }
}
