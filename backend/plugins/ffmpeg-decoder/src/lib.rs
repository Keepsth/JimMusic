//! JimMusic FFmpeg 解码器插件。
//!
//! 按需求文档，FFmpeg 解码器动态编译为共享库，支持 MP3/AAC/FLAC/WAV/OGG 等格式。
//! 本原型阶段：导出统一 C ABI 符号，`invoke` 支持 `formats` / `decode` 操作；
//! 真实 FFmpeg 绑定（第三方动态库）由 `build.rs` 后续接入。

// 导出的 C ABI 符号由核心经 `libloading` 加载调用，安全性由 ABI 契约保证，
// 统一豁免 missing_safety_doc 检查。
#![allow(clippy::missing_safety_doc)]

use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};

use plugin_abi::{ErrorCode, HostCtx, InvokeRequest, InvokeResponse, PluginInfo, PluginKind};

use decode::{decode_file, read_metadata};

static INITIALIZED: AtomicBool = AtomicBool::new(false);

static INFO: PluginInfo = PluginInfo::from_static(
    b"ffmpeg-decoder\0",
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
///
/// # Safety
/// `ctx` 可为空（原型阶段不使用宿主回调）。
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
        "formats" => b"mp3,aac,flac,wav,ogg".to_vec(),
        "info" => match read_path(req) {
            Ok(path) => match read_metadata(path) {
                Ok(meta) => serde_json::json!({
                    "decoder": "symphonia",
                    "title": meta.title,
                    "artist": meta.artist,
                    "album": meta.album,
                    "duration": meta.duration,
                    "sample_rate": meta.sample_rate,
                    "channels": meta.channels,
                })
                .to_string()
                .into_bytes(),
                Err(_) => return ErrorCode::InvokeFailed,
            },
            Err(code) => return code,
        },
        "decode" => match read_path(req) {
            Ok(path) => match decode_file(path) {
                Ok(audio) => {
                    // 返回 JSON 摘要 + 前若干 PCM 样本（i16 小端），证明真实解码发生。
                    let mut out = serde_json::json!({
                        "decoder": "symphonia",
                        "sample_rate": audio.sample_rate,
                        "channels": audio.channels,
                        "frames": audio.frames,
                        "sample_count": audio.samples.len(),
                    })
                    .to_string();
                    out.push('\n');
                    let mut bytes = out.into_bytes();
                    for s in audio.samples.iter().take(512) {
                        bytes.extend_from_slice(&s.to_le_bytes());
                    }
                    bytes
                }
                Err(_) => return ErrorCode::InvokeFailed,
            },
            Err(code) => return code,
        },
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

/// 从请求输入中读取 UTF-8 文件路径。
///
/// # Safety
/// `req.input` 必须指向至少 `req.input_len` 字节的有效内存（调用方保证）。
unsafe fn read_path(req: &InvokeRequest) -> Result<&str, ErrorCode> {
    if req.input_len == 0 || req.input.is_null() {
        return Err(ErrorCode::InvalidArgument);
    }
    let bytes = unsafe { std::slice::from_raw_parts(req.input, req.input_len) };
    std::str::from_utf8(bytes).map_err(|_| ErrorCode::InvalidArgument)
}

/// 解码相关的内部模块（直接基于 symphonia，无 ABI 符号，避免链接冲突）。
mod decode {
    use std::path::Path;

    use symphonia::core::codecs::audio::AudioDecoderOptions;
    use symphonia::core::formats::probe::Hint;
    use symphonia::core::formats::{FormatOptions, TrackType};
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::{MetadataOptions, StandardTag};

    /// 音轨元数据。
    #[derive(Debug, Clone, Default)]
    pub struct TrackMetadata {
        pub title: Option<String>,
        pub artist: Option<String>,
        pub album: Option<String>,
        pub duration: Option<f64>,
        pub sample_rate: Option<u32>,
        pub channels: Option<u16>,
    }

    /// 解码结果。
    pub struct DecodedAudio {
        pub sample_rate: u32,
        pub channels: u16,
        pub frames: u64,
        pub samples: Vec<i16>,
    }

    /// 读取元数据。
    pub fn read_metadata(path: &str) -> Result<TrackMetadata, Box<dyn std::error::Error>> {
        let file = std::fs::File::open(path)?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut format = symphonia::default::get_probe().probe(
            &Hint::new(),
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )?;

        let mut meta = TrackMetadata::default();
        if let Some(rev) = format.metadata().current() {
            for tag in &rev.media.tags {
                if let Some(std) = &tag.std {
                    match std {
                        StandardTag::TrackTitle(v) => meta.title = Some(v.to_string()),
                        StandardTag::Artist(v) => meta.artist = Some(v.to_string()),
                        StandardTag::Album(v) => meta.album = Some(v.to_string()),
                        _ => {}
                    }
                }
            }
        }
        if let Some(track) = format.default_track(TrackType::Audio) {
            if let Some(params) = track.codec_params.as_ref().and_then(|p| p.audio()) {
                meta.sample_rate = params.sample_rate;
                meta.channels = params.channels.as_ref().map(|c| c.count() as u16);
            }
            if let (Some(dur), Some(tb)) = (track.duration, track.time_base) {
                if let Some(time) = tb.calc_duration(dur) {
                    meta.duration = Some(time.as_secs_f64());
                }
            }
        }
        Ok(meta)
    }

    /// 完整解码为 16-bit PCM。
    pub fn decode_file(path: &str) -> Result<DecodedAudio, Box<dyn std::error::Error>> {
        let file = std::fs::File::open(Path::new(path))?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut format = symphonia::default::get_probe().probe(
            &Hint::new(),
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )?;
        let track = format
            .default_track(TrackType::Audio)
            .ok_or("no audio track")?
            .clone();
        let codec_params = track.codec_params.as_ref().ok_or("no codec params")?;
        let audio_params = codec_params.audio().ok_or("no audio params")?;

        let sample_rate = audio_params.sample_rate.unwrap_or(44_100);
        let channels = audio_params
            .channels
            .as_ref()
            .map(|c| c.count())
            .unwrap_or(2) as u16;

        let mut decoder = symphonia::default::get_codecs()
            .make_audio_decoder(audio_params, &AudioDecoderOptions::default())?;
        let mut samples: Vec<i16> = Vec::new();
        while let Some(packet) = format.next_packet()? {
            let decoded = decoder.decode(&packet)?;
            let mut tmp: Vec<i16> = Vec::new();
            decoded.copy_to_vec_interleaved::<i16>(&mut tmp);
            samples.extend_from_slice(&tmp);
        }
        let frames = (samples.len() / channels.max(1) as usize) as u64;
        Ok(DecodedAudio {
            sample_rate,
            channels,
            frames,
            samples,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 便捷调用：构造 request/response 并调用 invoke。
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

    /// 单测试串行覆盖全部 invoke 分支，避免共享 INITIALIZED 全局状态在并行测试间竞态。
    #[test]
    fn invoke_dispatch_paths() {
        unsafe {
            // 1. 未初始化 → NotInitialized
            jimmusic_plugin_shutdown();
            assert_eq!(call(b"formats\0", &[], 256).0, ErrorCode::NotInitialized);

            // 2. 初始化后 formats
            jimmusic_plugin_init(ptr::null_mut());
            let (code, out) = call(b"formats\0", &[], 256);
            assert_eq!(code, ErrorCode::Ok);
            assert_eq!(out, b"mp3,aac,flac,wav,ogg");

            // 3. decode 无输入 → InvalidArgument
            assert_eq!(call(b"decode\0", &[], 256).0, ErrorCode::InvalidArgument);

            // 4. 未知操作 → Unsupported
            assert_eq!(call(b"nope\0", &[], 256).0, ErrorCode::Unsupported);

            // 5. 空指针 → InvalidArgument
            assert_eq!(
                jimmusic_plugin_invoke(ptr::null(), ptr::null_mut()),
                ErrorCode::InvalidArgument
            );

            // 6. 真实解码：decode op 返回 JSON 摘要 + PCM 样本。
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("tone.wav");
            write_wav(&path, 800);
            let (code, out) = call(b"decode\0", path.to_string_lossy().as_bytes(), 64 * 1024);
            assert_eq!(code, ErrorCode::Ok);
            let s = String::from_utf8_lossy(&out);
            let json_part = s.lines().next().unwrap();
            let v: serde_json::Value = serde_json::from_str(json_part).unwrap();
            assert_eq!(v["sample_rate"], 8000);
            assert_eq!(v["channels"], 1);
            assert_eq!(v["frames"], 800);
            assert_eq!(v["sample_count"], 800);
            assert_eq!(v["decoder"], "symphonia");

            // 7. info 元数据读取。
            let (code, out) = call(b"info\0", path.to_string_lossy().as_bytes(), 64 * 1024);
            assert_eq!(code, ErrorCode::Ok);
            let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
            assert_eq!(v["sample_rate"], 8000);
            assert_eq!(v["channels"], 1);
            assert_eq!(v["decoder"], "symphonia");
        }
    }

    /// 生成一个最小有效 WAV（8kHz、单声道、N 个采样）。
    fn write_wav(path: &std::path::Path, n: usize) {
        use std::io::Write;
        let sr = 8000u32;
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
}
