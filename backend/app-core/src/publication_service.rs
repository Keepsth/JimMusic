//! 签名音乐 Manifest 与发布者 Feed 的事务服务。

use std::collections::BTreeMap;
use std::path::PathBuf;

use jimmusic_protocol::{
    canonical_dag_cbor, cid_v1_for, MusicManifestV1, PublicationEventType, PublicationEventV1,
    PublisherIdentityV1, Validate, SCHEMA_V1,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::crypto::{verify_ed25519_hex, SignatureError};
use crate::identity::{revocation_proof_message, rotation_proof_message};
use crate::node_service::{NodeError, NodeService};
use crate::storage::{AtomicJsonStore, StorageError};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PublicationRepositoryState {
    schema_version: u16,
    identities: BTreeMap<String, PublisherIdentityV1>,
    manifests: BTreeMap<String, MusicManifestV1>,
    events: BTreeMap<String, Vec<StoredPublicationEvent>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredPublicationEvent {
    pub cid: String,
    pub event: PublicationEventV1,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublicationReceipt {
    pub manifest_cid: Option<String>,
    pub event_cid: String,
    pub publisher_id: String,
    pub sequence: u64,
    pub pinned: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum PublicationError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Node(#[from] NodeError),
    #[error("invalid publication object: {0}")]
    Invalid(String),
    #[error("publisher identity `{0}` is unknown")]
    UnknownIdentity(String),
    #[error("publisher identity does not derive from its public key")]
    IdentityMismatch,
    #[error("signature is required for public publication")]
    MissingSignature,
    #[error(transparent)]
    Signature(#[from] SignatureError),
    #[error("feed sequence mismatch: expected {expected}, got {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("feed previous CID mismatch")]
    PreviousEvent,
    #[error("event does not reference the supplied manifest")]
    ManifestReference,
    #[error("event publisher does not match manifest publisher")]
    PublisherMismatch,
}

pub struct PublicationService {
    store: AtomicJsonStore<PublicationRepositoryState>,
}

impl PublicationService {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, PublicationError> {
        let path = path.into();
        let store = AtomicJsonStore::open(
            &path,
            PublicationRepositoryState {
                schema_version: SCHEMA_V1,
                identities: BTreeMap::new(),
                manifests: BTreeMap::new(),
                events: BTreeMap::new(),
            },
        )?;
        // NFR-014/API-007：拒绝更新版本写入的状态（降级保护），保留原文件。
        crate::storage::reject_future_schema_version(
            store.snapshot().schema_version,
            SCHEMA_V1,
            &path,
        )?;
        Ok(Self { store })
    }

    pub fn register_identity(
        &self,
        identity: PublisherIdentityV1,
        node: &NodeService,
    ) -> Result<String, PublicationError> {
        identity
            .validate()
            .map_err(|error| PublicationError::Invalid(error.to_string()))?;
        if publisher_id_from_public_key(&identity.public_key) != identity.publisher_id {
            return Err(PublicationError::IdentityMismatch);
        }
        if let (Some(previous_key), Some(proof)) =
            (&identity.previous_key, &identity.rotation_proof)
        {
            verify_ed25519_hex(
                previous_key,
                proof,
                &rotation_proof_message(previous_key, &identity.public_key),
            )?;
        }
        if let Some(revoked_at) = identity.revoked_at {
            let proof = identity.revocation_proof.as_deref().ok_or_else(|| {
                PublicationError::Invalid("revoked identity requires revocation_proof".into())
            })?;
            verify_ed25519_hex(
                &identity.public_key,
                proof,
                &revocation_proof_message(&identity.publisher_id, revoked_at),
            )?;
        } else if identity.revocation_proof.is_some() {
            return Err(PublicationError::Invalid(
                "revocation_proof requires revoked_at".into(),
            ));
        }
        let bytes = canonical_dag_cbor(&identity)
            .map_err(|error| PublicationError::Invalid(error.to_string()))?;
        let cid = node.add_dag_cbor(&bytes, true)?;
        self.store.transact(|state| {
            if identity.revoked_at.is_some() {
                for stored in state.identities.values_mut() {
                    if stored.publisher_id == identity.publisher_id {
                        stored.revoked_at = identity.revoked_at;
                        stored.revocation_proof = identity.revocation_proof.clone();
                    }
                }
            }
            state.identities.insert(cid.clone(), identity.clone());
            Ok(())
        })?;
        Ok(cid)
    }

    pub fn identity(&self, cid: &str) -> Option<PublisherIdentityV1> {
        self.store.snapshot().identities.get(cid).cloned()
    }

    /// 提交 publish/update。Manifest 与 Feed 事件均验签并按 CID Pin。
    pub fn publish(
        &self,
        manifest: MusicManifestV1,
        event: PublicationEventV1,
        node: &NodeService,
    ) -> Result<PublicationReceipt, PublicationError> {
        if !matches!(
            event.event_type,
            PublicationEventType::Publish | PublicationEventType::Update
        ) {
            return Err(PublicationError::Invalid(
                "publish requires a publish or update event".into(),
            ));
        }
        let manifest_cid = self.verify_manifest(&manifest)?;
        event
            .validate()
            .map_err(|error| PublicationError::Invalid(error.to_string()))?;
        let identity = self
            .identity(&manifest.publisher_identity_cid)
            .expect("verify_manifest checked identity existence");
        if identity.publisher_id != event.publisher_id {
            return Err(PublicationError::PublisherMismatch);
        }
        if event.manifest_cid.as_deref() != Some(manifest_cid.as_str()) {
            return Err(PublicationError::ManifestReference);
        }
        self.verify_event_chain(&event, &identity)?;

        let manifest_bytes = canonical_dag_cbor(&manifest)
            .map_err(|error| PublicationError::Invalid(error.to_string()))?;
        let event_bytes = canonical_dag_cbor(&event)
            .map_err(|error| PublicationError::Invalid(error.to_string()))?;
        let committed_manifest = node.add_dag_cbor(&manifest_bytes, true)?;
        debug_assert_eq!(committed_manifest, manifest_cid);
        let event_cid = node.add_dag_cbor(&event_bytes, true)?;

        self.store.transact(|state| {
            // 在落盘锁内再次检查，防止同一进程并发产生分叉。
            check_chain(state.events.get(&event.publisher_id), &event).map_err(|error| {
                StorageError::Corrupt {
                    path: PathBuf::from("publication-feed"),
                    reason: error.to_string(),
                }
            })?;
            state
                .manifests
                .insert(manifest_cid.clone(), manifest.clone());
            state
                .events
                .entry(event.publisher_id.clone())
                .or_default()
                .push(StoredPublicationEvent {
                    cid: event_cid.clone(),
                    event: event.clone(),
                });
            Ok(())
        })?;
        Ok(PublicationReceipt {
            manifest_cid: Some(manifest_cid),
            event_cid,
            publisher_id: event.publisher_id,
            sequence: event.sequence,
            pinned: true,
        })
    }

    pub fn tombstone(
        &self,
        event: PublicationEventV1,
        identity_cid: &str,
        node: &NodeService,
    ) -> Result<PublicationReceipt, PublicationError> {
        if event.event_type != PublicationEventType::Tombstone {
            return Err(PublicationError::Invalid(
                "tombstone requires a tombstone event".into(),
            ));
        }
        event
            .validate()
            .map_err(|error| PublicationError::Invalid(error.to_string()))?;
        let identity = self
            .identity(identity_cid)
            .ok_or_else(|| PublicationError::UnknownIdentity(identity_cid.into()))?;
        if identity.publisher_id != event.publisher_id {
            return Err(PublicationError::PublisherMismatch);
        }
        self.verify_event_chain(&event, &identity)?;
        let bytes = canonical_dag_cbor(&event)
            .map_err(|error| PublicationError::Invalid(error.to_string()))?;
        let event_cid = node.add_dag_cbor(&bytes, true)?;
        self.store.transact(|state| {
            check_chain(state.events.get(&event.publisher_id), &event).map_err(|error| {
                StorageError::Corrupt {
                    path: PathBuf::from("publication-feed"),
                    reason: error.to_string(),
                }
            })?;
            state
                .events
                .entry(event.publisher_id.clone())
                .or_default()
                .push(StoredPublicationEvent {
                    cid: event_cid.clone(),
                    event: event.clone(),
                });
            Ok(())
        })?;
        Ok(PublicationReceipt {
            manifest_cid: None,
            event_cid,
            publisher_id: event.publisher_id,
            sequence: event.sequence,
            pinned: true,
        })
    }

    pub fn feed(&self, publisher_id: &str) -> Vec<StoredPublicationEvent> {
        self.store
            .snapshot()
            .events
            .get(publisher_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn manifest(&self, cid: &str) -> Option<MusicManifestV1> {
        self.store.snapshot().manifests.get(cid).cloned()
    }

    /// Validate a public manifest independently from a feed commit. This is
    /// used when importing an immutable manifest into the local media library.
    pub fn verify_manifest(&self, manifest: &MusicManifestV1) -> Result<String, PublicationError> {
        manifest
            .validate()
            .map_err(|error| PublicationError::Invalid(error.to_string()))?;
        let identity = self
            .identity(&manifest.publisher_identity_cid)
            .ok_or_else(|| {
                PublicationError::UnknownIdentity(manifest.publisher_identity_cid.clone())
            })?;
        if identity.revoked_at.is_some() {
            return Err(PublicationError::Invalid(
                "publisher identity is revoked".into(),
            ));
        }
        verify_ed25519_hex(
            &identity.public_key,
            manifest
                .publisher_signature
                .as_deref()
                .ok_or(PublicationError::MissingSignature)?,
            &manifest
                .unsigned_bytes()
                .map_err(|error| PublicationError::Invalid(error.to_string()))?,
        )?;
        cid_v1_for(manifest).map_err(|error| PublicationError::Invalid(error.to_string()))
    }

    fn verify_event_chain(
        &self,
        event: &PublicationEventV1,
        identity: &PublisherIdentityV1,
    ) -> Result<(), PublicationError> {
        verify_ed25519_hex(
            &identity.public_key,
            event
                .signature
                .as_deref()
                .ok_or(PublicationError::MissingSignature)?,
            &event
                .unsigned_bytes()
                .map_err(|error| PublicationError::Invalid(error.to_string()))?,
        )?;
        check_chain(self.store.snapshot().events.get(&event.publisher_id), event)
    }
}

fn check_chain(
    existing: Option<&Vec<StoredPublicationEvent>>,
    event: &PublicationEventV1,
) -> Result<(), PublicationError> {
    let expected_sequence = existing.map_or(0, |events| events.len() as u64);
    if event.sequence != expected_sequence {
        return Err(PublicationError::Sequence {
            expected: expected_sequence,
            actual: event.sequence,
        });
    }
    let expected_previous =
        existing.and_then(|events| events.last().map(|entry| entry.cid.as_str()));
    if event.previous_event_cid.as_deref() != expected_previous {
        return Err(PublicationError::PreviousEvent);
    }
    Ok(())
}

pub fn publisher_id_from_public_key(public_key_hex: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"jimmusic:publisher-id:v1\0");
    hasher.update(public_key_hex.as_bytes());
    format!("jm:{}", &hex::encode(hasher.finalize())[..32])
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use jimmusic_protocol::{LicenseDeclaration, MusicRenditionV1};

    fn setup(
        dir: &std::path::Path,
    ) -> (
        PublicationService,
        NodeService,
        SigningKey,
        String,
        PublisherIdentityV1,
    ) {
        let service = PublicationService::open(dir.join("publications.json")).unwrap();
        let node = NodeService::open(dir.join("node"), "peer").unwrap();
        let key = SigningKey::from_bytes(&[9; 32]);
        let public = hex::encode(key.verifying_key().to_bytes());
        let identity = PublisherIdentityV1 {
            schema_version: SCHEMA_V1,
            publisher_id: publisher_id_from_public_key(&public),
            public_key: public,
            display_name: "Artist".into(),
            created_at: 1,
            previous_key: None,
            rotation_proof: None,
            revoked_at: None,
            revocation_proof: None,
        };
        let cid = service.register_identity(identity.clone(), &node).unwrap();
        (service, node, key, cid, identity)
    }

    fn signed_manifest(key: &SigningKey, identity_cid: String) -> MusicManifestV1 {
        let mut manifest = MusicManifestV1 {
            schema_version: SCHEMA_V1,
            work_id: "work".into(),
            release_id: "release".into(),
            title: "Track".into(),
            artists: vec!["Artist".into()],
            album: "Album".into(),
            track_number: Some(1),
            disc_number: Some(1),
            duration_ms: 1_000,
            language: "en".into(),
            genres: Vec::new(),
            tags: Vec::new(),
            cover_cid: None,
            lyrics_cid: None,
            credits: BTreeMap::new(),
            license: LicenseDeclaration {
                identifier: "CC-BY-4.0".into(),
                rights_statement: None,
                allows_redistribution: true,
            },
            content_labels: vec!["clean".into()],
            renditions: vec![MusicRenditionV1 {
                rendition_id: "original".into(),
                content_cid: "bafycontent".into(),
                container: "flac".into(),
                codec: "flac".into(),
                profile: String::new(),
                sample_rate: 44_100,
                bit_depth: 24,
                channels: 2,
                channel_layout: "stereo".into(),
                duration_ms: 1_000,
                byte_length: 10,
                lossless: true,
                original: true,
                streamable: true,
            }],
            publisher_identity_cid: identity_cid,
            created_at: 1,
            updated_at: 1,
            publisher_signature: None,
        };
        let signature = key.sign(&manifest.unsigned_bytes().unwrap());
        manifest.publisher_signature = Some(hex::encode(signature.to_bytes()));
        manifest
    }

    fn signed_event(
        key: &SigningKey,
        publisher_id: String,
        manifest_cid: String,
    ) -> PublicationEventV1 {
        let mut event = PublicationEventV1 {
            schema_version: SCHEMA_V1,
            event_type: PublicationEventType::Publish,
            publisher_id,
            sequence: 0,
            previous_event_cid: None,
            manifest_cid: Some(manifest_cid),
            target_cid: None,
            timestamp: 2,
            reason: None,
            signature: None,
        };
        event.signature = Some(hex::encode(
            key.sign(&event.unsigned_bytes().unwrap()).to_bytes(),
        ));
        event
    }

    #[test]
    fn signed_publish_is_pinned_and_queryable() {
        let dir = tempfile::tempdir().unwrap();
        let (service, node, key, identity_cid, identity) = setup(dir.path());
        let manifest = signed_manifest(&key, identity_cid);
        let manifest_cid = cid_v1_for(&manifest).unwrap();
        let event = signed_event(&key, identity.publisher_id, manifest_cid.clone());
        let receipt = service.publish(manifest, event, &node).unwrap();
        assert_eq!(receipt.manifest_cid.as_deref(), Some(manifest_cid.as_str()));
        assert_eq!(service.feed(&receipt.publisher_id).len(), 1);
        assert!(node.list_pins().contains(&receipt.event_cid));
    }

    #[test]
    fn tampered_manifest_and_feed_replay_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let (service, node, key, identity_cid, identity) = setup(dir.path());
        let mut manifest = signed_manifest(&key, identity_cid.clone());
        manifest.title = "Tampered".into();
        let cid = cid_v1_for(&manifest).unwrap();
        let event = signed_event(&key, identity.publisher_id.clone(), cid);
        assert!(matches!(
            service.publish(manifest, event, &node),
            Err(PublicationError::Signature(_))
        ));

        let manifest = signed_manifest(&key, identity_cid);
        let cid = cid_v1_for(&manifest).unwrap();
        let event = signed_event(&key, identity.publisher_id.clone(), cid);
        service
            .publish(manifest.clone(), event.clone(), &node)
            .unwrap();
        assert!(matches!(
            service.publish(manifest, event, &node),
            Err(PublicationError::Sequence { .. })
        ));
    }

    #[test]
    fn missing_license_is_rejected_before_pin() {
        let dir = tempfile::tempdir().unwrap();
        let (service, node, key, identity_cid, identity) = setup(dir.path());
        let mut manifest = signed_manifest(&key, identity_cid);
        manifest.license.identifier.clear();
        manifest.publisher_signature = Some(hex::encode(
            key.sign(&manifest.unsigned_bytes().unwrap()).to_bytes(),
        ));
        let cid = cid_v1_for(&manifest).unwrap();
        let event = signed_event(&key, identity.publisher_id, cid);
        assert!(matches!(
            service.publish(manifest, event, &node),
            Err(PublicationError::Invalid(_))
        ));
    }

    #[test]
    fn revocation_is_verified_and_applies_to_previous_identity_cid() {
        let dir = tempfile::tempdir().unwrap();
        let (service, node, key, original_cid, identity) = setup(dir.path());
        let mut invalid = identity.clone();
        invalid.revoked_at = Some(10);
        invalid.revocation_proof = Some("00".repeat(64));
        assert!(matches!(
            service.register_identity(invalid, &node),
            Err(PublicationError::Signature(_))
        ));

        let mut revoked = identity;
        revoked.revoked_at = Some(10);
        revoked.revocation_proof = Some(hex::encode(
            key.sign(&revocation_proof_message(&revoked.publisher_id, 10))
                .to_bytes(),
        ));
        service.register_identity(revoked, &node).unwrap();
        assert_eq!(
            service.identity(&original_cid).unwrap().revoked_at,
            Some(10)
        );
    }
}
