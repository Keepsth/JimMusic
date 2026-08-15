//! 音频输出插件附加 ABI（需求 3.3「统一输出 ABI（句柄化 + 推模型）」）。
//!
//! 输出插件在标准插件符号（`jimmusic_abi_version` / `jimmusic_plugin_info` / ...）之外，
//! 额外导出本模块约定的一组符号：
//!
//! | 符号                          | 语义                                                       |
//! |-------------------------------|------------------------------------------------------------|
//! | `jimmusic_output_open`        | 创建输出流，返回不透明句柄（支持多实例）                    |
//! | `jimmusic_output_close`       | 关闭输出流并释放句柄                                        |
//! | `jimmusic_output_write`       | 推模型写入交错 PCM（有界缓冲 + 背压）                       |
//! | `jimmusic_output_play/pause/stop` | 控制输出设备启停                                        |
//! | `jimmusic_output_flush`       | 清空/冲刷缓冲                                              |
//! | `jimmusic_output_set_volume`  | 设置音量（0.0 ~ 1.0）                                       |
//! | `jimmusic_output_buffered_frames` | 查询当前缓冲帧数（背压依据）                            |
//! | `jimmusic_output_capabilities` | 返回 JSON 能力描述（平台/后端/采样率/声道/格式/特性）      |
//! | `jimmusic_output_session_info` | 返回已打开会话的设备、驱动、协商格式、缓冲与时钟证据 |
//!
//! 所有函数返回值均以 [`crate::ErrorCode`] 的判别值（`i32`）表示，`Ok` 即成功。
//! `jimmusic_output_write` 的返回值为实际入队的**采样帧数**（`i32`），
//! 当缓冲已满时返回 `0` 表示背压（调用方应稍后重试）；负值表示错误（未初始化等）。

use std::ffi::c_void;

/// 输出插件导出的符号名。核心经 `libloading` 按名查找（NUL 结尾字节串）。
pub mod symbols {
    pub const OUTPUT_OPEN: &[u8] = b"jimmusic_output_open\0";
    pub const OUTPUT_CLOSE: &[u8] = b"jimmusic_output_close\0";
    pub const OUTPUT_WRITE: &[u8] = b"jimmusic_output_write\0";
    pub const OUTPUT_PLAY: &[u8] = b"jimmusic_output_play\0";
    pub const OUTPUT_PAUSE: &[u8] = b"jimmusic_output_pause\0";
    pub const OUTPUT_STOP: &[u8] = b"jimmusic_output_stop\0";
    pub const OUTPUT_FLUSH: &[u8] = b"jimmusic_output_flush\0";
    pub const OUTPUT_SET_VOLUME: &[u8] = b"jimmusic_output_set_volume\0";
    pub const OUTPUT_BUFFERED_FRAMES: &[u8] = b"jimmusic_output_buffered_frames\0";
    pub const OUTPUT_CAPABILITIES: &[u8] = b"jimmusic_output_capabilities\0";
    pub const OUTPUT_SESSION_INFO: &[u8] = b"jimmusic_output_session_info\0";
}

/// 输出流不透明句柄。`open` 返回，其余函数收到后原样回传。
///
/// 用 `#[repr(transparent)]` 新类型包装裸指针，既保证 FFI 布局与 `*mut c_void` 一致，
/// 又能为句柄提供 `Send + Sync`（核心可在 Tokio 任务间共享输出流）。
#[derive(Debug, Clone, Copy)]
#[repr(transparent)]
pub struct OutputHandle(pub *mut c_void);

impl OutputHandle {
    /// 空句柄（无效值）。
    pub const fn null() -> Self {
        OutputHandle(std::ptr::null_mut())
    }

    /// 是否为有效（非空）句柄。
    pub fn is_null(self) -> bool {
        self.0.is_null()
    }
}

// 句柄只是不透明指针，不携带所有权；其生命周期由输出插件（`close`）管理。
// 跨线程共享安全。
unsafe impl Send for OutputHandle {}
unsafe impl Sync for OutputHandle {}

