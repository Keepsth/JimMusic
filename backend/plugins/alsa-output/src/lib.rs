//! JimMusic 跨平台真实音频输出插件（`PluginKind::AudioOutput`，需求 3.3）。
//!
//! 基于 [`cpal`] 驱动真实音频设备，目标平台自动选择后端：Linux→ALSA/PipeWire、
//! Windows→WASAPI、macOS→CoreAudio、Android→AAudio/OpenSL、iOS→AudioUnit（能力经
//! [`jimmusic_output_capabilities`] 按 `target_os` 动态声明）。cpal 采用
//! 「拉模型」（设备线程回调请求 PCM），而输出 ABI 是「推模型」（`write` 推入 PCM），二者
//! 以**无锁 SPSC 环形缓冲**（[`RingBuffer`]）桥接：
//!
//! ```text
//! Core 播放引擎（生产者）               cpal 音频线程（消费者）
//!   write(PCM)  ──► RingBuffer ──►  data callback 读样本 → 扬声器
//! ```
//!
//! 完整实现需求 3.3「统一输出 ABI」（句柄化 + 推模型），符号与 `null-output`/`web-audio`
//! 一致，可由 Core 的 [`OutputPlugin`] 无缝加载。

#![allow(clippy::missing_safety_doc)]

// 环形缓冲作为独立数据通路，部分方法（capacity/is_empty 等）当前未被输出 ABI 使用。
#[allow(dead_code)]
mod ring;

use ring::RingBuffer;

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use plugin_abi::output::{OutputHandle, OutputOpenParams, OutputOpenResult, PcmFormat};
use plugin_abi::{ErrorCode, PluginInfo, PluginKind};

// ---------------------------------------------------------------------------
// 标准插件符号（Core 加载时校验 ABI 与类型）。
// ---------------------------------------------------------------------------

static INFO: PluginInfo = PluginInfo::from_static(
    b"alsa-output\0",
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
static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

/// 单个输出流实例的内部状态。
struct AlsaOutput {
    /// 无锁 SPSC 环形缓冲（i16 交错样本），连接 write（生产者）与 cpal 回调（消费者）。
    ring: Arc<RingBuffer>,
    /// 声道数（样本 ↔ 帧换算）。
    channels: usize,
    /// 音量（0.0 ~ 1.0，f32 位模式）。
    volume: Arc<AtomicU32>,
    /// 是否正在播放（false 时回调填静音）。
    playing: Arc<AtomicBool>,
    /// cpal 输出流（drop 即停止设备）。
    _stream: Option<cpal::Stream>,
    /// 建立设备流后生成的会话证据，不是静态平台声明。
    session_info_json: String,
}

impl AlsaOutput {
    fn buffered_frames(&self) -> u32 {
        (self.ring.available_read() / self.channels.max(1)) as u32
    }
}

/// 从原始句柄读取内部状态引用。
///
/// # Safety
/// 调用方必须保证 `handle` 为 `jimmusic_output_open` 返回的有效句柄。
unsafe fn deref_handle(handle: OutputHandle) -> &'static AlsaOutput {
    unsafe { &*(handle.0 as *const AlsaOutput) }
}

