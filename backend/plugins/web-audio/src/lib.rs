//! JimMusic Web 音频输出插件（`PluginKind::AudioOutput`，需求 3.3）。
//!
//! 浏览器无法由 Rust 直接驱动扬声器，因此该输出插件以 **无锁 SPSC 环形缓冲** 作为 PCM
//! 数据通路：Rust 侧（解码器）作为生产者 `write`。原生平台由后台 drain 线程模拟
//! AudioWorklet 消费（供无浏览器环境测试/验证输出 ABI）。
//!
//! wasm32 平台的真实扬声器输出经 **AudioWorklet**（见前端 `web/audio_worklet.js`）与
//! `SharedArrayBuffer` 桥（见 [`bindings`]）完成：`bindings::WebAudioRing` 把
//! `SharedArrayBuffer` 映射为与 [`RingBuffer`] 相同的 `[head][tail][i16 data]` 布局，
//! 供 Rust 侧写入、AudioWorklet 侧原子读取。运行时 SAB 的创建/绑定与 AudioWorkletNode
//! 连接由页面 JS 胶水负责（见 `audio_worklet.js` 头部注释），需 wasm32 工具链编译。
//!
//! 本 crate 完整实现需求 3.3「统一输出 ABI」（句柄化 + 推模型）：
//! - 标准插件符号：`jimmusic_abi_version` / `jimmusic_plugin_info` / `jimmusic_plugin_init` /
//!   `jimmusic_plugin_shutdown` / `jimmusic_plugin_invoke`；
//! - 输出符号：`jimmusic_output_open/close/write/play/pause/stop/flush/set_volume/
//!   buffered_frames/capabilities`。

#![allow(clippy::missing_safety_doc)]

pub mod ring;

#[cfg(target_arch = "wasm32")]
pub mod bindings;

pub use ring::RingBuffer;

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

use plugin_abi::output::{OutputHandle, OutputOpenParams, OutputOpenResult, PcmFormat};
use plugin_abi::{ErrorCode, PluginInfo, PluginKind};

// ---------------------------------------------------------------------------
// 标准插件符号（Core 加载时校验 ABI 与类型）。
// ---------------------------------------------------------------------------

static INFO: PluginInfo = PluginInfo::from_static(
    b"web-audio\0",
    b"0.1.0\0",
    b"JimMusic Team\0",
    PluginKind::AudioOutput,
);

#[no_mangle]
pub unsafe extern "C" fn jimmusic_abi_version() -> u32 {
    plugin_abi::ABI_VERSION
}

#[no_mangle]
pub unsafe extern "C" fn jimmusic_plugin_info() -> *const PluginInfo {
    &INFO
}

#[no_mangle]
pub unsafe extern "C" fn jimmusic_plugin_init(_ctx: *mut plugin_abi::HostCtx) -> ErrorCode {
    ErrorCode::Ok
}

#[no_mangle]
pub unsafe extern "C" fn jimmusic_plugin_shutdown() {}

#[no_mangle]
pub unsafe extern "C" fn jimmusic_plugin_invoke(
    _request: *const plugin_abi::InvokeRequest,
    _response: *mut plugin_abi::InvokeResponse,
) -> ErrorCode {
    ErrorCode::Unsupported
}

// ---------------------------------------------------------------------------
// 输出 ABI（需求 3.3）。
// ---------------------------------------------------------------------------

/// 默认缓冲帧数（当 open 参数 `buffer_frames == 0` 时）。
const DEFAULT_BUFFER_FRAMES: usize = 4096;
/// 后台消费线程轮询间隔（原生平台模拟 AudioWorklet 的 render quantum）。
#[cfg(not(target_arch = "wasm32"))]
const DRAIN_TICK: Duration = Duration::from_millis(1);
static NEXT_SESSION_ID: AtomicU32 = AtomicU32::new(1);

/// 单个输出流实例的内部状态。
struct WebAudioOutput {
    /// 无锁 SPSC 环形缓冲（i16 交错样本）。
    ring: RingBuffer,
    /// 声道数（样本 ↔ 帧换算）。
    channels: usize,
    /// 音量（0.0 ~ 1.0，f32 位模式）。
    volume: AtomicU32,
    /// 是否正在播放（控制后台消费）。
    playing: AtomicBool,
    /// 停止标志（终止后台线程）。
    stop_flag: Arc<AtomicBool>,
    session_info_json: String,
}

