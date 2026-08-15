//! JimMusic 参考音频输出插件（`null` 后端）。
//!
//! 该插件是需求 3.3「音频输出插件」的参考实现：它不驱动真实音频设备，而是
//! 通过一个**有界环形缓冲 + 后台消费线程**模拟「设备消费」，从而在无音频硬件的
//! 环境（CI、容器、单元测试）下完整演示输出 ABI 的语义：
//!
//! - `open` 返回不透明句柄，支持多实例（每实例独立缓冲/状态/音量）；
//! - `write` 以推模型写入交错 i16 PCM，缓冲满时返回 `0`（背压）；
//! - `play` 启动后台消费（丢弃样本模拟出声），`pause` 停止消费（缓冲累积 → 背压），
//!   `stop` 停止并清空，`flush` 立即清空；
//! - `buffered_frames` 报告当前缓冲帧数；
//! - `capabilities` 返回 JSON 能力描述（后端/平台/采样率/声道/格式/特性）。
//!
//! 真实后端（ALSA / PipeWire / WASAPI / CoreAudio / AAudio / AudioUnit）只需以
//! 相同符号实现设备驱动的 `write`/`play`/`pause`/`stop`，即可被 Core 无缝替换。

// 导出的 C ABI 符号由 Core 经 libloading 加载调用，安全性由 ABI 契约保证，
// 统一豁免 missing_safety_doc。
#![allow(clippy::missing_safety_doc)]

use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use plugin_abi::output::{OutputHandle, OutputOpenParams, OutputOpenResult, PcmFormat};
use plugin_abi::{ErrorCode, PluginInfo, PluginKind};

// ---------------------------------------------------------------------------
// 标准插件符号（Core 加载时校验 ABI 与类型）。
// ---------------------------------------------------------------------------

static INFO: PluginInfo = PluginInfo::from_static(
    b"null-output\0",
    b"0.1.0\0",
    b"JimMusic Team\0",
    PluginKind::AudioOutput,
);

/// 导出：ABI 版本。
#[no_mangle]
pub unsafe extern "C" fn jimmusic_abi_version() -> u32 {
    plugin_abi::ABI_VERSION
}

/// 导出：静态元数据。
#[no_mangle]
pub unsafe extern "C" fn jimmusic_plugin_info() -> *const PluginInfo {
    &INFO
}

/// 导出：初始化（无状态）。
#[no_mangle]
pub unsafe extern "C" fn jimmusic_plugin_init(_ctx: *mut plugin_abi::HostCtx) -> ErrorCode {
    ErrorCode::Ok
}

/// 导出：清理。
#[no_mangle]
pub unsafe extern "C" fn jimmusic_plugin_shutdown() {}

/// 导出：统一调用入口（输出插件无需 op 派发，保留兼容占位）。
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
/// 后台消费线程的轮询间隔。
const DRAIN_TICK: Duration = Duration::from_millis(1);
static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

/// 单个输出流实例的内部状态。
struct NullOutput {
    /// 交错 PCM 样本缓冲（有界）。
    buffer: Mutex<VecDeque<i16>>,
    /// 缓冲上限（采样帧数）。
    max_frames: usize,
    /// 声道数（用于换算样本 ↔ 帧）。
    channels: usize,
    /// 音量（0.0 ~ 1.0，f32 位模式）。
    volume: AtomicU32,
    /// 是否正在播放（控制后台消费）。
    playing: AtomicBool,
    /// 停止标志（终止后台线程）。
    stop_flag: Arc<AtomicBool>,
    /// 仅能在成功 open 后生成的会话证据。
    session_info_json: String,
}

impl NullOutput {
    fn new(params: &OutputOpenParams) -> Self {
        let channels = params.channels.max(1) as usize;
        let max_frames = if params.buffer_frames == 0 {
            DEFAULT_BUFFER_FRAMES
        } else {
            params.buffer_frames as usize
        };
        Self {
            buffer: Mutex::new(VecDeque::with_capacity(max_frames * channels)),
            max_frames,
            channels,
            volume: AtomicU32::new(1.0f32.to_bits()),
            playing: AtomicBool::new(false),
            stop_flag: Arc::new(AtomicBool::new(false)),
            session_info_json: serde_json::json!({
                "schema_version": 1,
                "session_id": format!(
                    "null-{}",
                    NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed)
                ),
                "device_id": "null://discard",
                "device_name": "JimMusic Null Output",
                "driver": "jimmusic-null",
                "share_mode": "virtual",
                "exclusive": false,
                "requested_format": session_format(params),
                "negotiated_format": session_format(params),
                "software_buffer_frames": max_frames,
                "device_buffer_frames": serde_json::Value::Null,
                "clock_source": "monotonic_drain_timer",
                "capability_source": "opened_null_session"
            })
            .to_string(),
        }
    }

    /// 缓冲中当前采样帧数。
    fn buffered_frames(&self) -> u32 {
        let buf = self.buffer.lock().unwrap();
        (buf.len() / self.channels) as u32
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

/// 后台消费线程：播放中时按 tick 丢弃缓冲内的样本（模拟出声）。
fn spawn_drain(handle: OutputHandle, stop_flag: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        loop {
            if stop_flag.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(DRAIN_TICK);
            // SAFETY: 句柄在 close 前有效，且 drain 线程在 close 前被 stop_flag 终止。
            let out = unsafe { deref_handle(handle) };
            if out.playing.load(Ordering::SeqCst) {
                let mut buf = out.buffer.lock().unwrap();
                buf.clear(); // null 后端：丢弃样本即“播放”。
            }
        }
    });
}