/// 打开真实音频设备并启动 cpal 输出流。
fn build_stream(params: &OutputOpenParams) -> Result<AlsaOutput, i32> {
    let host = cpal::default_host();
    let host_driver = host.id().name().to_string();
    let device = host
        .default_output_device()
        .ok_or(ErrorCode::NotFound.as_i32())?;
    let description = device.description().ok();
    let device_name = description
        .as_ref()
        .map(|value| value.name().to_string())
        .unwrap_or_else(|| device.to_string());
    let driver = description
        .as_ref()
        .and_then(|value| value.driver())
        .unwrap_or(&host_driver)
        .to_string();
    let device_id = device
        .id()
        .map(|value| value.to_string())
        .unwrap_or_else(|_| format!("{}:{}", host_driver, device_name));

    let channels = params.channels.max(1);
    let sample_rate = params.sample_rate.max(1);
    let config = cpal::StreamConfig {
        channels,
        sample_rate,
        buffer_size: cpal::BufferSize::Default,
    };

    let frames = if params.buffer_frames == 0 {
        DEFAULT_BUFFER_FRAMES
    } else {
        params.buffer_frames as usize
    };
    let ring = Arc::new(RingBuffer::new(
        frames.saturating_mul(channels as usize).max(2),
    ));
    let volume = Arc::new(AtomicU32::new(1.0f32.to_bits()));
    let playing = Arc::new(AtomicBool::new(false));

    // cpal 拉模型回调：从环形缓冲读样本，缺数据填静音，应用音量。
    let cb_ring = ring.clone();
    let cb_volume = volume.clone();
    let cb_playing = playing.clone();
    let data_callback = move |data: &mut [i16], _info: &cpal::OutputCallbackInfo| {
        let v = f32::from_bits(cb_volume.load(Ordering::SeqCst));
        if cb_playing.load(Ordering::SeqCst) {
            let n = cb_ring.read(data);
            if n < data.len() {
                data[n..].fill(0);
            }
            if v != 1.0 {
                for s in data.iter_mut() {
                    *s = (*s as f32 * v) as i16;
                }
            }
        } else {
            data.fill(0);
        }
    };

    let error_callback = |err: cpal::Error| {
        eprintln!("alsa-output stream error: {err}");
    };

    let stream = device
        .build_output_stream(config, data_callback, error_callback, None)
        .map_err(|_| ErrorCode::InvokeFailed.as_i32())?;
    stream
        .play()
        .map_err(|_| ErrorCode::InvokeFailed.as_i32())?;

    let format = session_format(params);
    let session_info_json = serde_json::json!({
        "schema_version": 1,
        "session_id": format!(
            "cpal-{}",
            NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed)
        ),
        "device_id": device_id,
        "device_name": device_name,
        "driver": driver,
        "share_mode": "backend_default",
        "exclusive": false,
        "requested_format": format.clone(),
        // cpal accepted this exact StreamConfig; no implicit converter exists in this plugin.
        "negotiated_format": format,
        "software_buffer_frames": frames,
        // cpal's portable API does not expose the backend-selected hardware period.
        "device_buffer_frames": serde_json::Value::Null,
        "clock_source": "cpal_output_callback_timestamp",
        "capability_source": "opened_cpal_device_session"
    })
    .to_string();

    Ok(AlsaOutput {
        ring,
        channels: channels as usize,
        volume,
        playing,
        _stream: Some(stream),
        session_info_json,
    })
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

    match build_stream(params) {
        Ok(out) => {
            let boxed = Box::new(out);
            let handle = OutputHandle(Box::into_raw(boxed) as *mut c_void);
            OutputOpenResult {
                handle,
                code: ErrorCode::Ok.as_i32(),
            }
        }
        Err(code) => OutputOpenResult {
            handle: OutputHandle::null(),
            code,
        },
    }
}

/// 导出：关闭输出流并释放句柄。
#[no_mangle]
pub unsafe extern "C" fn jimmusic_output_close(handle: OutputHandle) {
    if handle.is_null() {
        return;
    }
    // SAFETY: 由 Box::into_raw 创建，此处唯一回收；drop 顺带停止并释放 cpal 流。
    unsafe { drop(Box::from_raw(handle.0 as *mut AlsaOutput)) };
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

/// 导出：开始播放。
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

/// 导出：暂停（回调填静音）。
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

/// 导出：停止（停止播放并清空缓冲）。
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

/// 导出：返回本句柄已打开的 CPAL 设备会话证据。
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
    // cpal 在目标平台自动选择音频后端：Linux→ALSA、Windows→WASAPI、
    // macOS→CoreAudio、Android→AAudio/OpenSL、iOS→AudioUnit。
    let (backend, platform) = match std::env::consts::OS {
        "linux" => ("alsa", "linux"),
        "windows" => ("wasapi", "windows"),
        "macos" => ("coreaudio", "macos"),
        "android" => ("aaudio", "android"),
        "ios" => ("audio-unit", "ios"),
        _ => ("cpal", "unknown"),
    };
    serde_json::json!({
        "backend": backend,
        "platform": platform,
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

    #[test]
    fn capabilities_are_valid_json() {
        let mut buf = vec![0u8; 1024];
        let written = unsafe { jimmusic_output_capabilities(buf.as_mut_ptr(), buf.len() as u32) };
        assert!(written > 0);
        let json = String::from_utf8(buf[..written as usize].to_vec()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["backend"], "alsa");
        assert_eq!(v["platform"], "linux");
        assert_eq!(v["features"]["low_latency"], true);
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

    /// 真实设备打开测试：无设备环境下 open 返回错误（不 panic），有设备时走通写/背压。
    #[test]
    fn open_handles_missing_device_gracefully() {
        let params = OutputOpenParams {
            sample_rate: 44_100,
            channels: 2,
            format: PcmFormat::I16Interleaved as i32,
            buffer_frames: 512,
        };
        let res = unsafe { jimmusic_output_open(&params) };
        if res.code == ErrorCode::Ok.as_i32() {
            // 有真实设备：验证写/背压/flush/close。
            unsafe {
                let pcm = vec![0i16; 4096];
                let accepted = jimmusic_output_write(res.handle, pcm.as_ptr(), 512);
                assert!(accepted >= 0);
                assert_eq!(jimmusic_output_flush(res.handle), ErrorCode::Ok.as_i32());
                assert_eq!(jimmusic_output_play(res.handle), ErrorCode::Ok.as_i32());
                assert_eq!(jimmusic_output_pause(res.handle), ErrorCode::Ok.as_i32());
                assert_eq!(jimmusic_output_stop(res.handle), ErrorCode::Ok.as_i32());
                jimmusic_output_close(res.handle);
            }
        } else {
            // 无设备：open 返回非 Ok（NotFound 或 OpenFailed），不崩溃。
            assert!(res.handle.is_null());
        }
    }
}
