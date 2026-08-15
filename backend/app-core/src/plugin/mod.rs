//! 插件加载、管理与调用。
//!
//! [`PluginManager`] 负责运行时动态发现、加载、卸载插件动态库（`.so`/`.dll`/`.dylib`），
//! 校验 ABI 版本，并通过 [`plugin_abi`] 定义的统一 C ABI 调用插件能力，实现插件的
//! 热插拔（无需重启主程序）。

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::path::{Path, PathBuf};
use std::ptr;

use plugin_abi::{ErrorCode, HostCtx, InvokeRequest, InvokeResponse, PluginInfo, PluginKind};

/// 插件相关错误。
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    /// 动态库加载失败。
    #[error("failed to load library `{0}`: {1}")]
    Load(String, String),
    /// 符号查找失败。
    #[error("symbol `{0}` not found")]
    SymbolNotFound(String),
    /// ABI 版本不匹配。
    #[error("ABI version mismatch: expected {expected}, got {actual}")]
    AbiMismatch { expected: u32, actual: u32 },
    /// 插件未加载或已卸载。
    #[error("plugin `{0}` not found")]
    NotFound(String),
    /// 插件初始化失败。
    #[error("plugin init failed with code {0}")]
    InitFailed(i32),
    /// 插件调用失败。
    #[error("plugin invoke failed with code {0}")]
    InvokeFailed(i32),
    /// 无效库文件名（无平台后缀）。
    #[error("unsupported library filename `{0}`")]
    UnsupportedFileName(String),
    /// 非法 UTF-8 操作名。
    #[error("invalid operation name: {0}")]
    InvalidOp(String),
}

impl PluginError {
    /// 映射为跨 FFI 边界的统一错误码。
    pub fn to_code(&self) -> ErrorCode {
        match self {
            PluginError::Load(..) => ErrorCode::LoadFailed,
            PluginError::SymbolNotFound(_) => ErrorCode::SymbolNotFound,
            PluginError::AbiMismatch { .. } => ErrorCode::AbiMismatch,
            PluginError::NotFound(_) => ErrorCode::NotFound,
            PluginError::InitFailed(_) => ErrorCode::InvokeFailed,
            PluginError::InvokeFailed(_) => ErrorCode::InvokeFailed,
            PluginError::UnsupportedFileName(_) => ErrorCode::Unsupported,
            PluginError::InvalidOp(_) => ErrorCode::InvalidArgument,
        }
    }
}

// 热点符号名。
const SYM_ABI_VERSION: &[u8] = b"jimmusic_abi_version\0";
const SYM_PLUGIN_INFO: &[u8] = b"jimmusic_plugin_info\0";
const SYM_PLUGIN_INIT: &[u8] = b"jimmusic_plugin_init\0";
const SYM_PLUGIN_SHUTDOWN: &[u8] = b"jimmusic_plugin_shutdown\0";
const SYM_PLUGIN_INVOKE: &[u8] = b"jimmusic_plugin_invoke\0";

/// 插件元数据（宿主侧视图，已从 C 字符串拷贝为自有 [`String`]）。
#[derive(Debug, Clone)]
pub struct PluginMeta {
    /// 插件名。
    pub name: String,
    /// 插件版本。
    pub version: String,
    /// 插件作者。
    pub author: String,
    /// 插件种类。
    pub kind: PluginKind,
    /// 动态库磁盘路径。
    pub path: PathBuf,
}

/// 已加载插件的函数表。所有字段均将符号借用延长到 `'static`；
/// 其生命周期由 [`LoadedPlugin::_library`] 保证（字段 drop 顺序：先 `symbols` 后 `_library`）。
struct PluginSymbols {
    abi_version: libloading::Symbol<'static, unsafe extern "C" fn() -> u32>,
    plugin_info: libloading::Symbol<'static, unsafe extern "C" fn() -> *const PluginInfo>,
    plugin_init: libloading::Symbol<'static, unsafe extern "C" fn(*mut HostCtx) -> ErrorCode>,
    plugin_shutdown: libloading::Symbol<'static, unsafe extern "C" fn()>,
    plugin_invoke: libloading::Symbol<
        'static,
        unsafe extern "C" fn(*const InvokeRequest, *mut InvokeResponse) -> ErrorCode,
    >,
}

