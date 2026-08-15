//! JimMusic 插件 ABI（Application Binary Interface）。
//!
//! 该模块定义了核心（app-core）与各插件之间跨动态库边界的统一 C ABI：
//! - 使用 `#[repr(C)]` 保证内存布局在动态库边界两侧一致；
//! - 插件导出固定符号，核心通过 [`libloading`][libloading] 运行时加载；
//! - 所有跨边界指针均为裸指针（`*const` / `*mut`），所有权约定见各函数文档。
//!
//! [libloading]: https://docs.rs/libloading
//!
//! ## 导出符号约定
//!
//! 每个插件动态库必须导出以下符号（`#[no_mangle] extern "C"`）：
//!
//! - `jimmusic_abi_version() -> u32`：插件所遵循的 ABI 版本，须等于
//!   [`ABI_VERSION`]。
//! - `jimmusic_plugin_info() -> *const PluginInfo`：返回指向静态 `PluginInfo`
//!   的指针，核心只读，不负责释放。
//! - `jimmusic_plugin_init(ctx: *mut HostCtx) -> ErrorCode`：插件初始化。
//! - `jimmusic_plugin_shutdown()`：插件清理。
//! - `jimmusic_plugin_invoke(request: *const InvokeRequest, response: *mut InvokeResponse) -> ErrorCode`
//!   ：核心调用插件能力的统一入口。
//!
//! ## 回调约定
//!
//! `HostCtx` 提供一组函数指针，允许插件在初始化后回调核心（例如上报播放进度、
//! 请求日志输出）。向上转型的指针由核心保证在使用期间有效。

#![deny(unsafe_op_in_unsafe_fn)]

/// 当前插件 ABI 版本。核心与插件版本不一致时拒绝加载。
pub const ABI_VERSION: u32 = 1;

/// 统一错误码。跨 FFI 边界只能交换 `i32` 数值，因此使用此枚举的判别值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ErrorCode {
    /// 成功。
    Ok = 0,
    /// 未知错误。
    Unknown = 1,
    /// 参数非法（空指针、越界等）。
    InvalidArgument = 2,
    /// ABI 版本不匹配。
    AbiMismatch = 3,
    /// 插件未初始化。
    NotInitialized = 4,
    /// 动态库加载失败。
    LoadFailed = 5,
    /// 符号查找失败。
    SymbolNotFound = 6,
    /// 插件能力调用失败。
    InvokeFailed = 7,
    /// 资源耗尽（内存不足、句柄不足等）。
    OutOfMemory = 8,
    /// 不支持的操作。
    Unsupported = 9,
    /// 未找到指定目标（插件、文件等）。
    NotFound = 10,
}

impl ErrorCode {
    /// 将自身转换为判别值（`i32`），用于跨 FFI 边界。
    pub const fn as_i32(self) -> i32 {
        self as i32
    }

    /// 从判别值还原错误码；未知值回退到 [`ErrorCode::Unknown`]。
    pub fn from_i32(value: i32) -> Self {
        match value {
            0 => ErrorCode::Ok,
            2 => ErrorCode::InvalidArgument,
            3 => ErrorCode::AbiMismatch,
            4 => ErrorCode::NotInitialized,
            5 => ErrorCode::LoadFailed,
            6 => ErrorCode::SymbolNotFound,
            7 => ErrorCode::InvokeFailed,
            8 => ErrorCode::OutOfMemory,
            9 => ErrorCode::Unsupported,
            10 => ErrorCode::NotFound,
            _ => ErrorCode::Unknown,
        }
    }
}

/// 插件种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PluginKind {
    /// 解码器（FFmpeg / Symphonia）。
    Decoder = 1,
    /// UI 桥接。
    UiBridge = 2,
    /// 搜索。
    Search = 3,
    /// 收藏。
    Favorite = 4,
    /// 音频输出后端（ALSA / PipeWire / WASAPI / CoreAudio / AAudio / AudioUnit / Web Audio）。
    AudioOutput = 5,
    /// 未知/扩展类型。
    Unknown = 0,
}

/// 插件的静态元数据。由 `jimmusic_plugin_info()` 返回，核心只读。
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct PluginInfo {
    /// 插件名称，以 NUL 结尾的 UTF-8 字节串。
    pub name: *const u8,
    /// 插件版本，以 NUL 结尾的 UTF-8 字节串。
    pub version: *const u8,
    /// 插件作者，以 NUL 结尾的 UTF-8 字节串。
    pub author: *const u8,
    /// 插件种类。
    pub kind: PluginKind,
    /// 插件所遵循的 ABI 版本，须等于 [`ABI_VERSION`]。
    pub abi_version: u32,
}

