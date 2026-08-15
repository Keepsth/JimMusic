//! 统一错误类型与错误码规范。
//!
//! 核心内所有公开 API 的失败路径均返回 [`CoreResult<T>`]，其错误类型 [`CoreError`]
//! 按来源分层（插件、事件、IPFS、IO），并映射到 [`plugin_abi::ErrorCode`] 以便跨
//! FFI 边界传播。

use std::io;

use plugin_abi::ErrorCode;

/// 核心操作结果类型别名。
pub type CoreResult<T> = Result<T, CoreError>;

/// 核心统一错误。
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// 插件加载/调用相关错误。
    #[error("plugin error: {0}")]
    Plugin(#[from] crate::plugin::PluginError),

    /// 事件总线相关错误。
    #[error("event error: {0}")]
    Event(&'static str),

    /// IPFS 客户端错误。
    #[error("ipfs error: {0}")]
    Ipfs(#[from] crate::ipfs::IpfsError),

    /// 底层 IO 错误。
    #[error("io error: {0}")]
    Io(#[from] io::Error),

    /// 序列化错误。
    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

impl CoreError {
    /// 将自身映射到跨 FFI 边界的统一错误码。
    pub fn to_code(&self) -> ErrorCode {
        match self {
            CoreError::Plugin(e) => e.to_code(),
            CoreError::Event(_) => ErrorCode::Unknown,
            CoreError::Ipfs(_) => ErrorCode::InvokeFailed,
            CoreError::Io(_) => ErrorCode::LoadFailed,
            CoreError::Json(_) => ErrorCode::InvalidArgument,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_error_maps_to_load_failed() {
        let e: CoreError = io::Error::new(io::ErrorKind::NotFound, "nope").into();
        assert_eq!(e.to_code(), ErrorCode::LoadFailed);
        assert!(format!("{e}").contains("io error"));
    }

    #[test]
    fn json_error_maps_to_invalid_argument() {
        let e: CoreError = serde_json::from_str::<serde_json::Value>("not json")
            .err()
            .unwrap()
            .into();
        assert_eq!(e.to_code(), ErrorCode::InvalidArgument);
    }

    #[test]
    fn event_error_maps_to_unknown() {
        let e = CoreError::Event("boom");
        assert_eq!(e.to_code(), ErrorCode::Unknown);
    }
}
