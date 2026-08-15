//! 插件签名校验（防篡改）。
//!
//! 使用 Ed25519 对插件内容摘要进行签名验证。流程：
//! 1. 计算下载内容的 SHA-256 摘要（见 [`crate::state::sha256_hex`]）；
//! 2. 发布者用其 Ed25519 私钥对摘要签名；
//! 3. 安装方用发布者公钥验证签名，防止内容被篡改或替换。

use ed25519_dalek::{Signature, Verifier, VerifyingKey};

/// 签名校验错误。
#[derive(Debug, thiserror::Error)]
pub enum SignatureError {
    /// 公钥十六进制解析失败。
    #[error("invalid public key: {0}")]
    InvalidPublicKey(String),
    /// 签名十六进制解析失败。
    #[error("invalid signature: {0}")]
    InvalidSignature(String),
    /// 签名验证失败（内容被篡改或签名不匹配）。
    #[error("signature verification failed: {0}")]
    VerificationFailed(String),
}

/// 校验 Ed25519 签名。
///
/// `public_key_hex` / `signature_hex` 为十六进制字符串；`message` 为被签名的内容
/// （通常为内容的 SHA-256 摘要字节）。
pub fn verify(
    public_key_hex: &str,
    signature_hex: &str,
    message: &[u8],
) -> Result<(), SignatureError> {
    let pk_bytes = hex::decode(public_key_hex)
        .map_err(|_| SignatureError::InvalidPublicKey(public_key_hex.to_string()))?;
    let sig_bytes = hex::decode(signature_hex)
        .map_err(|_| SignatureError::InvalidSignature(signature_hex.to_string()))?;

    let pk_arr: [u8; 32] = pk_bytes
        .try_into()
        .map_err(|_| SignatureError::InvalidPublicKey("wrong length".into()))?;
    let sig_arr: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| SignatureError::InvalidSignature("wrong length".into()))?;

    let vk = VerifyingKey::from_bytes(&pk_arr)
        .map_err(|e| SignatureError::InvalidPublicKey(e.to_string()))?;
    let sig = Signature::from_bytes(&sig_arr);

    vk.verify(message, &sig)
        .map_err(|e| SignatureError::VerificationFailed(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use getrandom::{rand_core::UnwrapErr, SysRng};

    #[test]
    fn valid_signature_verifies() {
        let mut csprng = UnwrapErr(SysRng);
        let signing_key = SigningKey::generate(&mut csprng);
        let vk = signing_key.verifying_key();
        let message = b"plugin-bytes-digest";
        let sig = signing_key.sign(message);

        // 正确签名应通过。
        verify(
            &hex::encode(vk.to_bytes()),
            &hex::encode(sig.to_bytes()),
            message,
        )
        .expect("valid signature should verify");

        // 篡改消息应失败。
        assert!(verify(
            &hex::encode(vk.to_bytes()),
            &hex::encode(sig.to_bytes()),
            b"tampered-message",
        )
        .is_err());
    }

    #[test]
    fn malformed_inputs_are_rejected() {
        assert!(matches!(
            verify("zz", "aa", b"m"),
            Err(SignatureError::InvalidPublicKey(_))
        ));
        assert!(verify(&hex::encode([0u8; 32]), "abcd", b"m").is_err());
    }
}