/// 一个已加载到内存的插件实例。支持热插拔：drop 后动态库即被卸载。
pub struct LoadedPlugin {
    meta: PluginMeta,
    symbols: PluginSymbols,
    // 必须在 `symbols` 之后 drop，保证函数指针存活到释放之前。
    _library: libloading::Library,
}

impl LoadedPlugin {
    /// 返回插件元数据引用。
    pub fn meta(&self) -> &PluginMeta {
        &self.meta
    }

    /// 以字符串操作名与原始字节输入调用插件能力，返回输出字节。
    pub fn invoke(&self, op: &str, input: &[u8]) -> Result<Vec<u8>, PluginError> {
        let op_c = CString::new(op).map_err(|_| PluginError::InvalidOp(op.to_string()))?;
        let request = InvokeRequest {
            op: op_c.as_ptr().cast(),
            input: if input.is_empty() {
                ptr::null()
            } else {
                input.as_ptr()
            },
            input_len: input.len(),
        };

        // 输出缓冲：固定容量，调用方可按需扩充。
        let mut output = vec![0u8; 64 * 1024];
        let mut response = InvokeResponse {
            output: output.as_mut_ptr(),
            capacity: output.len(),
            written: 0,
        };

        // SAFETY: request/response 均为合法 repr(C) 结构，指针在本调用期间有效；
        // 函数指针来自仍存活的库。
        let code = unsafe { (self.symbols.plugin_invoke)(&request, &mut response) };
        if code != ErrorCode::Ok {
            return Err(PluginError::InvokeFailed(code.as_i32()));
        }

        output.truncate(response.written.min(output.len()));
        Ok(output)
    }
}

impl Drop for LoadedPlugin {
    fn drop(&mut self) {
        // SAFETY: 函数指针来自 `_library`（仍存活）；`_library` 声明在 `symbols` 之后，
        // 故 drop 顺序为先 `_library` 后 `symbols`？—— 注意：Rust 结构体 drop 顺序为
        // 字段声明顺序，`_library` 在 `symbols` 之后声明，会先 drop `_library`。
        // 因此这里显式在库卸载前调用 shutdown 是安全且必要的。
        unsafe { (self.symbols.plugin_shutdown)() };
    }
}

/// 插件管理器：发现、加载、卸载插件，维护已加载插件注册表。
#[derive(Default)]
pub struct PluginManager {
    plugins: HashMap<String, LoadedPlugin>,
}

impl PluginManager {
    /// 创建空管理器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 按名称查询已加载插件。
    pub fn get(&self, name: &str) -> Option<&LoadedPlugin> {
        self.plugins.get(name)
    }

    /// 已加载插件数量。
    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// 所有已加载插件的元数据快照。
    pub fn list(&self) -> Vec<PluginMeta> {
        self.plugins.values().map(|p| p.meta.clone()).collect()
    }

    /// 动态加载单个插件库文件。
    pub fn load(&mut self, path: impl AsRef<Path>) -> Result<&LoadedPlugin, PluginError> {
        let path = path.as_ref();
        validate_library_name(path)?;

        // SAFETY: 打开操作系统动态库。
        let library = unsafe { libloading::Library::new(path) }
            .map_err(|e| PluginError::Load(path.display().to_string(), e.to_string()))?;

        let result = self.load_from_library(path, library);
        if result.is_err() {
            tracing::warn!("plugin load failed: {}", path.display());
        }
        result
    }

