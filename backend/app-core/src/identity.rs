//! 发布者身份密钥的加密封装、导出、导入、轮换与撤销。
//!
//! 私钥只以 Argon2id 派生密钥 + XChaCha20-Poly1305 密文落盘；错误口令或篡改统一失败，
//! 不回退到明文。平台适配层可把同一密文再放入系统安全存储。

use std::path::Path;

use argon2::{Algorithm, Argon2, Params, Version};
use base64::engine::general_purpose::STANDARD_NO_PAD;
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use ed25519_dalek::{Signer, SigningKey};
use jimmusic_protocol::{PublisherIdentityV1, Validate, SCHEMA_V1};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::publication_service::publisher_id_from_public_key;

const AAD: &[u8] = b"jimmusic:identity-export:v1";
const MEMORY_KIB: u32 = 64 * 1024;
const ITERATIONS: u32 = 3;
const PARALLELISM: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EncryptedIdentityBundleV1 {
    pub schema_version: u16,
    pub kdf: String,
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
    pub salt: String,
    pub nonce: String,
    pub ciphertext: String,
    pub public_key: String,
    /// Public identity metadata is carried beside the encrypted seed so imports
    /// preserve the stable identity CID, rotation chain and revocation state.
    /// Older v1 bundles did not include it and remain importable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<PublisherIdentityV1>,
}

#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("passphrase must contain at least 10 characters")]
    WeakPassphrase,
    #[error("identity bundle has an unsupported version or KDF")]
    UnsupportedBundle,
    #[error("identity bundle is malformed")]
    MalformedBundle,
    #[error("wrong passphrase or tampered identity bundle")]
    DecryptionFailed,
    #[error("secure random generation failed")]
    Random,
    #[error("identity IO failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("identity serialization failed: {0}")]
    Json(#[from] serde_json::Error),
}

pub struct PublisherIdentityVault {
    signing_key: SigningKey,
    identity: PublisherIdentityV1,
}

impl PublisherIdentityVault {
    pub fn generate(display_name: String, created_at: i64) -> Result<Self, IdentityError> {
        let mut seed = [0u8; 32];
        getrandom::fill(&mut seed).map_err(|_| IdentityError::Random)?;
        let signing_key = SigningKey::from_bytes(&seed);
        seed.zeroize();
        Ok(Self::from_signing_key(
            signing_key,
            display_name,
            created_at,
        ))
    }

    fn from_signing_key(signing_key: SigningKey, display_name: String, created_at: i64) -> Self {
        let public_key = hex::encode(signing_key.verifying_key().to_bytes());
        Self {
            identity: PublisherIdentityV1 {
                schema_version: SCHEMA_V1,
                publisher_id: publisher_id_from_public_key(&public_key),
                public_key,
                display_name,
                created_at,
                previous_key: None,
                rotation_proof: None,
                revoked_at: None,
                revocation_proof: None,
            },
            signing_key,
        }
    }

    pub fn identity(&self) -> &PublisherIdentityV1 {
        &self.identity
    }

    pub fn sign_hex(&self, message: &[u8]) -> String {
        hex::encode(self.signing_key.sign(message).to_bytes())
    }