impl WebAudioOutput {
    fn new(params: &OutputOpenParams) -> Self {
        let channels = params.channels.max(1) as usize;
        let frames = if params.buffer_frames == 0 {
            DEFAULT_BUFFER_FRAMES
        } else {
            params.buffer_frames as usize
        };
        // RingBuffer 以样本数分配（内部取 2 的幂）。
        let capacity_samples = frames.saturating_mul(channels).max(2);
        Self {
            ring: RingBuffer::new(capacity_samples),
            channels,
            volume: AtomicU32::new(1.0f32.to_bits()),
            playing: AtomicBool::new(false),
            stop_flag: Arc::new(AtomicBool::new(false)),
            session_info_json: serde_json::json!({
                "schema_version": 1,
                "session_id": format!(
                    "web-audio-{}",
                    NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed)
                ),
                "device_id": "browser://default-output",
                "device_name": "Browser default output",
                "driver": "Web Audio AudioWorklet",
                "share_mode": "browser_managed",
                "exclusive": false,
                "requested_format": session_format(params),
                "negotiated_format": session_format(params),
                "software_buffer_frames": frames,
                "device_buffer_frames": 128,
                "clock_source": "audio_context_current_time",
                "capability_source": "opened_web_audio_session"
            })
            .to_string(),
        }
    }

    fn buffered_frames(&self) -> u32 {
        (self.ring.available_read() / self.channels.max(1)) as u32
    }
}

fn session_format(params: &OutputOpenParams) -> serde_json::Value {
    serde_json::json!({
        "sample_rate": params.sample_rate,
        "channels": params.channels.max(1),
        "sample_format": "i16",
        "bit_depth": 16,
        "packing": "interleaved"
    })
}

/// 后台消费线程：播放中定期读走样本（模拟 AudioWorklet 消费，仅原生平台）。
#[cfg(not(target_arch = "wasm32"))]
fn spawn_drain(handle: OutputHandle, stop_flag: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let mut scratch = vec![0i16; 512];
        loop {
            if stop_flag.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(DRAIN_TICK);
            // SAFETY: 句柄在 close 前有效，drain 线程在 close 前被 stop_flag 终止。
            let out = unsafe { deref_handle(handle) };
            if out.playing.load(Ordering::SeqCst) {
                let _ = out.ring.read(&mut scratch);
            }
        }
    });
}

/// 从原始句柄读取内部状态引用。
///
/// # Safety
/// 调用方必须保证 `handle` 为 `jimmusic_output_open` 返回的有效句柄。
unsafe fn deref_handle(handle: OutputHandle) -> &'static WebAudioOutput {
    unsafe { &*(handle.0 as *const WebAudioOutput) }
}

/// 导出：创建输出流，返回不透明句柄（支持多实例）。
#[no_mangle]
pub unsafe extern "C" fn jimmusic_output_open(params: *const OutputOpenParams) -> OutputOpenResult {
    if params.is_null() {
        return OutputOpenResult {
            handle: OutputHandle::null(),
            code: ErrorCode::InvalidArgument.as_i32(),
        };
    }
    // SAFETY: params 非空且指向有效 OutputOpenParams。
    let params = unsafe { &*params };
    if params.format != PcmFormat::I16Interleaved as i32 {
        return OutputOpenResult {
            handle: OutputHandle::null(),
            code: ErrorCode::Unsupported.as_i32(),
        };
    }

    let out = WebAudioOutput::new(params);
    let boxed = Box::new(out);
    let handle = OutputHandle(Box::into_raw(boxed) as *mut c_void);

    // 原生平台：启动后台消费线程（模拟 AudioWorklet）。wasm32 由 AudioWorklet 消费。
    #[cfg(not(target_arch = "wasm32"))]
    {
        // SAFETY: handle 刚由 Box::into_raw 创建，有效。
        let stop_flag = unsafe { deref_handle(handle) }.stop_flag.clone();
        spawn_drain(handle, stop_flag);
    }

    OutputOpenResult {
        handle,
        code: ErrorCode::Ok.as_i32(),
    }
}

/// 导出：关闭输出流并释放句柄。
#[no_mangle]
pub unsafe extern "C" fn jimmusic_output_close(handle: OutputHandle) {
    if handle.is_null() {
        return;
    }
    // SAFETY: handle 有效；先置停止标志终止后台线程，再回收内存。
    let out = unsafe { deref_handle(handle) };
    out.stop_flag.store(true, Ordering::SeqCst);
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::sleep(DRAIN_TICK * 2);
    // SAFETY: 由 Box::into_raw 创建，此处唯一回收。
    unsafe { drop(Box::from_raw(handle.0 as *mut WebAudioOutput)) };
}

/// 导出：推模型写入交错 PCM。返回实际入队采样帧数（缓冲满时返回 `0` = 背压）。
#[no_mangle]
pub unsafe extern "C" fn jimmusic_output_write(
    handle: OutputHandle,
    pcm: *const i16,
    frames: u32,
) -> i32 {
    if handle.is_null() || pcm.is_null() {
        return ErrorCode::InvalidArgument.as_i32();
    }
    // SAFETY: handle 有效。
    let out = unsafe { deref_handle(handle) };
    let ch = out.channels;
    let samples_to_write = (frames as usize).saturating_mul(ch);
    // SAFETY: pcm 指向 frames*channels 个 i16。
    let incoming = unsafe { std::slice::from_raw_parts(pcm, samples_to_write) };
    let written_samples = out.ring.write(incoming);
    (written_samples / ch) as i32
}

