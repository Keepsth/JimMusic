//! JimMusic UI 桥接插件。
//!
//! 按需求文档，UI 桥提供 FFI 接口与 Flutter 通信、事件总线（播放/暂停/进度回调）、
//! 资源管理（封面图、歌词同步）。本实现通过统一 C ABI 导出，支持：
//! - `ping`：连通性探测；
//! - `state`：查询最近一次同步的播放状态（JSON）；
//! - `on_state`：核心下发播放状态（0=停止 / 1=播放 / 2=暂停），经 `HostCtx.log` 回调转发；
//! - `on_progress`：核心下发播放进度（0.0~1.0，8 字节小端 f64），经 `HostCtx.progress` 回调转发。
//! - `on_error`：核心下发结构化 JSON 错误，经 `HostCtx.log` 以 error 级别转发。

// 导出的 C ABI 符号由核心经 `libloading` 加载调用，安全性由 ABI 契约保证，
// 统一豁免 missing_safety_doc 检查。
#![allow(clippy::missing_safety_doc)]

use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};

use plugin_abi::{ErrorCode, HostCtx, InvokeRequest, InvokeResponse, PluginInfo, PluginKind};

/// 进度回调类型。
type ProgressCb = unsafe extern "C" fn(user_data: *mut c_void, progress: f64);
/// 日志回调类型。
type LogCb = unsafe extern "C" fn(user_data: *mut c_void, level: i32, msg: *const u8);

static INITIALIZED: AtomicBool = AtomicBool::new(false);
/// 缓存最近一次 host ctx 的 user_data，用于回调。
static mut USER_DATA: *mut c_void = ptr::null_mut();
/// 缓存进度回调指针。
static mut PROGRESS_CB: Option<ProgressCb> = None;
/// 缓存日志回调指针。
static mut LOG_CB: Option<LogCb> = None;

/// 最近一次同步的播放状态：0=stopped, 1=playing, 2=paused。
static LAST_STATE: AtomicU8 = AtomicU8::new(0);
/// 最近一次同步的进度（f64 位模式）。
static LAST_PROGRESS: AtomicU64 = AtomicU64::new(0);