/// PCM 样本格式（交错 i16，对齐 symphonia 解码输出的 `Vec<i16>`）。
///
/// 为未来扩展（f32 / u8 等）保留判别值编号；当前输出 ABI **仅**支持 `I16Interleaved`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum PcmFormat {
    /// 16-bit 有符号整数，交错声道。
    I16Interleaved = 0,
    /// 32-bit 浮点，交错声道（预留，当前未实现）。
    F32Interleaved = 1,
}

impl PcmFormat {
    /// 从判别值还原；未知值回退到 [`PcmFormat::I16Interleaved`]。
    pub fn from_i32(value: i32) -> Self {
        match value {
            0 => PcmFormat::I16Interleaved,
            1 => PcmFormat::F32Interleaved,
            _ => PcmFormat::I16Interleaved,
        }
    }
}

/// `jimmusic_output_open` 的参数：描述输出流所需的 PCM 规格。
///
/// 输出插件据此校验能力、创建并返回 [`OutputHandle`]。
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct OutputOpenParams {
    /// 采样率（Hz）。
    pub sample_rate: u32,
    /// 声道数。
    pub channels: u16,
    /// 样本格式（见 [`PcmFormat`]）。
    pub format: i32,
    /// 缓冲帧数（有界缓冲容量，供背压控制）；`0` 表示由插件决定。
    pub buffer_frames: u32,
}

/// `jimmusic_output_open` 的结果：句柄 + 是否支持该规格。
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct OutputOpenResult {
    /// 输出流句柄；失败时为 `null`。
    pub handle: OutputHandle,
    /// 失败原因码（[`crate::ErrorCode`] 判别值）；成功时为 `Ok`。
    pub code: i32,
}

/// 输出插件各导出符号的 C 函数指针类型（供核心加载后强类型调用）。
///
/// 函数指针签名与插件侧 `#[no_mangle] extern "C"` 导出严格一致。
pub mod fns {
    use super::*;

    /// `open(params) -> OutputOpenResult`。
    pub type Open = unsafe extern "C" fn(*const OutputOpenParams) -> OutputOpenResult;
    /// `close(handle)`。
    pub type Close = unsafe extern "C" fn(OutputHandle);
    /// `write(handle, pcm_ptr, frames) -> frames_accepted`。
    pub type Write = unsafe extern "C" fn(OutputHandle, *const i16, u32) -> i32;
    /// `play/pause/stop/flush(handle) -> i32`。
    pub type Control = unsafe extern "C" fn(OutputHandle) -> i32;
    /// `set_volume(handle, volume) -> i32`。
    pub type SetVolume = unsafe extern "C" fn(OutputHandle, f32) -> i32;
    /// `buffered_frames(handle) -> u32`。
    pub type BufferedFrames = unsafe extern "C" fn(OutputHandle) -> u32;
    /// `capabilities(out_buf, capacity) -> i32`（写入 NUL 结尾 JSON，返回写入字节数或负错误）。
    pub type Capabilities = unsafe extern "C" fn(*mut u8, u32) -> i32;
    /// `session_info(handle, out_buf, capacity) -> i32`；数据必须来自已打开会话。
    pub type SessionInfo = unsafe extern "C" fn(OutputHandle, *mut u8, u32) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_format_roundtrip() {
        assert_eq!(PcmFormat::from_i32(0), PcmFormat::I16Interleaved);
        assert_eq!(PcmFormat::from_i32(1), PcmFormat::F32Interleaved);
        // 未知值回退为默认格式。
        assert_eq!(PcmFormat::from_i32(999), PcmFormat::I16Interleaved);
    }

    #[test]
    fn open_params_layout_fields_accessible() {
        let p = OutputOpenParams {
            sample_rate: 44_100,
            channels: 2,
            format: PcmFormat::I16Interleaved as i32,
            buffer_frames: 4096,
        };
        assert_eq!(p.sample_rate, 44_100);
        assert_eq!(p.channels, 2);
        assert_eq!(p.format, 0);
        assert_eq!(p.buffer_frames, 4096);
    }
}
