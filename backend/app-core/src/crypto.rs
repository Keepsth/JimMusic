//! 可信微内核使用的签名域分离与 Ed25519 验签。

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use jimmusic_protocol::{canonical_dag_cbor, decode_dag_cbor, ModerationReportV1, Validate};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SignatureError {
    #[error("invalid public key encoding")]
    InvalidPublicKey,
    #[error("invalid signature encoding")]
    InvalidSignature,
    #[error("signature verification failed")]
    VerificationFailed,
}

pub fn verify_ed25519_hex(
    public_key_hex: &str,
    signature_hex: &str,
    message: &[u8],
) -> Result<(), SignatureError> {
    let public_key: [u8; 32] = hex::decode(public_key_hex)
        .map_err(|_| SignatureError::InvalidPublicKey)?
        .try_into()
        .map_err(|_| SignatureError::InvalidPublicKey)?;
    let signature: [u8; 64] = hex::decode(signature_hex)
        .map_err(|_| SignatureError::InvalidSignature)?
        .try_into()
        .map_err(|_| SignatureError::InvalidSignature)?;
    let verifying_key =
        VerifyingKey::from_bytes(&public_key).map_err(|_| SignatureError::InvalidPublicKey)?;
    verifying_key
        .verify(message, &Signature::from_bytes(&signature))
        .map_err(|_| SignatureError::VerificationFailed)
}

const REPORT_ENVELOPE_ALGORITHM: &str = "x25519-xchacha20poly1305-v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModerationEnvelope {
    schema_version: u16,
    algorithm: String,
    report_id: String,
    recipient_source_id: String,
    ephemeral_public_key: String,
    nonce: String,
    ciphertext: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ReportEncryptionError {
    #[error("invalid X25519 key encoding")]
    InvalidKey,
    #[error("secure random generation failed")]
    Random,
    #[error("moderation report encoding failed: {0}")]
    Encoding(String),
    #[error("moderation envelope is invalid")]
    InvalidEnvelope,
    #[error("moderation envelope authentication failed")]
    Authentication,
    #[error("decrypted moderation report is invalid: {0}")]
    InvalidReport(String),
}

/// Encrypts the complete signed report for a community's X25519 recipient key.
/// The returned JSON envelope exposes only routing identifiers, never the
/// target, reason, evidence, reporter identity, or reporter signing key.
pub fn encrypt_moderation_report(
    recipient_public_key_hex: &str,
    report: &ModerationReportV1,
) -> Result<String, ReportEncryptionError> {
    report
        .validate()
        .map_err(|error| ReportEncryptionError::InvalidReport(error.to_string()))?;
    let recipient = PublicKey::from(parse_key(recipient_public_key_hex)?);
    let mut ephemeral_bytes = [0u8; 32];
    getrandom::fill(&mut ephemeral_bytes).map_err(|_| ReportEncryptionError::Random)?;
    let ephemeral_secret = StaticSecret::from(ephemeral_bytes);
    ephemeral_bytes.fill(0);
    let ephemeral_public = PublicKey::from(&ephemeral_secret);
    let shared = ephemeral_secret.diffie_hellman(&recipient);
    let key = derive_report_key(
        shared.as_bytes(),
        ephemeral_public.as_bytes(),
        recipient.as_bytes(),
    );
    let mut nonce = [0u8; 24];
    getrandom::fill(&mut nonce).map_err(|_| ReportEncryptionError::Random)?;
    let mut plaintext_report = report.clone();
    plaintext_report.encrypted_envelope = None;
    let plaintext = canonical_dag_cbor(&plaintext_report)
        .map_err(|error| ReportEncryptionError::Encoding(error.to_string()))?;
    let aad = report_aad(&report.report_id, &report.recipient_source_id);
    let cipher = XChaCha20Poly1305::new((&key).into());
    let ciphertext = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: &plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| ReportEncryptionError::Authentication)?;
    serde_json::to_string(&ModerationEnvelope {
        schema_version: 1,
        algorithm: REPORT_ENVELOPE_ALGORITHM.into(),
        report_id: report.report_id.clone(),
        recipient_source_id: report.recipient_source_id.clone(),
        ephemeral_public_key: URL_SAFE_NO_PAD.encode(ephemeral_public.as_bytes()),
        nonce: URL_SAFE_NO_PAD.encode(nonce),
        ciphertext: URL_SAFE_NO_PAD.encode(ciphertext),
    })
    .map_err(|error| ReportEncryptionError::Encoding(error.to_string()))
}