/// 从原始句柄读取内部状态引用。
///
/// # Safety
/// 调用方必须保证 `handle` 为 `jimmusic_output_open` 返回的有效句柄。
unsafe fn deref_handle(handle: OutputHandle) -> &'static NullOutput {
    unsafe { &*(handle.0 as *const NullOutput) }
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
    // 当前仅支持 i16 交错格式。
    if params.format != PcmFormat::I16Interleaved as i32 {
        return OutputOpenResult {
            handle: OutputHandle::null(),
            code: ErrorCode::Unsupported.as_i32(),
        };
    }

    let out = NullOutput::new(params);
    let stop_flag = out.stop_flag.clone();
    let boxed = Box::new(out);
    let handle = OutputHandle(Box::into_raw(boxed) as *mut c_void);

    // 启动后台消费线程。
    spawn_drain(handle, stop_flag);

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
    // 短暂等待后台线程退出（避免 use-after-free）。
    std::thread::sleep(DRAIN_TICK * 2);
    // SAFETY: 由 Box::into_raw 创建，此处唯一回收。
    unsafe { drop(Box::from_raw(handle.0 as *mut NullOutput)) };
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

    let mut buf = out.buffer.lock().unwrap();
    let max_samples = out.max_frames.saturating_mul(ch);
    let space = max_samples.saturating_sub(buf.len());
    let accept = space.min(incoming.len());
    buf.extend(incoming[..accept].iter().copied());
    (accept / ch) as i32
}

/// 导出：开始播放（启动后台消费）。
#[no_mangle]
pub unsafe extern "C" fn jimmusic_output_play(handle: OutputHandle) -> i32 {
    if handle.is_null() {
        return ErrorCode::InvalidArgument.as_i32();
    }
    // SAFETY: handle 有效。
    unsafe { deref_handle(handle) }
        .playing
        .store(true, Ordering::SeqCst);
    ErrorCode::Ok.as_i32()
}

/// 导出：暂停（停止后台消费，缓冲累积）。
#[no_mangle]
pub unsafe extern "C" fn jimmusic_output_pause(handle: OutputHandle) -> i32 {
    if handle.is_null() {
        return ErrorCode::InvalidArgument.as_i32();
    }
    // SAFETY: handle 有效。
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
    // SAFETY: handle 有效。
    let out = unsafe { deref_handle(handle) };
    out.playing.store(false, Ordering::SeqCst);
    out.buffer.lock().unwrap().clear();
    ErrorCode::Ok.as_i32()
}

/// 导出：冲刷缓冲（立即清空）。
#[no_mangle]
pub unsafe extern "C" fn jimmusic_output_flush(handle: OutputHandle) -> i32 {
    if handle.is_null() {
        return ErrorCode::InvalidArgument.as_i32();
    }
    // SAFETY: handle 有效。
    unsafe { deref_handle(handle) }
        .buffer
        .lock()
        .unwrap()
        .clear();
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
    // SAFETY: handle 有效。
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
    // SAFETY: handle 有效。
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

/// 导出：查询已打开 null 会话的格式、缓冲和时钟来源。
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

/// 构造能力描述 JSON（与 core 的 `OutputCapabilities` 反序列化约定一致）。
fn capabilities_json() -> String {
    serde_json::json!({
        "backend": "null",
        "platform": "any",
        "sample_rates": [44_100, 48_000, 96_000],
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

    /// 打开一个 2 声道、48kHz、128 帧缓冲的输出流。
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

            // 写 200 帧（400 样本），仅 128 帧被接受。
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
            // 音量越界被拒绝。
            assert_eq!(
                jimmusic_output_set_volume(res.handle, 1.5),
                ErrorCode::InvalidArgument.as_i32()
            );
            // 合法音量设置成功。
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
        assert_eq!(v["backend"], "null");
        assert_eq!(v["platform"], "any");
        assert!(v["features"]["low_latency"] == true);
    }

    #[test]
    fn opened_session_reports_negotiated_evidence() {
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
            assert_eq!(value["negotiated_format"]["sample_rate"], 48_000);
            assert_eq!(value["negotiated_format"]["channels"], 2);
            assert_eq!(value["software_buffer_frames"], 256);
            assert_eq!(value["capability_source"], "opened_null_session");
            assert_ne!(value["session_id"], "");
            jimmusic_output_close(result.handle);
        }
    }

    #[test]
    fn multiple_instances_are_independent() {
        unsafe {
            let a = open(64);
            let b = open(64);
            let pcm = vec![0i16; 128]; // 64 帧 × 2 声道
                                       // 各写满各自的 64 帧缓冲。
            assert_eq!(jimmusic_output_write(a.handle, pcm.as_ptr(), 64), 64);
            assert_eq!(jimmusic_output_write(b.handle, pcm.as_ptr(), 64), 64);
            assert_eq!(jimmusic_output_buffered_frames(a.handle), 64);
            assert_eq!(jimmusic_output_buffered_frames(b.handle), 64);
            // 清空 a 不影响 b。
            jimmusic_output_flush(a.handle);
            assert_eq!(jimmusic_output_buffered_frames(a.handle), 0);
            assert_eq!(jimmusic_output_buffered_frames(b.handle), 64);
            jimmusic_output_close(a.handle);
            jimmusic_output_close(b.handle);
        }
    }
}
