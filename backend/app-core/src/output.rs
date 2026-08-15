//! 音频输出插件的宿主侧封装（需求 3.3「统一输出 ABI」）。
//!
//! [`OutputPlugin`] 动态加载输出插件库，解析其附加输出符号，并据此创建
//! [`OutputStream`]（对应一个 `open` 出的输出流句柄）。核心的 [`crate::PlaybackEngine`]
//! 通过 [`OutputStream::write`] 以推模型写入交错 PCM，配合有界缓冲实现背压。

use std::ffi::CStr;
use std::path::Path;
use std::sync::Arc;

use plugin_abi::output::{self, fns, OutputHandle, OutputOpenParams};
use plugin_abi::{ErrorCode, PluginInfo, PluginKind};

/// 输出插件错误。
#[derive(Debug, thiserror::Error)]
pub enum OutputError {
    /// 动态库加载失败。
    #[error("failed to load output library `{0}`: {1}")]
    Load(String, String),
    /// 符号查找失败。
    #[error("output symbol `{0}` not found")]
    SymbolNotFound(String),
    /// 插件并非音频输出类型。
    #[error("plugin `{0}` is not an audio output plugin (kind: {1:?})")]
    NotOutput(String, PluginKind),
    /// ABI 版本不匹配。
    #[error("ABI version mismatch: expected {expected}, got {actual}")]
    AbiMismatch { expected: u32, actual: u32 },
    /// 能力查询失败（缓冲过小或插件错误）。
    #[error("capabilities query failed with code {0}")]
    Capabilities(i32),
    /// 能力 JSON 非法。
    #[error("invalid capabilities JSON: {0}")]
    BadCapabilities(String),
    /// 已打开会话查询失败。
    #[error("output session info query failed with code {0}")]
    SessionInfo(i32),
    /// 会话证据 JSON 非法或不完整。
    #[error("invalid output session info: {0}")]
    BadSessionInfo(String),
    /// 打开输出流失败。
    #[error("output open failed with code {0}")]
    OpenFailed(i32),
    /// 输出流操作失败。
    #[error("output operation failed with code {0}")]
    Operation(i32),
    /// 解码失败（播放引擎在输出前解码音轨）。
    #[error("decode error: {0}")]
    Decode(String),
    /// 非法 UTF-8（能力名等）。
    #[error("invalid UTF-8: {0}")]
    InvalidUtf8(String),
}

/// 输出插件声明的能力（`jimmusic_output_capabilities` 返回的 JSON 反序列化）。
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct OutputCapabilities {
    /// 后端名（如 `null` / `alsa` / `wasapi` / `coreaudio` / `web-audio`）。
    #[serde(default)]
    pub backend: String,
    /// 目标平台（如 `linux` / `windows` / `macos` / `web` / `any`）。
    #[serde(default)]
    pub platform: String,
    /// 支持的采样率列表。
    #[serde(default)]
    pub sample_rates: Vec<u32>,
    /// 支持的声道数列表。
    #[serde(default)]
    pub channels: Vec<u16>,
    /// 支持的样本格式（如 `["i16"]`）。
    #[serde(default)]
    pub formats: Vec<String>,
    /// 特性开关。
    #[serde(default)]
    pub features: OutputFeatures,
}

/// 输出插件特性开关（独占模式 / 硬件音量 / 低延迟）。
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct OutputFeatures {
    /// 是否支持独占模式。
    #[serde(default)]
    pub exclusive: bool,
    /// 是否支持硬件音量控制。
    #[serde(default)]
    pub hardware_volume: bool,
    /// 是否支持低延迟。
    #[serde(default)]
    pub low_latency: bool,
}

/// 打开前请求或打开后由驱动接受的音频格式。
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct OutputSessionFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_format: String,
    pub bit_depth: u16,
    pub packing: String,
}

/// 从一个成功打开的输出句柄查询的会话证据。
/// 该结构与静态 capabilities 分离，避免用平台字符串伪装协商成功。
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct OutputSessionInfo {
    pub schema_version: u16,
    pub session_id: String,
    pub device_id: String,
    pub device_name: String,
    pub driver: String,
    pub share_mode: String,
    pub exclusive: bool,
    pub requested_format: OutputSessionFormat,
    pub negotiated_format: OutputSessionFormat,
    pub software_buffer_frames: u32,
    pub device_buffer_frames: Option<u32>,
    pub clock_source: String,
    pub capability_source: String,
}