/// 导出：开始播放（启动消费）。
#[no_mangle]
pub unsafe extern "C" fn jimmusic_output_play(handle: OutputHandle) -> i32 {
    if handle.is_null() {
        return ErrorCode::InvalidArgument.as_i32();
    }
    unsafe { deref_handle(handle) }
        .playing
        .store(true, Ordering::SeqCst);
    ErrorCode::Ok.as_i32()
}

/// 导出：暂停（停止消费，缓冲累积 → 背压）。
#[no_mangle]
pub unsafe extern "C" fn jimmusic_output_pause(handle: OutputHandle) -> i32 {
    if handle.is_null() {
        return ErrorCode::InvalidArgument.as_i32();
    }
    unsafe { deref_handle(handle) }
        .playing
        .store(false, Ordering::SeqCst);
    ErrorCode::Ok.as_i32()
}

/// 导出：停止（停止消费并清空缓冲）。
#[no_mangle]
pub unsafe extern "C" fn jimmusic_output_stop(handle: OutputHandle) -> i32 {
    if handle.is_null() {
        return ErrorCode::InvalidArgument.as_i32();
    }
    let out = unsafe { deref_handle(handle) };
    out.playing.store(false, Ordering::SeqCst);
    out.ring.clear();
    ErrorCode::Ok.as_i32()
}

/// 导出：冲刷缓冲（立即清空）。
#[no_mangle]
pub unsafe extern "C" fn jimmusic_output_flush(handle: OutputHandle) -> i32 {
    if handle.is_null() {
        return ErrorCode::InvalidArgument.as_i32();
    }
    unsafe { deref_handle(handle) }.ring.clear();
    ErrorCode::Ok.as_i32()
}

/// 导出：设置音量（0.0 ~ 1.0）。
#[no_mangle]
pub unsafe extern "C" fn jimmusic_output_set_volume(handle: OutputHandle, volume: f32) -> i32 {
    if handle.is_null() {
        return ErrorCode::InvalidArgument.as_i32();
    }
    if !(0.0..=1.0).contains(&volume) {
        return ErrorCode::InvalidArgument.as_i32();
    }
    unsafe { deref_handle(handle) }
        .volume
        .store(volume.to_bits(), Ordering::SeqCst);
    ErrorCode::Ok.as_i32()
}

/// 导出：查询当前缓冲帧数（背压依据）。
#[no_mangle]
pub unsafe extern "C" fn jimmusic_output_buffered_frames(handle: OutputHandle) -> u32 {
    if handle.is_null() {
        return 0;
    }
    unsafe { deref_handle(handle) }.buffered_frames()
}

/// 导出：返回能力描述 JSON（NUL 结尾），返回写入字节数（不含 NUL）或负错误码。
#[no_mangle]
pub unsafe extern "C" fn jimmusic_output_capabilities(out: *mut u8, capacity: u32) -> i32 {
    if out.is_null() || capacity == 0 {
        return ErrorCode::InvalidArgument.as_i32();
    }
    let json = capabilities_json();
    let bytes = json.as_bytes();
    if bytes.len() + 1 > capacity as usize {
        return ErrorCode::InvalidArgument.as_i32();
    }
    // SAFETY: out 指向至少 capacity 字节可写缓冲。
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len());
        *out.add(bytes.len()) = 0;
    }
    bytes.len() as i32
}

#[no_mangle]
pub unsafe extern "C" fn jimmusic_output_session_info(
    handle: OutputHandle,
    out: *mut u8,
    capacity: u32,
) -> i32 {
    if handle.is_null() || out.is_null() || capacity == 0 {
        return ErrorCode::InvalidArgument.as_i32();
    }
    let json = &unsafe { deref_handle(handle) }.session_info_json;
    let bytes = json.as_bytes();
    if bytes.len() + 1 > capacity as usize {
        return ErrorCode::InvalidArgument.as_i32();
    }
    // SAFETY: 调用方提供了足够大的可写缓冲。
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len());
        *out.add(bytes.len()) = 0;
    }
    bytes.len() as i32
}

