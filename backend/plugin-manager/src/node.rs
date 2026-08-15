//! libp2p 节点身份认证（对应需求 4.2「libp2p 节点认证」）。
//!
//! 基于 `libp2p-identity` 的 Ed25519 密钥对与 PeerId：
//! - 每个节点持有自己的 Ed25519 密钥对，并派生 libp2p [`PeerId`] 作为节点标识；
//! - 节点间通过「消息签名 + 公钥验签」完成身份认证——只有持有某 PeerId 对应
//!   私钥的节点才能产出可通过该 PeerId 公钥验签的签名。

use std::path::Path;

use libp2p_identity::{Keypair, PeerId, PublicKey, SigningError};

/// 节点身份：Ed25519 密钥对 + 派生 PeerId。
pub struct NodeIdentity {
    keypair: Keypair,
    peer_id: PeerId,
}

impl NodeIdentity {
    /// 生成一个新的随机节点身份。
    pub fn generate() -> Self {
        let keypair = Keypair::generate_ed25519();
        let peer_id = PeerId::from_public_key(&keypair.public());
        Self { keypair, peer_id }
    }

    /// Load a stable node key from disk or atomically create a private 0600
    /// key file. A repository must never advertise a persisted PeerId while
    /// silently replacing the corresponding signing key on every launch.
    pub fn load_or_create(path: &Path) -> std::io::Result<Self> {
        if path.exists() {
            let bytes = std::fs::read(path)?;
            let keypair = Keypair::from_protobuf_encoding(&bytes)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
            let peer_id = PeerId::from_public_key(&keypair.public());
            return Ok(Self { keypair, peer_id });
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let identity = Self::generate();
        let bytes = identity
            .keypair
            .to_protobuf_encoding()
            .map_err(std::io::Error::other)?;
        let temporary = path.with_extension("tmp");
        let mut options = std::fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        {
            use std::io::Write;
            let mut file = options.open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        std::fs::rename(&temporary, path)?;
        Ok(identity)
    }

    /// 节点 PeerId（libp2p 节点标识）。
    pub fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    /// 节点 PeerId 的 base58 字符串。
    pub fn peer_id_str(&self) -> String {
        self.peer_id.to_base58()
    }

    /// Private protobuf encoding consumed by the embedded libp2p runtime. The
    /// value never crosses the process boundary and is not included in any
    /// diagnostics or API response.
    pub(crate) fn private_key_protobuf(&self) -> std::io::Result<Vec<u8>> {
        self.keypair
            .to_protobuf_encoding()
            .map_err(std::io::Error::other)
    }

    /// 节点公钥（可导出给对端用于验签）。
    pub fn public_key(&self) -> PublicKey {
        self.keypair.public()
    }

    /// 对消息签名（节点间认证：证明持有本节点私钥）。
    pub fn sign(&self, msg: &[u8]) -> Result<Vec<u8>, SigningError> {
        self.keypair.sign(msg)
    }

    /// 用给定公钥验签（认证对端是否持有对应私钥）。
    pub fn verify_with(public: &PublicKey, msg: &[u8], sig: &[u8]) -> bool {
        public.verify(msg, sig)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_identity_signs_and_verifies() {
        let a = NodeIdentity::generate();
        let b = NodeIdentity::generate();

        // PeerId 唯一、稳定（base58 字符串非空）。
        assert_ne!(a.peer_id(), b.peer_id());
        assert!(!a.peer_id_str().is_empty());

        // A 签名 → 用 A 公钥验签成功，用 B 公钥验签失败。
        let msg = b"authenticate node";
        let sig = a.sign(msg).unwrap();
        assert!(NodeIdentity::verify_with(&a.public_key(), msg, &sig));
        assert!(!NodeIdentity::verify_with(&b.public_key(), msg, &sig));

        // 篡改消息后验签失败。
        assert!(!NodeIdentity::verify_with(
            &a.public_key(),
            b"tampered",
            &sig
        ));
    }

    #[test]
    fn node_identity_is_stable_across_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("node-key.pb");
        let first = NodeIdentity::load_or_create(&path).unwrap();
        let peer_id = first.peer_id();
        let signature = first.sign(b"restart-proof").unwrap();
        drop(first);
        let restored = NodeIdentity::load_or_create(&path).unwrap();
        assert_eq!(restored.peer_id(), peer_id);
        assert!(NodeIdentity::verify_with(
            &restored.public_key(),
            b"restart-proof",
            &signature
        ));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