impl OutputSessionInfo {
    fn validate(&self) -> Result<(), OutputError> {
        if self.schema_version != 1 {
            return Err(OutputError::BadSessionInfo(format!(
                "unsupported schema version {}",
                self.schema_version
            )));
        }
        if self.session_id.is_empty()
            || self.device_id.is_empty()
            || self.driver.is_empty()
            || self.clock_source.is_empty()
            || self.capability_source.is_empty()
            || self.negotiated_format.sample_rate == 0
            || self.negotiated_format.channels == 0
            || self.software_buffer_frames == 0
        {
            return Err(OutputError::BadSessionInfo(
                "opened-session evidence contains empty or zero required fields".into(),
            ));
        }
        Ok(())
    }
}

/// 输出插件导出的函数表（强类型函数指针）。
struct OutputFns {
    open: fns::Open,
    close: fns::Close,
    write: fns::Write,
    play: fns::Control,
    pause: fns::Control,
    stop: fns::Control,
    flush: fns::Control,
    set_volume: fns::SetVolume,
    buffered_frames: fns::BufferedFrames,
    capabilities: fns::Capabilities,
    session_info: fns::SessionInfo,
}

/// 一个已加载的音频输出插件。
pub struct OutputPlugin {
    name: String,
    fns: OutputFns,
    capabilities: OutputCapabilities,
    // 必须在 `fns` 之后 drop，保证函数指针存活到库释放之前。
    _library: libloading::Library,
}

/// 一个已打开的输出流（`open` 返回句柄的 RAII 封装）。
pub struct OutputStream {
    plugin: Arc<OutputPlugin>,
    handle: OutputHandle,
    session_info: OutputSessionInfo,
}

impl OutputPlugin {
    /// 从动态库路径加载输出插件并解析其能力。
    pub fn load(path: impl AsRef<Path>) -> Result<Self, OutputError> {
        let path = path.as_ref();
        // SAFETY: 打开操作系统动态库。
        let library = unsafe { libloading::Library::new(path) }
            .map_err(|e| OutputError::Load(path.display().to_string(), e.to_string()))?;

        // 校验 ABI 版本与插件种类（复用标准插件符号）。
        // SAFETY: 符号来自已加载且仍存活的库。
        let abi_version: libloading::Symbol<'static, unsafe extern "C" fn() -> u32> = unsafe {
            library
                .get(b"jimmusic_abi_version\0")
                .map(extend_lifetime)
                .map_err(|_| OutputError::SymbolNotFound("jimmusic_abi_version".into()))?
        };
        let actual = unsafe { (*abi_version)() };
        if actual != plugin_abi::ABI_VERSION {
            return Err(OutputError::AbiMismatch {
                expected: plugin_abi::ABI_VERSION,
                actual,
            });
        }

        // SAFETY: 读取插件静态元数据指针（只读）。
        let info_fn: libloading::Symbol<'static, unsafe extern "C" fn() -> *const PluginInfo> = unsafe {
            library
                .get(b"jimmusic_plugin_info\0")
                .map(extend_lifetime)
                .map_err(|_| OutputError::SymbolNotFound("jimmusic_plugin_info".into()))?
        };
        let info_ptr = unsafe { (*info_fn)() };
        if info_ptr.is_null() {
            return Err(OutputError::SymbolNotFound("jimmusic_plugin_info".into()));
        }
        // SAFETY: info_ptr 非空且指向有效 PluginInfo。
        let info = unsafe { &*info_ptr };
        if info.kind != PluginKind::AudioOutput {
            return Err(OutputError::NotOutput(plugin_name(info), info.kind));
        }

        let fns = unsafe { lookup_output_fns(&library)? };

        let mut plugin = OutputPlugin {
            name: plugin_name(info),
            fns,
            capabilities: OutputCapabilities::default(),
            _library: library,
        };

        // 查询并解析能力。
        let caps = plugin.query_capabilities()?;
        plugin.capabilities = caps;
        Ok(plugin)
    }

    /// 插件名。
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 插件声明的能力。
    pub fn capabilities(&self) -> &OutputCapabilities {
        &self.capabilities
    }

    /// 打开一个输出流（返回 RAII 句柄）。
    pub fn open(self: &Arc<Self>, params: OutputOpenParams) -> Result<OutputStream, OutputError> {
        // SAFETY: params 为合法 repr(C) 结构，open 函数指针来自仍存活的库。
        let result = unsafe { (self.fns.open)(&params) };
        if result.code != ErrorCode::Ok.as_i32() || result.handle.is_null() {
            return Err(OutputError::OpenFailed(result.code));
        }
        let mut stream = OutputStream {
            plugin: self.clone(),
            handle: result.handle,
            session_info: OutputSessionInfo {
                schema_version: 0,
                session_id: String::new(),
                device_id: String::new(),
                device_name: String::new(),
                driver: String::new(),
                share_mode: String::new(),
                exclusive: false,
                requested_format: OutputSessionFormat {
                    sample_rate: 0,
                    channels: 0,
                    sample_format: String::new(),
                    bit_depth: 0,
                    packing: String::new(),
                },
                negotiated_format: OutputSessionFormat {
                    sample_rate: 0,
                    channels: 0,
                    sample_format: String::new(),
                    bit_depth: 0,
                    packing: String::new(),
                },
                software_buffer_frames: 0,
                device_buffer_frames: None,
                clock_source: String::new(),
                capability_source: String::new(),
            },
        };
        stream.session_info = stream.query_session_info()?;
        Ok(stream)
    }

    /// 查询输出插件能力（JSON → 结构体）。
    fn query_capabilities(&self) -> Result<OutputCapabilities, OutputError> {
        const CAP: usize = 4096;
        let mut buf = vec![0u8; CAP];
        // SAFETY: buf 为有效可写缓冲，capabilities 函数指针来自仍存活的库。
        let written = unsafe { (self.fns.capabilities)(buf.as_mut_ptr(), CAP as u32) };
        if written < 0 {
            return Err(OutputError::Capabilities(written));
        }
        let written = written as usize;
        if written == 0 || written > CAP {
            return Err(OutputError::BadCapabilities("empty or oversized".into()));
        }
        // 去除可能的尾随 NUL。
        let end = buf[..written]
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(written);
        let json = std::str::from_utf8(&buf[..end])
            .map_err(|e| OutputError::InvalidUtf8(e.to_string()))?;
        serde_json::from_str(json).map_err(|e| OutputError::BadCapabilities(e.to_string()))
    }
}