static INFO: PluginInfo = PluginInfo::from_static(
    b"ui-bridge\0",
    b"0.1.0\0",
    b"JimMusic Team\0",
    PluginKind::UiBridge,
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

/// 导出：初始化。缓存宿主回调（日志/进度）供后续事件转发使用。
///
/// # Safety
/// `ctx` 必须为有效指针（可为空）。
#[no_mangle]
pub unsafe extern "C" fn jimmusic_plugin_init(ctx: *mut HostCtx) -> ErrorCode {
    INITIALIZED.store(true, Ordering::SeqCst);
    if !ctx.is_null() {
        let host = unsafe { &*ctx };
        unsafe {
            USER_DATA = host.user_data;
            PROGRESS_CB = host.progress;
            LOG_CB = host.log;
        }
        if let Some(log) = host.log {
            let msg = b"ui-bridge initialized\0";
            unsafe { log(host.user_data, 1, msg.as_ptr()) };
        }
    }
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
    let input = if req.input.is_null() || req.input_len == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(req.input, req.input_len) }
    };

    let body: Vec<u8> = match op.as_ref() {
        "ping" => b"pong".to_vec(),
        "state" => current_state_json().into_bytes(),
        "on_progress" => match decode_progress(input) {
            Some(v) => {
                dispatch_progress(v);
                b"ok".to_vec()
            }
            None => return ErrorCode::InvalidArgument,
        },
        "on_state" => match decode_state(input) {
            Some(v) => {
                dispatch_state(v);
                b"ok".to_vec()
            }
            None => return ErrorCode::InvalidArgument,
        },
        "on_error" if !input.is_empty() => {
            dispatch_error(input);
            b"ok".to_vec()
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

/// 解析 8 字节小端 f64 进度。
fn decode_progress(input: &[u8]) -> Option<f64> {
    if input.len() != 8 {
        return None;
    }
    let arr: [u8; 8] = input.try_into().ok()?;
    let v = f64::from_le_bytes(arr);
    (0.0..=1.0).contains(&v).then_some(v)
}

/// 解析 1 字节播放状态（0/1/2）。
fn decode_state(input: &[u8]) -> Option<u8> {
    if input.len() != 1 {
        return None;
    }
    let v = input[0];
    (v <= 2).then_some(v)
}

/// 通过进度回调上报进度，并更新最近进度。
fn dispatch_progress(value: f64) {
    LAST_PROGRESS.store(value.to_bits(), Ordering::SeqCst);
    unsafe {
        if let Some(cb) = PROGRESS_CB {
            cb(USER_DATA, value);
        }
    }
}

/// 通过日志回调上报播放状态，并更新最近状态。
fn dispatch_state(value: u8) {
    LAST_STATE.store(value, Ordering::SeqCst);
    unsafe {
        if let Some(cb) = LOG_CB {
            let (msg, level): (&[u8], i32) = match value {
                1 => (b"playing\0", 1),
                2 => (b"paused\0", 1),
                _ => (b"stopped\0", 1),
            };
            cb(USER_DATA, level, msg.as_ptr());
        }
    }
}

fn dispatch_error(json: &[u8]) {
    LAST_STATE.store(0, Ordering::SeqCst);
    unsafe {
        if let Some(cb) = LOG_CB {
            let mut message = json.to_vec();
            message.push(0);
            cb(USER_DATA, 3, message.as_ptr());
        }
    }
}

/// 生成当前状态 JSON（供 `state` op 返回）。
fn current_state_json() -> String {
    let state = LAST_STATE.load(Ordering::SeqCst);
    let progress = f64::from_bits(LAST_PROGRESS.load(Ordering::SeqCst));
    let playing = state == 1;
    format!(r#"{{"playing":{playing},"progress":{progress}}}"#)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// 串行化共享静态状态的测试（避免并行执行竞态）。
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn decode_progress_valid() {
        assert_eq!(decode_progress(&0.5f64.to_le_bytes()), Some(0.5));
        assert_eq!(decode_progress(&[1, 2, 3]), None); // 长度错误
                                                       // 越界值（>1.0）应被拒绝。
        assert_eq!(decode_progress(&1.5f64.to_le_bytes()), None);
    }

    #[test]
    fn decode_state_valid() {
        assert_eq!(decode_state(&[0]), Some(0));
        assert_eq!(decode_state(&[1]), Some(1));
        assert_eq!(decode_state(&[2]), Some(2));
        assert_eq!(decode_state(&[3]), None);
        assert_eq!(decode_state(&[1, 2]), None);
    }

    #[test]
    fn state_json_reflects_dispatch() {
        let _g = TEST_LOCK.lock().unwrap();
        LAST_STATE.store(1, Ordering::SeqCst);
        LAST_PROGRESS.store(0.25f64.to_bits(), Ordering::SeqCst);
        assert_eq!(current_state_json(), r#"{"playing":true,"progress":0.25}"#);
    }

    // ---- invoke 派发路径（串行，避免共享静态状态在并行测试间竞态）----

    static mut CAPTURED_PROGRESS: f64 = -1.0;
    static mut CAPTURED_STATE: u8 = 255;

    unsafe extern "C" fn test_progress(_user_data: *mut c_void, progress: f64) {
        unsafe { CAPTURED_PROGRESS = progress };
    }

    unsafe extern "C" fn test_log(_user_data: *mut c_void, _level: i32, msg: *const u8) {
        // 仅捕获「播放中/暂停/停止」的首字节，用于断言。
        unsafe {
            let b = *msg;
            CAPTURED_STATE = match b {
                b'p' => {
                    if *msg.add(1) == b'a' {
                        2 // paused
                    } else {
                        1 // playing
                    }
                }
                b's' => 0, // stopped
                _ => 255,
            };
        }
    }

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

    #[test]
    fn invoke_dispatches_playback_events() {
        let _g = TEST_LOCK.lock().unwrap();
        unsafe {
            let mut ctx = HostCtx {
                log: Some(test_log),
                progress: Some(test_progress),
                user_data: ptr::null_mut(),
            };
            jimmusic_plugin_init(&mut ctx);
        }

        // ping
        let (code, out) = unsafe { call(b"ping\0", &[], 64) };
        assert_eq!(code, ErrorCode::Ok);
        assert_eq!(out, b"pong");

        // on_progress
        let (code, out) = unsafe { call(b"on_progress\0", &0.5f64.to_le_bytes(), 64) };
        assert_eq!(code, ErrorCode::Ok);
        assert_eq!(out, b"ok");
        assert_eq!(unsafe { CAPTURED_PROGRESS }, 0.5);

        // on_state (playing)
        let (code, _) = unsafe { call(b"on_state\0", &[1], 64) };
        assert_eq!(code, ErrorCode::Ok);
        assert_eq!(unsafe { CAPTURED_STATE }, 1);

        // state 反映最近派发
        let (_, out) = unsafe { call(b"state\0", &[], 64) };
        assert_eq!(out, br#"{"playing":true,"progress":0.5}"#.to_vec());
    }
}