/// Recipient-side counterpart used by a community report processor and by
/// interoperability tests. Any envelope/routing/ciphertext tampering fails.
pub fn decrypt_moderation_report(
    recipient_private_key_hex: &str,
    envelope_json: &str,
) -> Result<ModerationReportV1, ReportEncryptionError> {
    let envelope: ModerationEnvelope =
        serde_json::from_str(envelope_json).map_err(|_| ReportEncryptionError::InvalidEnvelope)?;
    if envelope.schema_version != 1 || envelope.algorithm != REPORT_ENVELOPE_ALGORITHM {
        return Err(ReportEncryptionError::InvalidEnvelope);
    }
    let secret = StaticSecret::from(parse_key(recipient_private_key_hex)?);
    let recipient_public = PublicKey::from(&secret);
    let ephemeral_public = PublicKey::from(decode_array::<32>(&envelope.ephemeral_public_key)?);
    let nonce = decode_array::<24>(&envelope.nonce)?;
    let ciphertext = URL_SAFE_NO_PAD
        .decode(&envelope.ciphertext)
        .map_err(|_| ReportEncryptionError::InvalidEnvelope)?;
    let shared = secret.diffie_hellman(&ephemeral_public);
    let key = derive_report_key(
        shared.as_bytes(),
        ephemeral_public.as_bytes(),
        recipient_public.as_bytes(),
    );
    let aad = report_aad(&envelope.report_id, &envelope.recipient_source_id);
    let plaintext = XChaCha20Poly1305::new((&key).into())
        .decrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: &ciphertext,
                aad: &aad,
            },
        )
        .map_err(|_| ReportEncryptionError::Authentication)?;
    let mut report: ModerationReportV1 = decode_dag_cbor(&plaintext)
        .map_err(|error| ReportEncryptionError::InvalidReport(error.to_string()))?;
    report
        .validate()
        .map_err(|error| ReportEncryptionError::InvalidReport(error.to_string()))?;
    if report.report_id != envelope.report_id
        || report.recipient_source_id != envelope.recipient_source_id
    {
        return Err(ReportEncryptionError::Authentication);
    }
    report.encrypted_envelope = Some(envelope_json.into());
    Ok(report)
}

fn parse_key(value: &str) -> Result<[u8; 32], ReportEncryptionError> {
    hex::decode(value)
        .map_err(|_| ReportEncryptionError::InvalidKey)?
        .try_into()
        .map_err(|_| ReportEncryptionError::InvalidKey)
}

fn decode_array<const N: usize>(value: &str) -> Result<[u8; N], ReportEncryptionError> {
    URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| ReportEncryptionError::InvalidEnvelope)?
        .try_into()
        .map_err(|_| ReportEncryptionError::InvalidEnvelope)
}

fn derive_report_key(shared: &[u8; 32], ephemeral: &[u8; 32], recipient: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"jimmusic:moderation-report-key:v1\0");
    hasher.update(shared);
    hasher.update(ephemeral);
    hasher.update(recipient);
    hasher.finalize().into()
}

fn report_aad(report_id: &str, source_id: &str) -> Vec<u8> {
    let mut aad = b"jimmusic:moderation-report-envelope:v1\0".to_vec();
    aad.extend_from_slice(report_id.as_bytes());
    aad.push(0);
    aad.extend_from_slice(source_id.as_bytes());
    aad
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use jimmusic_protocol::SCHEMA_V1;

    #[test]
    fn verifies_valid_signature_and_rejects_tampering() {
        let signing = SigningKey::from_bytes(&[7; 32]);
        let signature = signing.sign(b"domain\0payload");
        let public = hex::encode(signing.verifying_key().to_bytes());
        let signature = hex::encode(signature.to_bytes());
        assert!(verify_ed25519_hex(&public, &signature, b"domain\0payload").is_ok());
        assert_eq!(
            verify_ed25519_hex(&public, &signature, b"domain\0tampered"),
            Err(SignatureError::VerificationFailed)
        );
    }

    #[test]
    fn moderation_envelope_round_trips_and_rejects_tampering() {
        let reporter = SigningKey::from_bytes(&[31; 32]);
        let mut report = ModerationReportV1 {
            schema_version: SCHEMA_V1,
            report_id: "report-private".into(),
            target: "bafy-private-target".into(),
            reason_code: "safety".into(),
            description: "private evidence narrative".into(),
            evidence_cids: vec!["bafy-private-evidence".into()],
            reporter_identity: None,
            reporter_public_key: hex::encode(reporter.verifying_key().to_bytes()),
            anonymous: true,
            recipient_source_id: "community.example".into(),
            created_at: 1,
            signature: None,
            encrypted_envelope: None,
        };
        report.signature = Some(hex::encode(
            reporter.sign(&report.unsigned_bytes().unwrap()).to_bytes(),
        ));
        let recipient_secret = StaticSecret::from([41; 32]);
        let recipient_public = PublicKey::from(&recipient_secret);
        let envelope =
            encrypt_moderation_report(&hex::encode(recipient_public.as_bytes()), &report).unwrap();
        assert!(!envelope.contains("private evidence"));
        assert!(!envelope.contains("bafy-private-target"));
        let decrypted =
            decrypt_moderation_report(&hex::encode(recipient_secret.to_bytes()), &envelope)
                .unwrap();
        assert_eq!(decrypted.target, report.target);
        assert_eq!(decrypted.signature, report.signature);

        let mut tampered: serde_json::Value = serde_json::from_str(&envelope).unwrap();
        tampered["ciphertext"] = serde_json::Value::String(URL_SAFE_NO_PAD.encode([0u8; 32]));
        assert!(matches!(
            decrypt_moderation_report(
                &hex::encode(recipient_secret.to_bytes()),
                &tampered.to_string()
            ),
            Err(ReportEncryptionError::Authentication)
        ));
    }
}