impl OutputStream {
    /// 输出流句柄（不透明）。
    pub fn handle(&self) -> OutputHandle {
        self.handle
    }

    /// 返回已打开会话的真实协商与能力来源快照。
    pub fn session_info(&self) -> &OutputSessionInfo {
        &self.session_info
    }

    fn query_session_info(&self) -> Result<OutputSessionInfo, OutputError> {
        const CAP: usize = 8 * 1024;
        let mut buffer = vec![0u8; CAP];
        // SAFETY: handle 已成功打开，buffer 在调用期间可写。
        let written =
            unsafe { (self.plugin.fns.session_info)(self.handle, buffer.as_mut_ptr(), CAP as u32) };
        if written < 0 {
            return Err(OutputError::SessionInfo(written));
        }
        let written = written as usize;
        if written == 0 || written > CAP {
            return Err(OutputError::BadSessionInfo("empty or oversized".into()));
        }
        let end = buffer[..written]
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(written);
        let json = std::str::from_utf8(&buffer[..end])
            .map_err(|error| OutputError::InvalidUtf8(error.to_string()))?;
        let info: OutputSessionInfo = serde_json::from_str(json)
            .map_err(|error| OutputError::BadSessionInfo(error.to_string()))?;
        info.validate()?;
        Ok(info)
    }

    /// 当前缓冲帧数（背压依据）。
    pub fn buffered_frames(&self) -> u32 {
        // SAFETY: handle 有效，函数指针来自仍存活的库（plugin 持有）。
        unsafe { (self.plugin.fns.buffered_frames)(self.handle) }
    }

    /// 推模型写入交错 PCM。返回实际入队的采样帧数（可能因背压而小于请求值）。
    ///
    /// `samples` 必须为 `frames * channels` 个交错 i16 样本。
    pub fn write(&self, samples: &[i16], frames: u32) -> Result<u32, OutputError> {
        if samples.is_empty() {
            return Ok(0);
        }
        // SAFETY: samples 为有效切片，函数指针来自仍存活的库。
        let written = unsafe { (self.plugin.fns.write)(self.handle, samples.as_ptr(), frames) };
        if written < 0 {
            Err(OutputError::Operation(written))
        } else {
            Ok(written as u32)
        }
    }

    /// 开始播放。
    pub fn play(&self) -> Result<(), OutputError> {
        self.control(self.plugin.fns.play)
    }

    /// 暂停。
    pub fn pause(&self) -> Result<(), OutputError> {
        self.control(self.plugin.fns.pause)
    }

    /// 停止。
    pub fn stop(&self) -> Result<(), OutputError> {
        self.control(self.plugin.fns.stop)
    }

    /// 冲刷缓冲。
    pub fn flush(&self) -> Result<(), OutputError> {
        self.control(self.plugin.fns.flush)
    }

    /// 设置音量（0.0 ~ 1.0）。
    pub fn set_volume(&self, volume: f32) -> Result<(), OutputError> {
        // SAFETY: handle 有效，函数指针来自仍存活的库。
        let code = unsafe { (self.plugin.fns.set_volume)(self.handle, volume) };
        if code != ErrorCode::Ok.as_i32() {
            Err(OutputError::Operation(code))
        } else {
            Ok(())
        }
    }