impl PluginInfo {
    /// 从静态 C 字符串构造元数据。所有字符串必须是 `'static` 的 NUL 结尾字节串。
    pub const fn from_static(
        name: &'static [u8],
        version: &'static [u8],
        author: &'static [u8],
        kind: PluginKind,
    ) -> Self {
        PluginInfo {
            name: name.as_ptr(),
            version: version.as_ptr(),
            author: author.as_ptr(),
            kind,
            abi_version: ABI_VERSION,
        }
    }
}

/// 插件调用请求：调用方（核心）传入的操作与输入缓冲。
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct InvokeRequest {
    /// 操作名，以 NUL 结尾的 UTF-8 字节串。
    pub op: *const u8,
    /// 输入缓冲指针，可为空（当 `input_len == 0`）。
    pub input: *const u8,
    /// 输入缓冲长度。
    pub input_len: usize,
}

/// 插件调用响应：插件填入输出缓冲（由核心预先分配）。
///
/// 插件最多写入 `capacity` 字节，并通过 `written` 返回实际写入长度。
/// 若输出超过 `capacity`，应返回 [`ErrorCode::InvalidArgument`] 或截断并置溢出标记。
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct InvokeResponse {
    /// 输出缓冲指针，由核心分配并保证生命周期覆盖本次调用，可为空。
    pub output: *mut u8,
    /// 输出缓冲容量。
    pub capacity: usize,
    /// 实际写入的字节数（由插件设置）。
    pub written: usize,
}

/// 宿主上下文：核心在插件初始化时提供的回调表。
///
/// 结构内所有函数指针均可为空（`None`）；插件调用前须判空。
/// 通过 `user_data` 可以携带宿主私有状态，其生命周期由宿主保证。
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct HostCtx {
    /// 日志回调：`level` 为 `tracing::Level` 的数值表示。
    pub log: Option<unsafe extern "C" fn(user_data: *mut c_void, level: i32, msg: *const u8)>,
    /// 进度回调（UI 桥插件可用于上报播放进度，`progress` 取 [0.0, 1.0]）
    pub progress: Option<unsafe extern "C" fn(user_data: *mut c_void, progress: f64)>,
    /// 宿主私有数据，原样透传给回调。
    pub user_data: *mut c_void,
}

pub mod audio_v2;
/// 音频输出插件的附加 C ABI。
///
/// 输出插件除导出标准插件符号（`jimmusic_abi_version` / `jimmusic_plugin_info` / ...）外，
/// 还额外导出本节定义的一组**句柄化 + 推模型**的符号，用于把解码后的 PCM 流送到音频设备。
pub mod output;

// 复用标准库的 `c_void`，避免重复定义。
use std::ffi::c_void;

// PluginInfo 仅包含指向不可变静态字节串的只读指针与无内部可变性字段，跨线程共享安全。
unsafe impl Sync for PluginInfo {}
unsafe impl Send for PluginInfo {}

// HostCtx / InvokeRequest / InvokeResponse 均为纯数据描述结构，跨线程传递安全。
unsafe impl Sync for HostCtx {}
unsafe impl Send for HostCtx {}
unsafe impl Sync for InvokeRequest {}
unsafe impl Send for InvokeRequest {}
unsafe impl Sync for InvokeResponse {}
unsafe impl Send for InvokeResponse {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_roundtrip() {
        for code in [
            ErrorCode::Ok,
            ErrorCode::InvalidArgument,
            ErrorCode::AbiMismatch,
            ErrorCode::NotInitialized,
            ErrorCode::LoadFailed,
            ErrorCode::SymbolNotFound,
            ErrorCode::InvokeFailed,
            ErrorCode::OutOfMemory,
            ErrorCode::Unsupported,
        ] {
            assert_eq!(ErrorCode::from_i32(code.as_i32()), code);
        }
        assert_eq!(ErrorCode::from_i32(12345), ErrorCode::Unknown);
    }

    #[test]
    fn plugin_info_layout() {
        let info = PluginInfo::from_static(b"test\0", b"0.1.0\0", b"author\0", PluginKind::Decoder);
        assert_eq!(info.abi_version, ABI_VERSION);
        assert_eq!(info.kind, PluginKind::Decoder);
    }
}