    fn load_from_library(
        &mut self,
        path: &Path,
        library: libloading::Library,
    ) -> Result<&LoadedPlugin, PluginError> {
        // 查找符号（借用 library）。
        let symbols = unsafe { lookup_symbols(&library)? };

        // 校验 ABI 版本。
        // SAFETY: 函数指针来自已加载库且签名正确。
        let actual = unsafe { (*symbols.abi_version)() };
        if actual != plugin_abi::ABI_VERSION {
            return Err(PluginError::AbiMismatch {
                expected: plugin_abi::ABI_VERSION,
                actual,
            });
        }

        // 读取并拷贝元数据。
        // SAFETY: plugin_info 返回指向插件静态数据的只读指针。
        let info_ptr = unsafe { (*symbols.plugin_info)() };
        if info_ptr.is_null() {
            return Err(PluginError::SymbolNotFound("jimmusic_plugin_info".into()));
        }
        // SAFETY: info_ptr 非空且指向有效 PluginInfo。
        let info = unsafe { &*info_ptr };
        let meta = PluginMeta {
            name: unsafe { cstr_to_string(info.name) },
            version: unsafe { cstr_to_string(info.version) },
            author: unsafe { cstr_to_string(info.author) },
            kind: info.kind,
            path: path.to_path_buf(),
        };

        if meta.name.is_empty() {
            return Err(PluginError::SymbolNotFound("empty plugin name".into()));
        }

        // 初始化插件（原型阶段不提供宿主回调）。
        let ctx = HostCtx {
            log: None,
            progress: None,
            user_data: ptr::null_mut(),
        };
        // SAFETY: ctx 为合法 repr(C) 结构，且被函数作为只读/借用使用。
        let init_code = unsafe { (*symbols.plugin_init)(&ctx as *const HostCtx as *mut HostCtx) };
        if init_code != ErrorCode::Ok {
            return Err(PluginError::InitFailed(init_code.as_i32()));
        }

        let plugin = LoadedPlugin {
            meta: meta.clone(),
            symbols,
            _library: library,
        };

        tracing::info!(
            "loaded plugin `{}` v{} ({:?})",
            meta.name,
            meta.version,
            meta.kind
        );
        self.plugins.insert(meta.name.clone(), plugin);
        Ok(self.plugins.get(&meta.name).expect("just inserted"))
    }

    /// 按名称卸载插件，释放动态库。
    pub fn unload(&mut self, name: &str) -> Result<(), PluginError> {
        match self.plugins.remove(name) {
            Some(plugin) => {
                tracing::info!("unloaded plugin `{name}`");
                drop(plugin); // drop 触发 shutdown 并卸载库
                Ok(())
            }
            None => Err(PluginError::NotFound(name.to_string())),
        }
    }

    /// 卸载全部插件。
    pub fn unload_all(&mut self) {
        let names: Vec<String> = self.plugins.keys().cloned().collect();
        for name in names {
            let _ = self.unload(&name);
        }
    }
}

/// 校验动态库文件名是否带平台后缀。
fn validate_library_name(path: &Path) -> Result<(), PluginError> {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let ok = name.ends_with(".so") || name.ends_with(".dll") || name.ends_with(".dylib");
    if ok {
        Ok(())
    } else {
        Err(PluginError::UnsupportedFileName(name.to_string()))
    }
}

/// 从 C 字符串指针安全拷贝为 Rust [`String`]（空指针返回空串）。
///
/// # Safety
/// 调用方必须保证 `ptr` 为 NUL 结尾的有效字节串或空指针。
unsafe fn cstr_to_string(ptr: *const u8) -> String {
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: 调用方保证指针有效。
    unsafe { CStr::from_ptr(ptr.cast()) }
        .to_string_lossy()
        .into_owned()
}