fn capabilities_json() -> String {
    serde_json::json!({
        "backend": "web-audio",
        "platform": "web",
        "sample_rates": [44_100, 48_000],
        "channels": [1, 2],
        "formats": ["i16"],
        "features": {
            "exclusive": false,
            "hardware_volume": false,
            "low_latency": true,
        },
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe fn open(buffer_frames: u32) -> OutputOpenResult {
        let params = OutputOpenParams {
            sample_rate: 48_000,
            channels: 2,
            format: PcmFormat::I16Interleaved as i32,
            buffer_frames,
        };
        unsafe { jimmusic_output_open(&params) }
    }

    #[test]
    fn open_rejects_null_and_bad_format() {
        unsafe {
            assert_eq!(
                jimmusic_output_open(std::ptr::null()).code,
                ErrorCode::InvalidArgument.as_i32()
            );
        }
        let params = OutputOpenParams {
            sample_rate: 48_000,
            channels: 2,
            format: PcmFormat::F32Interleaved as i32,
            buffer_frames: 0,
        };
        unsafe {
            assert_eq!(
                jimmusic_output_open(&params).code,
                ErrorCode::Unsupported.as_i32()
            );
        }
    }

    #[test]
    fn write_applies_backpressure_and_flush_releases() {
        unsafe {
            let res = open(128);
            assert_eq!(res.code, ErrorCode::Ok.as_i32());
            assert!(!res.handle.is_null());

            // 写 200 帧（400 样本），仅 128 帧被接受（环形缓冲背压）。
            let pcm = vec![0i16; 400];
            let accepted = jimmusic_output_write(res.handle, pcm.as_ptr(), 200);
            assert_eq!(accepted, 128);
            assert_eq!(jimmusic_output_buffered_frames(res.handle), 128);

            // 缓冲已满 → 背压（0 帧入队）。
            assert_eq!(jimmusic_output_write(res.handle, pcm.as_ptr(), 10), 0);

            // flush 清空后可继续写入。
            assert_eq!(jimmusic_output_flush(res.handle), ErrorCode::Ok.as_i32());
            assert_eq!(jimmusic_output_buffered_frames(res.handle), 0);
            assert_eq!(jimmusic_output_write(res.handle, pcm.as_ptr(), 64), 64);

            jimmusic_output_close(res.handle);
        }
    }

    #[test]
    fn play_pause_stop_and_volume() {
        unsafe {
            let res = open(1024);
            assert_eq!(
                jimmusic_output_set_volume(res.handle, 1.5),
                ErrorCode::InvalidArgument.as_i32()
            );
            assert_eq!(
                jimmusic_output_set_volume(res.handle, 0.5),
                ErrorCode::Ok.as_i32()
            );
            assert_eq!(jimmusic_output_play(res.handle), ErrorCode::Ok.as_i32());
            assert_eq!(jimmusic_output_pause(res.handle), ErrorCode::Ok.as_i32());
            assert_eq!(jimmusic_output_stop(res.handle), ErrorCode::Ok.as_i32());
            jimmusic_output_close(res.handle);
        }
    }

    #[test]
    fn capabilities_are_valid_json() {
        let mut buf = vec![0u8; 1024];
        let written = unsafe { jimmusic_output_capabilities(buf.as_mut_ptr(), buf.len() as u32) };
        assert!(written > 0);
        let json = String::from_utf8(buf[..written as usize].to_vec()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["backend"], "web-audio");
        assert_eq!(v["platform"], "web");
        assert_eq!(v["features"]["low_latency"], true);
    }

    #[test]
    fn opened_session_reports_audio_worklet_contract() {
        unsafe {
            let result = open(256);
            let mut buffer = vec![0u8; 2048];
            let written = jimmusic_output_session_info(
                result.handle,
                buffer.as_mut_ptr(),
                buffer.len() as u32,
            );
            assert!(written > 0);
            let value: serde_json::Value =
                serde_json::from_slice(&buffer[..written as usize]).unwrap();
            assert_eq!(value["driver"], "Web Audio AudioWorklet");
            assert_eq!(value["negotiated_format"]["sample_rate"], 48_000);
            assert_eq!(value["device_buffer_frames"], 128);
            assert_eq!(value["capability_source"], "opened_web_audio_session");
            jimmusic_output_close(result.handle);
        }
    }

    #[test]
    fn multiple_instances_are_independent() {
        unsafe {
            let a = open(64);
            let b = open(64);
            let pcm = vec![0i16; 128]; // 64 帧 × 2 声道
            assert_eq!(jimmusic_output_write(a.handle, pcm.as_ptr(), 64), 64);
            assert_eq!(jimmusic_output_write(b.handle, pcm.as_ptr(), 64), 64);
            assert_eq!(jimmusic_output_buffered_frames(a.handle), 64);
            assert_eq!(jimmusic_output_buffered_frames(b.handle), 64);
            jimmusic_output_flush(a.handle);
            assert_eq!(jimmusic_output_buffered_frames(a.handle), 0);
            assert_eq!(jimmusic_output_buffered_frames(b.handle), 64);
            jimmusic_output_close(a.handle);
            jimmusic_output_close(b.handle);
        }
    }
}