    /// 统一控制操作（play/pause/stop/flush 共用签名）。
    fn control(&self, f: fns::Control) -> Result<(), OutputError> {
        // SAFETY: handle 有效，函数指针来自仍存活的库。
        let code = unsafe { f(self.handle) };
        if code != ErrorCode::Ok.as_i32() {
            Err(OutputError::Operation(code))
        } else {
            Ok(())
        }
    }
}

impl Drop for OutputStream {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            // SAFETY: 关闭并释放输出流句柄。
            unsafe { (self.plugin.fns.close)(self.handle) };
        }
    }
}

/// 从 `PluginInfo` 读取插件名（空指针/非法 UTF-8 回退为空串）。
fn plugin_name(info: &PluginInfo) -> String {
    if info.name.is_null() {
        return String::new();
    }
    // SAFETY: 调用方保证 name 为 NUL 结尾字节串。
    unsafe { CStr::from_ptr(info.name.cast()) }
        .to_string_lossy()
        .into_owned()
}

/// 从库中查找全部输出符号并延长生命周期。
///
/// # Safety
/// `library` 必须在返回的 [`OutputFns`] 存活期间保持有效（由 [`OutputPlugin::_library`] 保证）。
unsafe fn lookup_output_fns(library: &libloading::Library) -> Result<OutputFns, OutputError> {
    macro_rules! get {
        ($sym:expr, $ty:ty) => {
            unsafe {
                *library
                    .get::<$ty>($sym)
                    .map(extend_lifetime)
                    .map_err(|_| OutputError::SymbolNotFound(symbol_name($sym)))?
            }
        };
    }

    let open = get!(output::symbols::OUTPUT_OPEN, fns::Open);
    let close = get!(output::symbols::OUTPUT_CLOSE, fns::Close);
    let write = get!(output::symbols::OUTPUT_WRITE, fns::Write);
    let play = get!(output::symbols::OUTPUT_PLAY, fns::Control);
    let pause = get!(output::symbols::OUTPUT_PAUSE, fns::Control);
    let stop = get!(output::symbols::OUTPUT_STOP, fns::Control);
    let flush = get!(output::symbols::OUTPUT_FLUSH, fns::Control);
    let set_volume = get!(output::symbols::OUTPUT_SET_VOLUME, fns::SetVolume);
    let buffered_frames = get!(output::symbols::OUTPUT_BUFFERED_FRAMES, fns::BufferedFrames);
    let capabilities = get!(output::symbols::OUTPUT_CAPABILITIES, fns::Capabilities);
    let session_info = get!(output::symbols::OUTPUT_SESSION_INFO, fns::SessionInfo);

    Ok(OutputFns {
        open,
        close,
        write,
        play,
        pause,
        stop,
        flush,
        set_volume,
        buffered_frames,
        capabilities,
        session_info,
    })
}

/// 将符号名（NUL 结尾字节串）转为可读字符串用于错误信息。
fn symbol_name(sym: &[u8]) -> String {
    String::from_utf8_lossy(&sym[..sym.len().saturating_sub(1)]).into_owned()
}

/// 将借用生命周期的符号延长为 `'static`（安全性由调用方保证库不被卸载）。
fn extend_lifetime<'a, T>(s: libloading::Symbol<'a, T>) -> libloading::Symbol<'static, T> {
    // SAFETY: 仅调整生命周期，不改变内存表示；库由 OutputPlugin::_library 持有。
    unsafe { std::mem::transmute::<libloading::Symbol<'a, T>, libloading::Symbol<'static, T>>(s) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_deserialize_partial() {
        // 缺失字段回退默认值。
        let caps: OutputCapabilities =
            serde_json::from_str(r#"{"backend":"alsa","platform":"linux"}"#).unwrap();
        assert_eq!(caps.backend, "alsa");
        assert_eq!(caps.platform, "linux");
        assert!(caps.sample_rates.is_empty());
        assert!(!caps.features.low_latency);
    }

    #[test]
    fn capabilities_full_roundtrip() {
        let caps = OutputCapabilities {
            backend: "null".into(),
            platform: "any".into(),
            sample_rates: vec![44_100, 48_000],
            channels: vec![1, 2],
            formats: vec!["i16".into()],
            features: OutputFeatures {
                exclusive: false,
                hardware_volume: false,
                low_latency: true,
            },
        };
        let json = serde_json::to_string(&caps).unwrap();
        let back: OutputCapabilities = serde_json::from_str(&json).unwrap();
        assert_eq!(back.backend, "null");
        assert_eq!(back.sample_rates, vec![44_100, 48_000]);
        assert!(back.features.low_latency);
    }
}
