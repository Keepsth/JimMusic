//! JimMusic Symphonia 解码器插件。
//!
//! 按需求文档，Symphonia 为纯 Rust 实现、静态链接，作为可选插件以减小体积。
//! 本 crate 提供真实音频解码能力（见 [`decode`] 模块），并以统一 C ABI 导出供核心动态加载。

// 导出的 C ABI 符号（#[no_mangle] unsafe extern "C" fn）由核心通过 `libloading`
// 加载并调用，安全性由 ABI 契约保证，而非由 Rust 调用方自行提供的参数保证，
// 因此统一豁免 missing_safety_doc 检查。
#![allow(clippy::missing_safety_doc)]

pub mod decode;

pub use decode::{
    decode_file, decode_from_reader, read_metadata, DecodeError, DecodedAudio, DecodedChunk,
    StreamingDecoder, TrackMetadata,
};

// 以下为 C ABI 导出：仅在启用 `abi` feature 时编译，供作为独立动态库加载；
// 被其它解码器插件以 rlib 形式复用时（default-features = false）不导出这些符号，
// 避免与调用方自身的 jimmusic_* 导出产生链接冲突。
#[cfg(feature = "abi")]
mod abi {
    use std::ptr;
    use std::sync::atomic::{AtomicBool, Ordering};

    use plugin_abi::{ErrorCode, HostCtx, InvokeRequest, InvokeResponse, PluginInfo, PluginKind};

    static INITIALIZED: AtomicBool = AtomicBool::new(false);

    static INFO: PluginInfo = PluginInfo::from_static(
        b"symphonia-decoder\0",
        b"0.1.0\0",
        b"JimMusic Team\0",
        PluginKind::Decoder,
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

    /// 导出：初始化。
    #[no_mangle]
    pub unsafe extern "C" fn jimmusic_plugin_init(_ctx: *mut HostCtx) -> ErrorCode {
        INITIALIZED.store(true, Ordering::SeqCst);
        ErrorCode::Ok
    }

    /// 导出：清理。
    #[no_mangle]
    pub unsafe extern "C" fn jimmusic_plugin_shutdown() {
        INITIALIZED.store(false, Ordering::SeqCst);
    }

    /// 导出：统一调用入口。
    ///
    /// 支持的操作：
    /// - `formats`：返回支持的音频格式列表（逗号分隔）。
    /// - `info`：输入为文件路径（UTF-8），返回 JSON 元数据。
    /// - `decode`：输入为文件路径（UTF-8），返回 JSON 摘要（采样率/声道/帧数/样本数）。
    ///
    /// # Safety
    /// `request` / `response` 必须为有效指针。
    #[no_mangle]
    pub unsafe extern "C" fn jimmusic_plugin_invoke(
        request: *const InvokeRequest,
        response: *mut InvokeResponse,
    ) -> ErrorCode {
        if request.is_null() || response.is_null() {
            return ErrorCode::InvalidArgument;
        }
        if !INITIALIZED.load(Ordering::SeqCst) {
            return ErrorCode::NotInitialized;
        }

        let req = unsafe { &*request };
        if req.op.is_null() {
            return ErrorCode::InvalidArgument;
        }
        let op = unsafe { std::ffi::CStr::from_ptr(req.op.cast()) }.to_string_lossy();

        let body: Vec<u8> = match op.as_ref() {
            "formats" => b"mp3,aac,flac,wav,ogg,pcm".to_vec(),
            "info" | "decode" => {
                if req.input_len == 0 || req.input.is_null() {
                    return ErrorCode::InvalidArgument;
                }
                let path_bytes = unsafe { std::slice::from_raw_parts(req.input, req.input_len) };
                let path = match std::str::from_utf8(path_bytes) {
                    Ok(p) => p,
                    Err(_) => return ErrorCode::InvalidArgument,
                };

                let meta = match crate::read_metadata(std::path::Path::new(path)) {
                    Ok(m) => m,
                    Err(_) => return ErrorCode::InvokeFailed,
                };

                let json = serde_json::json!({
                    "title": meta.title,
                    "artist": meta.artist,
                    "album": meta.album,
                    "duration": meta.duration,
                    "sample_rate": meta.sample_rate,
                    "channels": meta.channels,
                });
                serde_json::to_vec(&json).unwrap_or_default()
            }
            _ => return ErrorCode::Unsupported,
        };

        let resp = unsafe { &mut *response };
        if body.len() > resp.capacity {
            return ErrorCode::InvalidArgument;
        }
        if !resp.output.is_null() {
            unsafe { ptr::copy_nonoverlapping(body.as_ptr(), resp.output, body.len()) };
        }
        resp.written = body.len();
        ErrorCode::Ok
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        unsafe fn call(op: &[u8], input: &[u8], cap: usize) -> (ErrorCode, Vec<u8>) {
            let req = InvokeRequest {
                op: op.as_ptr().cast(),
                input: if input.is_empty() {
                    ptr::null()
                } else {
                    input.as_ptr()
                },
                input_len: input.len(),
            };
            let mut out = vec![0u8; cap];
            let mut resp = InvokeResponse {
                output: out.as_mut_ptr(),
                capacity: cap,
                written: 0,
            };
            let code = unsafe { jimmusic_plugin_invoke(&req, &mut resp) };
            out.truncate(resp.written.min(out.len()));
            (code, out)
        }

        /// 串行覆盖 C ABI 导出路径，避免共享 INITIALIZED 状态在并行测试间竞态。
        #[test]
        fn c_abi_exports() {
            unsafe {
                jimmusic_plugin_shutdown();
                assert_eq!(call(b"formats\0", &[], 256).0, ErrorCode::NotInitialized);

                assert_eq!(jimmusic_abi_version(), plugin_abi::ABI_VERSION);
                let info = jimmusic_plugin_info();
                assert!(!info.is_null());
                assert_eq!((*info).kind, PluginKind::Decoder);

                assert_eq!(jimmusic_plugin_init(ptr::null_mut()), ErrorCode::Ok);

                let (code, out) = call(b"formats\0", &[], 256);
                assert_eq!(code, ErrorCode::Ok);
                assert_eq!(out, b"mp3,aac,flac,wav,ogg,pcm");

                assert_eq!(call(b"info\0", &[], 256).0, ErrorCode::InvalidArgument);
                assert_eq!(call(b"nope\0", &[], 256).0, ErrorCode::Unsupported);
                assert_eq!(
                    jimmusic_plugin_invoke(ptr::null(), ptr::null_mut()),
                    ErrorCode::InvalidArgument
                );
            }
        }
    }
}