/// 从库中查找全部导出符号并延长生命周期。
///
/// # Safety
/// `library` 必须在返回的 `PluginSymbols` 存活期间保持有效（由 `LoadedPlugin` 保证）。
unsafe fn lookup_symbols(library: &libloading::Library) -> Result<PluginSymbols, PluginError> {
    // SAFETY: 每个 get 均从同一 library 读取；随后 extend_lifetime 将借用延长为 'static，
    // 生命周期由 LoadedPlugin::_library 保证。
    let abi_version = unsafe {
        library
            .get::<unsafe extern "C" fn() -> u32>(SYM_ABI_VERSION)
            .map(extend_lifetime)
            .map_err(|_| PluginError::SymbolNotFound("jimmusic_abi_version".into()))?
    };
    let plugin_info = unsafe {
        library
            .get::<unsafe extern "C" fn() -> *const PluginInfo>(SYM_PLUGIN_INFO)
            .map(extend_lifetime)
            .map_err(|_| PluginError::SymbolNotFound("jimmusic_plugin_info".into()))?
    };
    let plugin_init = unsafe {
        library
            .get::<unsafe extern "C" fn(*mut HostCtx) -> ErrorCode>(SYM_PLUGIN_INIT)
            .map(extend_lifetime)
            .map_err(|_| PluginError::SymbolNotFound("jimmusic_plugin_init".into()))?
    };
    let plugin_shutdown = unsafe {
        library
            .get::<unsafe extern "C" fn()>(SYM_PLUGIN_SHUTDOWN)
            .map(extend_lifetime)
            .map_err(|_| PluginError::SymbolNotFound("jimmusic_plugin_shutdown".into()))?
    };
    let plugin_invoke = unsafe {
        library
            .get::<unsafe extern "C" fn(*const InvokeRequest, *mut InvokeResponse) -> ErrorCode>(
                SYM_PLUGIN_INVOKE,
            )
            .map(extend_lifetime)
            .map_err(|_| PluginError::SymbolNotFound("jimmusic_plugin_invoke".into()))?
    };

    Ok(PluginSymbols {
        abi_version,
        plugin_info,
        plugin_init,
        plugin_shutdown,
        plugin_invoke,
    })
}

/// 将借用生命周期的符号延长为 `'static`。
///
/// 该函数是**安全**的：它不执行任何解引用，仅调整生命周期参数。
/// 安全性由调用方 `lookup_symbols`（`unsafe fn`）保证底层库在返回符号存活期间有效。
fn extend_lifetime<'a, T>(s: libloading::Symbol<'a, T>) -> libloading::Symbol<'static, T> {
    // SAFETY: `Symbol` 的底层数据（库句柄 + 函数指针）在 `'static` 期间有效，前提是
    // 调用方保证库不被卸载（由 `LoadedPlugin::_library` 持有）。此处仅延长借用期，
    // 不改变内存表示。
    unsafe { std::mem::transmute::<libloading::Symbol<'a, T>, libloading::Symbol<'static, T>>(s) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manager_new_is_empty() {
        let m = PluginManager::new();
        assert!(m.is_empty());
        assert_eq!(m.len(), 0);
    }

    #[test]
    fn validate_rejects_non_library() {
        assert!(validate_library_name(Path::new("foo.txt")).is_err());
        assert!(validate_library_name(Path::new("libfoo.so")).is_ok());
        assert!(validate_library_name(Path::new("foo.dll")).is_ok());
        assert!(validate_library_name(Path::new("foo.dylib")).is_ok());
    }

    #[test]
    fn unload_missing_returns_not_found() {
        let mut m = PluginManager::new();
        assert!(matches!(m.unload("nope"), Err(PluginError::NotFound(_))));
    }

    #[test]
    fn error_maps_to_code() {
        assert_eq!(
            PluginError::NotFound("x".into()).to_code(),
            ErrorCode::NotFound
        );
        assert_eq!(
            PluginError::Load("p".into(), "e".into()).to_code(),
            ErrorCode::LoadFailed
        );
    }
}
