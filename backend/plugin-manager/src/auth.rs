//! 节点认证：API 令牌鉴权（对应需求 4.2「libp2p 节点认证」的 HTTP 控制面部分）。
//!
//! 提供常量时间比较与 Bearer token 解析，防止时序侧信道攻击。实际 libp2p 网络栈的
//! 节点身份认证在插件管理器的 HTTP 控制面以 API 令牌形式落地。

/// 常量时间字节比较，防时序侧信道。
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// 校验提供的 Bearer token 是否通过。
///
/// - `configured` 为 `None`：未启用鉴权，放行；
/// - 否则要求 `provided == configured`（常量时间比较）。
pub fn authorize_token(configured: Option<&str>, provided: Option<&str>) -> bool {
    match configured {
        None => true,
        Some(secret) => match provided {
            Some(p) => constant_time_eq(secret.as_bytes(), p.as_bytes()),
            None => false,
        },
    }
}

/// 从 `Authorization` 头解析 Bearer token。
pub fn bearer_from_header(value: Option<&str>) -> Option<&str> {
    let v = value?.trim();
    v.strip_prefix("Bearer ")
        .or_else(|| v.strip_prefix("bearer "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_basics() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab")); // 长度不同
        assert!(!constant_time_eq(b"", b"a"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn authorize_without_configured_token_allows() {
        assert!(authorize_token(None, None));
        assert!(authorize_token(None, Some("anything")));
    }

    #[test]
    fn authorize_with_token_requires_match() {
        let secret = Some("s3cret");
        assert!(authorize_token(secret, Some("s3cret")));
        assert!(!authorize_token(secret, Some("wrong")));
        assert!(!authorize_token(secret, None));
    }

    #[test]
    fn bearer_parsing() {
        assert_eq!(bearer_from_header(Some("Bearer abc")), Some("abc"));
        assert_eq!(bearer_from_header(Some("bearer abc")), Some("abc"));
        assert_eq!(bearer_from_header(Some("abc")), None);
        assert_eq!(bearer_from_header(None), None);
    }
}