    pub fn export(&self, passphrase: &str) -> Result<EncryptedIdentityBundleV1, IdentityError> {
        if passphrase.chars().count() < 10 {
            return Err(IdentityError::WeakPassphrase);
        }
        let mut salt = [0u8; 16];
        let mut nonce = [0u8; 24];
        getrandom::fill(&mut salt).map_err(|_| IdentityError::Random)?;
        getrandom::fill(&mut nonce).map_err(|_| IdentityError::Random)?;
        let mut derived = [0u8; 32];
        derive_key(passphrase, &salt, &mut derived)?;
        let cipher = XChaCha20Poly1305::new(Key::from_slice(&derived));
        let plaintext = self.signing_key.to_bytes();
        let ciphertext = cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: &plaintext,
                    aad: AAD,
                },
            )
            .map_err(|_| IdentityError::DecryptionFailed)?;
        derived.zeroize();
        Ok(EncryptedIdentityBundleV1 {
            schema_version: SCHEMA_V1,
            kdf: "argon2id".into(),
            memory_kib: MEMORY_KIB,
            iterations: ITERATIONS,
            parallelism: PARALLELISM,
            salt: STANDARD_NO_PAD.encode(salt),
            nonce: STANDARD_NO_PAD.encode(nonce),
            ciphertext: STANDARD_NO_PAD.encode(ciphertext),
            public_key: self.identity.public_key.clone(),
            identity: Some(self.identity.clone()),
        })
    }

    pub fn import(
        bundle: &EncryptedIdentityBundleV1,
        passphrase: &str,
        display_name: String,
        created_at: i64,
    ) -> Result<Self, IdentityError> {
        if bundle.schema_version != SCHEMA_V1
            || bundle.kdf != "argon2id"
            || bundle.memory_kib != MEMORY_KIB
            || bundle.iterations != ITERATIONS
            || bundle.parallelism != PARALLELISM
        {
            return Err(IdentityError::UnsupportedBundle);
        }
        let salt = decode_array::<16>(&bundle.salt)?;
        let nonce = decode_array::<24>(&bundle.nonce)?;
        let ciphertext = STANDARD_NO_PAD
            .decode(&bundle.ciphertext)
            .map_err(|_| IdentityError::MalformedBundle)?;
        let mut derived = [0u8; 32];
        derive_key(passphrase, &salt, &mut derived)?;
        let cipher = XChaCha20Poly1305::new(Key::from_slice(&derived));
        let mut plaintext = cipher
            .decrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: &ciphertext,
                    aad: AAD,
                },
            )
            .map_err(|_| IdentityError::DecryptionFailed)?;
        derived.zeroize();
        let seed: [u8; 32] = plaintext
            .as_slice()
            .try_into()
            .map_err(|_| IdentityError::MalformedBundle)?;
        let mut vault =
            Self::from_signing_key(SigningKey::from_bytes(&seed), display_name, created_at);
        plaintext.zeroize();
        if vault.identity.public_key != bundle.public_key {
            return Err(IdentityError::DecryptionFailed);
        }
        if let Some(identity) = &bundle.identity {
            identity
                .validate()
                .map_err(|_| IdentityError::MalformedBundle)?;
            if identity.public_key != bundle.public_key
                || identity.publisher_id != publisher_id_from_public_key(&bundle.public_key)
            {
                return Err(IdentityError::DecryptionFailed);
            }
            vault.identity = identity.clone();
        }
        Ok(vault)
    }

    /// Export the same signing seed together with an updated public identity,
    /// used after signing a revocation without ever exposing private material.
    pub fn export_with_identity(
        &self,
        passphrase: &str,
        identity: PublisherIdentityV1,
    ) -> Result<EncryptedIdentityBundleV1, IdentityError> {
        if identity.public_key != self.identity.public_key
            || identity.publisher_id != publisher_id_from_public_key(&identity.public_key)
        {
            return Err(IdentityError::DecryptionFailed);
        }
        identity
            .validate()
            .map_err(|_| IdentityError::MalformedBundle)?;
        let mut bundle = self.export(passphrase)?;
        bundle.identity = Some(identity);
        Ok(bundle)
    }

    pub fn save_encrypted(&self, path: &Path, passphrase: &str) -> Result<(), IdentityError> {
        let bundle = self.export(passphrase)?;
        let bytes = serde_json::to_vec_pretty(&bundle)?;
        write_private_atomic(path, &bytes)?;
        Ok(())
    }

    pub fn load_encrypted(
        path: &Path,
        passphrase: &str,
        display_name: String,
        created_at: i64,
    ) -> Result<Self, IdentityError> {
        let bundle: EncryptedIdentityBundleV1 = serde_json::from_slice(&std::fs::read(path)?)?;
        Self::import(&bundle, passphrase, display_name, created_at)
    }

    /// 生成新身份，并由旧密钥对「旧公钥 -> 新公钥」关系签名。
    pub fn rotate(&self, display_name: String, created_at: i64) -> Result<Self, IdentityError> {
        let mut next = Self::generate(display_name, created_at)?;
        let proof_message =
            rotation_proof_message(&self.identity.public_key, &next.identity.public_key);
        next.identity.previous_key = Some(self.identity.public_key.clone());
        next.identity.rotation_proof = Some(self.sign_hex(&proof_message));
        Ok(next)
    }

    pub fn revoked_identity(&self, revoked_at: i64) -> PublisherIdentityV1 {
        let mut identity = self.identity.clone();
        let message = revocation_proof_message(&identity.publisher_id, revoked_at);
        identity.revoked_at = Some(revoked_at);
        identity.revocation_proof = Some(self.sign_hex(&message));
        identity
    }
}

fn derive_key(passphrase: &str, salt: &[u8], output: &mut [u8; 32]) -> Result<(), IdentityError> {
    let params = Params::new(MEMORY_KIB, ITERATIONS, PARALLELISM, Some(32))
        .map_err(|_| IdentityError::UnsupportedBundle)?;
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase.as_bytes(), salt, output)
        .map_err(|_| IdentityError::DecryptionFailed)
}

fn decode_array<const N: usize>(encoded: &str) -> Result<[u8; N], IdentityError> {
    STANDARD_NO_PAD
        .decode(encoded)
        .map_err(|_| IdentityError::MalformedBundle)?
        .try_into()
        .map_err(|_| IdentityError::MalformedBundle)
}

pub fn rotation_proof_message(previous: &str, next: &str) -> Vec<u8> {
    let mut message = b"jimmusic:identity-rotation:v1\0".to_vec();
    message.extend_from_slice(previous.as_bytes());
    message.push(0);
    message.extend_from_slice(next.as_bytes());
    message
}

pub fn revocation_proof_message(publisher_id: &str, revoked_at: i64) -> Vec<u8> {
    let mut message = b"jimmusic:identity-revocation:v1\0".to_vec();
    message.extend_from_slice(publisher_id.as_bytes());
    message.extend_from_slice(&revoked_at.to_le_bytes());
    message
}

fn write_private_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("tmp");
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::verify_ed25519_hex;

    #[test]
    fn encrypted_export_round_trips_and_rejects_wrong_password_or_tampering() {
        let vault = PublisherIdentityVault::generate("Artist".into(), 1).unwrap();
        let bundle = vault.export("correct horse battery").unwrap();
        assert_eq!(bundle.identity.as_ref(), Some(vault.identity()));
        let restored =
            PublisherIdentityVault::import(&bundle, "correct horse battery", "Artist".into(), 1)
                .unwrap();
        assert_eq!(restored.identity().public_key, vault.identity().public_key);
        assert_eq!(restored.identity(), vault.identity());
        let message = b"signed";
        assert!(verify_ed25519_hex(
            &restored.identity().public_key,
            &restored.sign_hex(message),
            message
        )
        .is_ok());
        assert!(matches!(
            PublisherIdentityVault::import(&bundle, "wrong password", "A".into(), 1),
            Err(IdentityError::DecryptionFailed)
        ));
        let mut tampered = bundle;
        tampered.ciphertext.push('a');
        assert!(
            PublisherIdentityVault::import(&tampered, "correct horse battery", "A".into(), 1)
                .is_err()
        );
    }

    #[test]
    fn saved_file_contains_no_plaintext_private_seed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("identity.json");
        let vault = PublisherIdentityVault::generate("Artist".into(), 1).unwrap();
        let seed_hex = hex::encode(vault.signing_key.to_bytes());
        vault
            .save_encrypted(&path, "correct horse battery")
            .unwrap();
        let disk = std::fs::read_to_string(&path).unwrap();
        assert!(!disk.contains(&seed_hex));
        let restored = PublisherIdentityVault::load_encrypted(
            &path,
            "correct horse battery",
            "Artist".into(),
            1,
        )
        .unwrap();
        assert_eq!(restored.identity().public_key, vault.identity().public_key);
    }

    #[test]
    fn rotation_and_revocation_are_signed() {
        let vault = PublisherIdentityVault::generate("Artist".into(), 1).unwrap();
        let next = vault.rotate("Artist".into(), 2).unwrap();
        let proof = next.identity().rotation_proof.as_deref().unwrap();
        let message = rotation_proof_message(
            next.identity().previous_key.as_deref().unwrap(),
            &next.identity().public_key,
        );
        assert!(verify_ed25519_hex(&vault.identity().public_key, proof, &message).is_ok());
        let revoked = vault.revoked_identity(3);
        assert_eq!(revoked.revoked_at, Some(3));
        assert!(revoked.revocation_proof.is_some());
    }
}
