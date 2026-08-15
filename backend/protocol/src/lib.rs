//! JimMusic 2.x 的传输无关协议契约。
//!
//! 本 crate 只包含可序列化模型、确定性 DAG-CBOR 编码、CIDv1 计算与边界校验，
//! 不依赖 HTTP、FFI 或 Flutter。所有传输适配器必须复用这里的 DTO 与错误语义。

use std::collections::{BTreeMap, BTreeSet};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const SCHEMA_V1: u16 = 1;
pub const EVENT_SCHEMA_V1: u16 = 1;
pub const DAG_CBOR_CODEC: u64 = 0x71;
pub const SHA2_256_CODE: u64 = 0x12;

/// 网络对象解析上限。调用方可收紧，但不能绕过这些默认安全边界。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjectLimits {
    pub max_bytes: usize,
    pub max_depth: usize,
    pub max_items: usize,
    pub max_string_bytes: usize,
}

impl Default for ObjectLimits {
    fn default() -> Self {
        Self {
            max_bytes: 2 * 1024 * 1024,
            max_depth: 32,
            max_items: 10_000,
            max_string_bytes: 64 * 1024,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProtocolError {
    #[error("unsupported schema version {0}")]
    UnsupportedSchema(u16),
    #[error("missing required field `{0}`")]
    MissingField(&'static str),
    #[error("invalid field `{field}`: {reason}")]
    InvalidField { field: &'static str, reason: String },
    #[error("object exceeds {kind} limit ({actual} > {limit})")]
    LimitExceeded {
        kind: &'static str,
        actual: usize,
        limit: usize,
    },
    #[error("DAG-CBOR does not support floating-point values in signed JimMusic objects")]
    FloatingPoint,
    #[error("serialization failed: {0}")]
    Serialization(String),
}

pub trait Validate {
    fn validate(&self) -> Result<(), ProtocolError>;
}

fn require_v1(version: u16) -> Result<(), ProtocolError> {
    if version == SCHEMA_V1 {
        Ok(())
    } else {
        Err(ProtocolError::UnsupportedSchema(version))
    }
}

fn required(value: &str, field: &'static str) -> Result<(), ProtocolError> {
    if value.trim().is_empty() {
        Err(ProtocolError::MissingField(field))
    } else {
        Ok(())
    }
}

/// 将模型编码成确定性 DAG-CBOR。映射键按 RFC 8949 确定性顺序排列。
pub fn canonical_dag_cbor<T: Serialize>(value: &T) -> Result<Vec<u8>, ProtocolError> {
    canonical_dag_cbor_with_limits(value, ObjectLimits::default())
}

pub fn canonical_dag_cbor_with_limits<T: Serialize>(
    value: &T,
    limits: ObjectLimits,
) -> Result<Vec<u8>, ProtocolError> {
    let value =
        serde_json::to_value(value).map_err(|e| ProtocolError::Serialization(e.to_string()))?;
    let mut stats = ObjectStats::default();
    inspect_value(&value, 0, limits, &mut stats)?;
    let mut out = Vec::new();
    encode_value(&value, &mut out)?;
    if out.len() > limits.max_bytes {
        return Err(ProtocolError::LimitExceeded {
            kind: "encoded bytes",
            actual: out.len(),
            limit: limits.max_bytes,
        });
    }
    Ok(out)
}

/// 解码本协议生成的确定性 DAG-CBOR 子集，并重新编码验证 canonical 表示。
pub fn decode_dag_cbor<T: DeserializeOwned + Serialize>(bytes: &[u8]) -> Result<T, ProtocolError> {
    let limits = ObjectLimits::default();
    if bytes.len() > limits.max_bytes {
        return Err(ProtocolError::LimitExceeded {
            kind: "encoded bytes",
            actual: bytes.len(),
            limit: limits.max_bytes,
        });
    }
    let mut decoder = DagCborDecoder {
        bytes,
        offset: 0,
        limits,
        items: 0,
    };
    let value = decoder.value(0)?;
    if decoder.offset != bytes.len() {
        return Err(ProtocolError::Serialization(
            "trailing bytes after DAG-CBOR value".into(),
        ));
    }
    let decoded: T = serde_json::from_value(value)
        .map_err(|error| ProtocolError::Serialization(error.to_string()))?;
    let canonical = canonical_dag_cbor(&decoded)?;
    if canonical != bytes {
        return Err(ProtocolError::Serialization(
            "DAG-CBOR input is not in deterministic canonical form".into(),
        ));
    }
    Ok(decoded)
}

struct DagCborDecoder<'a> {
    bytes: &'a [u8],
    offset: usize,
    limits: ObjectLimits,
    items: usize,
}

impl DagCborDecoder<'_> {
    fn value(&mut self, depth: usize) -> Result<Value, ProtocolError> {
        if depth > self.limits.max_depth {
            return Err(ProtocolError::LimitExceeded {
                kind: "nesting depth",
                actual: depth,
                limit: self.limits.max_depth,
            });
        }
        self.items += 1;
        if self.items > self.limits.max_items {
            return Err(ProtocolError::LimitExceeded {
                kind: "item count",
                actual: self.items,
                limit: self.limits.max_items,
            });
        }
        let initial = self.byte()?;
        let major = initial >> 5;
        let additional = initial & 0x1f;
        match major {
            0 => Ok(Value::Number(self.length(additional)?.into())),
            1 => {
                let encoded = self.length(additional)?;
                let negative = -1i128 - encoded as i128;
                let integer = i64::try_from(negative).map_err(|_| {
                    ProtocolError::Serialization("negative integer exceeds i64".into())
                })?;
                Ok(Value::Number(integer.into()))
            }
            3 => {
                let length = self.length(additional)? as usize;
                if length > self.limits.max_string_bytes {
                    return Err(ProtocolError::LimitExceeded {
                        kind: "string bytes",
                        actual: length,
                        limit: self.limits.max_string_bytes,
                    });
                }
                let bytes = self.take(length)?;
                let text = std::str::from_utf8(bytes)
                    .map_err(|error| ProtocolError::Serialization(error.to_string()))?;
                Ok(Value::String(text.into()))
            }
            4 => {
                let length = self.length(additional)? as usize;
                let mut values = Vec::with_capacity(length.min(self.limits.max_items));
                for _ in 0..length {
                    values.push(self.value(depth + 1)?);
                }
                Ok(Value::Array(values))
            }
            5 => {
                let length = self.length(additional)? as usize;
                let mut values = serde_json::Map::new();
                for _ in 0..length {
                    let Value::String(key) = self.value(depth + 1)? else {
                        return Err(ProtocolError::Serialization(
                            "DAG-CBOR map keys must be text strings".into(),
                        ));
                    };
                    if values.contains_key(&key) {
                        return Err(ProtocolError::Serialization(
                            "duplicate DAG-CBOR map key".into(),
                        ));
                    }
                    values.insert(key, self.value(depth + 1)?);
                }
                Ok(Value::Object(values))
            }
            7 => match additional {
                20 => Ok(Value::Bool(false)),
                21 => Ok(Value::Bool(true)),
                22 => Ok(Value::Null),
                _ => Err(ProtocolError::Serialization(
                    "floats, undefined and unsupported simple values are forbidden".into(),
                )),
            },
            _ => Err(ProtocolError::Serialization(format!(
                "unsupported DAG-CBOR major type {major}"
            ))),
        }
    }

    fn length(&mut self, additional: u8) -> Result<u64, ProtocolError> {
        match additional {
            0..=23 => Ok(additional as u64),
            24 => Ok(self.byte()? as u64),
            25 => Ok(u16::from_be_bytes(self.take(2)?.try_into().expect("length checked")) as u64),
            26 => Ok(u32::from_be_bytes(self.take(4)?.try_into().expect("length checked")) as u64),
            27 => Ok(u64::from_be_bytes(
                self.take(8)?.try_into().expect("length checked"),
            )),
            _ => Err(ProtocolError::Serialization(
                "indefinite-length DAG-CBOR is forbidden".into(),
            )),
        }
    }

    fn byte(&mut self) -> Result<u8, ProtocolError> {
        let byte = *self
            .bytes
            .get(self.offset)
            .ok_or_else(|| ProtocolError::Serialization("truncated DAG-CBOR".into()))?;
        self.offset += 1;
        Ok(byte)
    }

    fn take(&mut self, length: usize) -> Result<&[u8], ProtocolError> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or_else(|| ProtocolError::Serialization("DAG-CBOR length overflow".into()))?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| ProtocolError::Serialization("truncated DAG-CBOR".into()))?;
        self.offset = end;
        Ok(bytes)
    }
}

#[derive(Default)]
struct ObjectStats {
    items: usize,
}

fn inspect_value(
    value: &Value,
    depth: usize,
    limits: ObjectLimits,
    stats: &mut ObjectStats,
) -> Result<(), ProtocolError> {
    if depth > limits.max_depth {
        return Err(ProtocolError::LimitExceeded {
            kind: "nesting depth",
            actual: depth,
            limit: limits.max_depth,
        });
    }
    stats.items += 1;
    if stats.items > limits.max_items {
        return Err(ProtocolError::LimitExceeded {
            kind: "item count",
            actual: stats.items,
            limit: limits.max_items,
        });
    }
    match value {
        Value::String(s) => {
            if s.len() > limits.max_string_bytes {
                return Err(ProtocolError::LimitExceeded {
                    kind: "string bytes",
                    actual: s.len(),
                    limit: limits.max_string_bytes,
                });
            }
        }
        Value::Array(values) => {
            for child in values {
                inspect_value(child, depth + 1, limits, stats)?;
            }
        }
        Value::Object(values) => {
            for (key, child) in values {
                if key.len() > limits.max_string_bytes {
                    return Err(ProtocolError::LimitExceeded {
                        kind: "map key bytes",
                        actual: key.len(),
                        limit: limits.max_string_bytes,
                    });
                }
                inspect_value(child, depth + 1, limits, stats)?;
            }
        }
        Value::Number(n) if n.as_i64().is_none() && n.as_u64().is_none() => {
            return Err(ProtocolError::FloatingPoint);
        }
        _ => {}
    }
    Ok(())
}

fn encode_value(value: &Value, out: &mut Vec<u8>) -> Result<(), ProtocolError> {
    match value {
        Value::Null => out.push(0xf6),
        Value::Bool(false) => out.push(0xf4),
        Value::Bool(true) => out.push(0xf5),
        Value::Number(number) => {
            if let Some(n) = number.as_u64() {
                encode_head(0, n, out);
            } else if let Some(n) = number.as_i64() {
                let encoded = (-1i128 - n as i128) as u64;
                encode_head(1, encoded, out);
            } else {
                return Err(ProtocolError::FloatingPoint);
            }
        }
        Value::String(text) => {
            encode_head(3, text.len() as u64, out);
            out.extend_from_slice(text.as_bytes());
        }
        Value::Array(values) => {
            encode_head(4, values.len() as u64, out);
            for value in values {
                encode_value(value, out)?;
            }
        }
        Value::Object(values) => {
            encode_head(5, values.len() as u64, out);
            let mut keys: Vec<&String> = values.keys().collect();
            keys.sort_by(|a, b| {
                let aa = a.as_bytes();
                let bb = b.as_bytes();
                aa.len().cmp(&bb.len()).then_with(|| aa.cmp(bb))
            });
            for key in keys {
                encode_head(3, key.len() as u64, out);
                out.extend_from_slice(key.as_bytes());
                encode_value(&values[key], out)?;
            }
        }
    }
    Ok(())
}

fn encode_head(major: u8, value: u64, out: &mut Vec<u8>) {
    let prefix = major << 5;
    match value {
        0..=23 => out.push(prefix | value as u8),
        24..=0xff => out.extend_from_slice(&[prefix | 24, value as u8]),
        0x100..=0xffff => {
            out.push(prefix | 25);
            out.extend_from_slice(&(value as u16).to_be_bytes());
        }
        0x1_0000..=0xffff_ffff => {
            out.push(prefix | 26);
            out.extend_from_slice(&(value as u32).to_be_bytes());
        }
        _ => {
            out.push(prefix | 27);
            out.extend_from_slice(&value.to_be_bytes());
        }
    }
}

/// 计算 `dag-cbor + sha2-256` 的 CIDv1（base32 小写、无 padding）。
pub fn cid_v1_for<T: Serialize>(value: &T) -> Result<String, ProtocolError> {
    let bytes = canonical_dag_cbor(value)?;
    Ok(cid_v1_for_bytes(DAG_CBOR_CODEC, &bytes))
}

pub fn cid_v1_for_bytes(codec: u64, bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    cid_v1_for_sha256_digest(codec, digest.into())
}

/// 从已流式计算的 SHA-256 摘要构造 CIDv1，避免为了内容寻址再次将大对象载入内存。
pub fn cid_v1_for_sha256_digest(codec: u64, digest: [u8; 32]) -> String {
    let mut cid = Vec::with_capacity(4 + digest.len());
    push_varint(1, &mut cid);
    push_varint(codec, &mut cid);
    push_varint(SHA2_256_CODE, &mut cid);
    push_varint(digest.len() as u64, &mut cid);
    cid.extend_from_slice(&digest);
    format!("b{}", base32_lower(&cid))
}

fn push_varint(mut value: u64, out: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

fn base32_lower(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut output = String::with_capacity((bytes.len() * 8).div_ceil(5));
    let mut buffer = 0u16;
    let mut bits = 0u8;
    for &byte in bytes {
        buffer = (buffer << 8) | byte as u16;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            output.push(ALPHABET[((buffer >> bits) & 0x1f) as usize] as char);
        }
    }
    if bits > 0 {
        output.push(ALPHABET[((buffer << (5 - bits)) & 0x1f) as usize] as char);
    }
    output
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LicenseDeclaration {
    pub identifier: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rights_statement: Option<String>,
    pub allows_redistribution: bool,
}

impl Validate for LicenseDeclaration {
    fn validate(&self) -> Result<(), ProtocolError> {
        required(&self.identifier, "license.identifier")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublisherIdentityV1 {
    pub schema_version: u16,
    pub publisher_id: String,
    pub public_key: String,
    pub display_name: String,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation_proof: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revocation_proof: Option<String>,
}

impl Validate for PublisherIdentityV1 {
    fn validate(&self) -> Result<(), ProtocolError> {
        require_v1(self.schema_version)?;
        required(&self.publisher_id, "publisher_id")?;
        required(&self.public_key, "public_key")?;
        required(&self.display_name, "display_name")?;
        if self.previous_key.is_some() != self.rotation_proof.is_some() {
            return Err(ProtocolError::InvalidField {
                field: "rotation_proof",
                reason: "previous_key and rotation_proof must be provided together".into(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MusicRenditionV1 {
    pub rendition_id: String,
    pub content_cid: String,
    pub container: String,
    pub codec: String,
    #[serde(default)]
    pub profile: String,
    pub sample_rate: u32,
    pub bit_depth: u16,
    pub channels: u16,
    pub channel_layout: String,
    pub duration_ms: u64,
    pub byte_length: u64,
    pub lossless: bool,
    pub original: bool,
    pub streamable: bool,
}

impl Validate for MusicRenditionV1 {
    fn validate(&self) -> Result<(), ProtocolError> {
        required(&self.rendition_id, "rendition_id")?;
        required(&self.content_cid, "content_cid")?;
        required(&self.container, "container")?;
        required(&self.codec, "codec")?;
        if self.sample_rate == 0 || self.channels == 0 || self.byte_length == 0 {
            return Err(ProtocolError::InvalidField {
                field: "rendition",
                reason: "sample_rate, channels and byte_length must be non-zero".into(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MusicManifestV1 {
    pub schema_version: u16,
    pub work_id: String,
    pub release_id: String,
    pub title: String,
    pub artists: Vec<String>,
    #[serde(default)]
    pub album: String,
    #[serde(default)]
    pub track_number: Option<u16>,
    #[serde(default)]
    pub disc_number: Option<u16>,
    pub duration_ms: u64,
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub cover_cid: Option<String>,
    #[serde(default)]
    pub lyrics_cid: Option<String>,
    #[serde(default)]
    pub credits: BTreeMap<String, Vec<String>>,
    pub license: LicenseDeclaration,
    #[serde(default)]
    pub content_labels: Vec<String>,
    pub renditions: Vec<MusicRenditionV1>,
    pub publisher_identity_cid: String,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher_signature: Option<String>,
}

impl MusicManifestV1 {
    pub fn unsigned_bytes(&self) -> Result<Vec<u8>, ProtocolError> {
        let mut unsigned = self.clone();
        unsigned.publisher_signature = None;
        let mut bytes = b"jimmusic:music-manifest:v1\0".to_vec();
        bytes.extend(canonical_dag_cbor(&unsigned)?);
        Ok(bytes)
    }
}

impl Validate for MusicManifestV1 {
    fn validate(&self) -> Result<(), ProtocolError> {
        require_v1(self.schema_version)?;
        required(&self.work_id, "work_id")?;
        required(&self.release_id, "release_id")?;
        required(&self.title, "title")?;
        required(&self.publisher_identity_cid, "publisher_identity_cid")?;
        self.license.validate()?;
        if self.artists.is_empty() {
            return Err(ProtocolError::MissingField("artists"));
        }
        if self.renditions.is_empty() {
            return Err(ProtocolError::MissingField("renditions"));
        }
        let mut ids = BTreeSet::new();
        for rendition in &self.renditions {
            rendition.validate()?;
            if !ids.insert(&rendition.rendition_id) {
                return Err(ProtocolError::InvalidField {
                    field: "renditions",
                    reason: format!("duplicate rendition `{}`", rendition.rendition_id),
                });
            }
        }
        if self.updated_at < self.created_at {
            return Err(ProtocolError::InvalidField {
                field: "updated_at",
                reason: "must not precede created_at".into(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PublicationEventType {
    Publish,
    Update,
    Tombstone,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublicationEventV1 {
    pub schema_version: u16,
    pub event_type: PublicationEventType,
    pub publisher_id: String,
    pub sequence: u64,
    #[serde(default)]
    pub previous_event_cid: Option<String>,
    #[serde(default)]
    pub manifest_cid: Option<String>,
    #[serde(default)]
    pub target_cid: Option<String>,
    pub timestamp: i64,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub signature: Option<String>,
}

impl PublicationEventV1 {
    pub fn unsigned_bytes(&self) -> Result<Vec<u8>, ProtocolError> {
        let mut unsigned = self.clone();
        unsigned.signature = None;
        let mut bytes = b"jimmusic:publication-event:v1\0".to_vec();
        bytes.extend(canonical_dag_cbor(&unsigned)?);
        Ok(bytes)
    }
}

impl Validate for PublicationEventV1 {
    fn validate(&self) -> Result<(), ProtocolError> {
        require_v1(self.schema_version)?;
        required(&self.publisher_id, "publisher_id")?;
        if self.sequence > 0 && self.previous_event_cid.is_none() {
            return Err(ProtocolError::MissingField("previous_event_cid"));
        }
        match self.event_type {
            PublicationEventType::Publish | PublicationEventType::Update
                if self.manifest_cid.is_none() =>
            {
                Err(ProtocolError::MissingField("manifest_cid"))
            }
            PublicationEventType::Tombstone if self.target_cid.is_none() => {
                Err(ProtocolError::MissingField("target_cid"))
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommunitySourceManifestV1 {
    pub schema_version: u16,
    pub source_id: String,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub languages: Vec<String>,
    pub maintainer_identity_cid: String,
    #[serde(default)]
    pub catalog_head: Option<String>,
    #[serde(default)]
    pub policy_head: Option<String>,
    pub supported_schemas: Vec<u16>,
    #[serde(default)]
    pub report_endpoint: Option<String>,
    /// Optional X25519 recipient key for encrypted moderation submissions.
    #[serde(default)]
    pub report_encryption_public_key: Option<String>,
    pub updated_at: i64,
    #[serde(default)]
    pub signature: Option<String>,
}

impl CommunitySourceManifestV1 {
    pub fn unsigned_bytes(&self) -> Result<Vec<u8>, ProtocolError> {
        let mut unsigned = self.clone();
        unsigned.signature = None;
        let mut bytes = b"jimmusic:community-source:v1\0".to_vec();
        bytes.extend(canonical_dag_cbor(&unsigned)?);
        Ok(bytes)
    }
}

impl Validate for CommunitySourceManifestV1 {
    fn validate(&self) -> Result<(), ProtocolError> {
        require_v1(self.schema_version)?;
        required(&self.source_id, "source_id")?;
        required(&self.name, "name")?;
        required(&self.maintainer_identity_cid, "maintainer_identity_cid")?;
        if !self.supported_schemas.contains(&SCHEMA_V1) {
            return Err(ProtocolError::InvalidField {
                field: "supported_schemas",
                reason: "schema v1 is required".into(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MaintainerKeyAction {
    Rotate,
    Revoke,
}

/// Signed continuity record for a community maintainer key.
///
/// The current key always signs the event. A rotation additionally carries a
/// proof from the new key over the exact same domain-separated bytes, so a
/// caller cannot rotate to a key it does not control.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MaintainerKeyEventV1 {
    pub schema_version: u16,
    pub source_id: String,
    pub action: MaintainerKeyAction,
    pub sequence: u64,
    #[serde(default)]
    pub previous_event_cid: Option<String>,
    pub current_public_key: String,
    #[serde(default)]
    pub new_public_key: Option<String>,
    pub issued_at: i64,
    #[serde(default)]
    pub signature: Option<String>,
    #[serde(default)]
    pub new_key_proof: Option<String>,
}

impl MaintainerKeyEventV1 {
    pub fn unsigned_bytes(&self) -> Result<Vec<u8>, ProtocolError> {
        let mut unsigned = self.clone();
        unsigned.signature = None;
        unsigned.new_key_proof = None;
        let mut bytes = b"jimmusic:maintainer-key-event:v1\0".to_vec();
        bytes.extend(canonical_dag_cbor(&unsigned)?);
        Ok(bytes)
    }
}

impl Validate for MaintainerKeyEventV1 {
    fn validate(&self) -> Result<(), ProtocolError> {
        require_v1(self.schema_version)?;
        required(&self.source_id, "source_id")?;
        required(&self.current_public_key, "current_public_key")?;
        if self.sequence > 0 && self.previous_event_cid.is_none() {
            return Err(ProtocolError::MissingField("previous_event_cid"));
        }
        match self.action {
            MaintainerKeyAction::Rotate => {
                let new_key = self
                    .new_public_key
                    .as_deref()
                    .ok_or(ProtocolError::MissingField("new_public_key"))?;
                required(new_key, "new_public_key")?;
                if new_key == self.current_public_key {
                    return Err(ProtocolError::InvalidField {
                        field: "new_public_key",
                        reason: "must differ from current_public_key".into(),
                    });
                }
                if self.new_key_proof.as_deref().is_none_or(str::is_empty) {
                    return Err(ProtocolError::MissingField("new_key_proof"));
                }
            }
            MaintainerKeyAction::Revoke => {
                if self.new_public_key.is_some() || self.new_key_proof.is_some() {
                    return Err(ProtocolError::InvalidField {
                        field: "new_public_key",
                        reason: "revocation cannot introduce a replacement key".into(),
                    });
                }
            }
        }
        if self.signature.as_deref().is_none_or(str::is_empty) {
            return Err(ProtocolError::MissingField("signature"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CatalogAction {
    Include,
    Update,
    Remove,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CatalogEventV1 {
    pub schema_version: u16,
    pub action: CatalogAction,
    pub target_type: String,
    pub target_cid: String,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub annotation: Option<String>,
    pub sequence: u64,
    #[serde(default)]
    pub previous_event_cid: Option<String>,
    #[serde(default)]
    pub expires_at: Option<i64>,
    pub issued_at: i64,
    #[serde(default)]
    pub signature: Option<String>,
}

impl CatalogEventV1 {
    pub fn unsigned_bytes(&self) -> Result<Vec<u8>, ProtocolError> {
        let mut unsigned = self.clone();
        unsigned.signature = None;
        let mut bytes = b"jimmusic:catalog-event:v1\0".to_vec();
        bytes.extend(canonical_dag_cbor(&unsigned)?);
        Ok(bytes)
    }
}

impl Validate for CatalogEventV1 {
    fn validate(&self) -> Result<(), ProtocolError> {
        require_v1(self.schema_version)?;
        required(&self.target_type, "target_type")?;
        required(&self.target_cid, "target_cid")?;
        if self.sequence > 0 && self.previous_event_cid.is_none() {
            return Err(ProtocolError::MissingField("previous_event_cid"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum PolicyAction {
    Warn,
    Demote,
    Hide,
    Block,
    Revoke,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PolicyEventV1 {
    pub schema_version: u16,
    pub action: PolicyAction,
    pub target_type: String,
    pub target: String,
    pub reason_code: String,
    pub description: String,
    #[serde(default)]
    pub evidence_cids: Vec<String>,
    #[serde(default)]
    pub scope: Vec<String>,
    pub issued_at: i64,
    #[serde(default)]
    pub expires_at: Option<i64>,
    pub sequence: u64,
    #[serde(default)]
    pub previous_event_cid: Option<String>,
    #[serde(default)]
    pub signature: Option<String>,
}

impl PolicyEventV1 {
    pub fn unsigned_bytes(&self) -> Result<Vec<u8>, ProtocolError> {
        let mut unsigned = self.clone();
        unsigned.signature = None;
        let mut bytes = b"jimmusic:policy-event:v1\0".to_vec();
        bytes.extend(canonical_dag_cbor(&unsigned)?);
        Ok(bytes)
    }
}

impl Validate for PolicyEventV1 {
    fn validate(&self) -> Result<(), ProtocolError> {
        require_v1(self.schema_version)?;
        required(&self.target_type, "target_type")?;
        required(&self.target, "target")?;
        required(&self.reason_code, "reason_code")?;
        if self.sequence > 0 && self.previous_event_cid.is_none() {
            return Err(ProtocolError::MissingField("previous_event_cid"));
        }
        if self
            .expires_at
            .is_some_and(|expires| expires <= self.issued_at)
        {
            return Err(ProtocolError::InvalidField {
                field: "expires_at",
                reason: "must be later than issued_at".into(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModerationReportV1 {
    pub schema_version: u16,
    pub report_id: String,
    pub target: String,
    pub reason_code: String,
    pub description: String,
    #[serde(default)]
    pub evidence_cids: Vec<String>,
    #[serde(default)]
    pub reporter_identity: Option<String>,
    /// Signing key for this report. Anonymous reports use a fresh unlinkable
    /// key and omit `reporter_identity`.
    pub reporter_public_key: String,
    pub anonymous: bool,
    pub recipient_source_id: String,
    pub created_at: i64,
    #[serde(default)]
    pub signature: Option<String>,
    #[serde(default)]
    pub encrypted_envelope: Option<String>,
}

impl ModerationReportV1 {
    pub fn unsigned_bytes(&self) -> Result<Vec<u8>, ProtocolError> {
        let mut unsigned = self.clone();
        unsigned.signature = None;
        // The envelope is a transport wrapper generated after the reporter
        // signs the plaintext report. Its AEAD authentication is independent.
        unsigned.encrypted_envelope = None;
        let mut bytes = b"jimmusic:moderation-report:v1\0".to_vec();
        bytes.extend(canonical_dag_cbor(&unsigned)?);
        Ok(bytes)
    }
}

impl Validate for ModerationReportV1 {
    fn validate(&self) -> Result<(), ProtocolError> {
        require_v1(self.schema_version)?;
        required(&self.report_id, "report_id")?;
        required(&self.target, "target")?;
        required(&self.reason_code, "reason_code")?;
        required(&self.recipient_source_id, "recipient_source_id")?;
        required(&self.reporter_public_key, "reporter_public_key")?;
        if self.anonymous && self.reporter_identity.is_some() {
            return Err(ProtocolError::InvalidField {
                field: "reporter_identity",
                reason: "anonymous reports must not include an identity".into(),
            });
        }
        if !self.anonymous && self.reporter_identity.as_deref().is_none_or(str::is_empty) {
            return Err(ProtocolError::MissingField("reporter_identity"));
        }
        if self.signature.as_deref().is_none_or(str::is_empty) {
            return Err(ProtocolError::MissingField("signature"));
        }
        if self
            .encrypted_envelope
            .as_deref()
            .is_some_and(str::is_empty)
        {
            return Err(ProtocolError::InvalidField {
                field: "encrypted_envelope",
                reason: "must not be empty".into(),
            });
        }
        for evidence in &self.evidence_cids {
            required(evidence, "evidence_cids")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FeedKind {
    Catalog,
    Policy,
}

/// 一个社区 Feed 在签名 head 处的紧凑、确定性可编码快照条目。
///
/// 快照把同一目标的多个历史事件折叠为“最新有效决策”，并丢弃已过期事件，因此大型 Feed
/// 无需每次全量回放即可引导。`action` 使用 snake_case 名称（Catalog 为
/// `include`/`update`/`remove`，Policy 为 `warn`/`demote`/`hide`/`block`/`revoke`）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeedSnapshotEntryV1 {
    pub target: String,
    pub target_type: String,
    pub action: String,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub annotation: Option<String>,
    #[serde(default)]
    pub reason_code: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    pub issued_at: i64,
    #[serde(default)]
    pub expires_at: Option<i64>,
}

/// 社区 Catalog/Policy Feed 的紧凑快照。head 字段锚定到签名事件链，使消费者能把快照
/// 与已信任的签名 head 对应，而不必重放完整历史。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeedSnapshotV1 {
    pub schema_version: u16,
    pub source_id: String,
    pub feed_kind: FeedKind,
    pub head_sequence: u64,
    #[serde(default)]
    pub head_event_cid: Option<String>,
    pub created_at: i64,
    pub entries: Vec<FeedSnapshotEntryV1>,
}

impl Validate for FeedSnapshotV1 {
    fn validate(&self) -> Result<(), ProtocolError> {
        require_v1(self.schema_version)?;
        required(&self.source_id, "source_id")?;
        for entry in &self.entries {
            required(&entry.target, "entries.target")?;
            required(&entry.target_type, "entries.target_type")?;
            required(&entry.action, "entries.action")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PluginRuntime {
    Declarative,
    Wasm,
    Native,
    Service,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum PluginPermission {
    MusicLibraryRead,
    MusicLibraryWrite,
    IpfsFetch,
    IpfsPublish,
    NetworkDomains,
    IsolatedStorage,
    AudioRealtime,
    AudioDevice,
    HardwareExclusive,
    UserInterfaceSchema,
    Diagnostics,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginArtifactV1 {
    pub artifact_cid: String,
    pub platform: String,
    pub architecture: String,
    pub runtime: PluginRuntime,
    pub entrypoint: String,
    pub byte_length: u64,
    pub sha256: String,
    #[serde(default)]
    pub provenance_cid: Option<String>,
    #[serde(default)]
    pub sbom_cid: Option<String>,
    pub sandbox_profile: String,
    #[serde(default)]
    pub required_host_capabilities: Vec<String>,
    #[serde(default)]
    pub hardware_requirements: Vec<String>,
}

impl Validate for PluginArtifactV1 {
    fn validate(&self) -> Result<(), ProtocolError> {
        required(&self.artifact_cid, "artifact_cid")?;
        required(&self.platform, "platform")?;
        required(&self.architecture, "architecture")?;
        required(&self.entrypoint, "entrypoint")?;
        required(&self.sha256, "sha256")?;
        if self.byte_length == 0 {
            return Err(ProtocolError::InvalidField {
                field: "byte_length",
                reason: "must be non-zero".into(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginDependencyV1 {
    pub plugin_id: String,
    pub version_requirement: String,
    pub optional: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginManifestV1 {
    pub schema_version: u16,
    pub plugin_id: String,
    pub name: String,
    pub version: String,
    pub publisher: String,
    pub plugin_kind: String,
    pub interface_versions: BTreeMap<String, String>,
    pub minimum_core_version: String,
    pub maximum_core_version: String,
    pub artifacts: Vec<PluginArtifactV1>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub permissions: BTreeSet<PluginPermission>,
    #[serde(default)]
    pub dependencies: Vec<PluginDependencyV1>,
    #[serde(default)]
    pub conflicts: Vec<String>,
    pub configuration_schema_cid: String,
    pub state_schema_version: u16,
    pub license: String,
    #[serde(default)]
    pub release_notes_cid: Option<String>,
    #[serde(default)]
    pub previous_release_cid: Option<String>,
    #[serde(default)]
    pub signature: Option<String>,
    #[serde(default)]
    pub revoked_at: Option<i64>,
}

impl PluginManifestV1 {
    pub fn compatible_artifact(&self, platform: &str, arch: &str) -> Option<&PluginArtifactV1> {
        self.artifacts.iter().find(|artifact| {
            artifact.platform.eq_ignore_ascii_case(platform)
                && artifact.architecture.eq_ignore_ascii_case(arch)
        })
    }

    pub fn unsigned_bytes(&self) -> Result<Vec<u8>, ProtocolError> {
        let mut unsigned = self.clone();
        unsigned.signature = None;
        let mut bytes = b"jimmusic:plugin-manifest:v1\0".to_vec();
        bytes.extend(canonical_dag_cbor(&unsigned)?);
        Ok(bytes)
    }
}

impl Validate for PluginManifestV1 {
    fn validate(&self) -> Result<(), ProtocolError> {
        require_v1(self.schema_version)?;
        required(&self.plugin_id, "plugin_id")?;
        required(&self.name, "name")?;
        required(&self.version, "version")?;
        required(&self.publisher, "publisher")?;
        required(&self.plugin_kind, "plugin_kind")?;
        required(&self.minimum_core_version, "minimum_core_version")?;
        required(&self.maximum_core_version, "maximum_core_version")?;
        required(&self.configuration_schema_cid, "configuration_schema_cid")?;
        required(&self.license, "license")?;
        if self.interface_versions.is_empty() || self.artifacts.is_empty() {
            return Err(ProtocolError::MissingField("interface_versions/artifacts"));
        }
        for artifact in &self.artifacts {
            artifact.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PluginLifecycleState {
    Available,
    Downloading,
    Verifying,
    Staged,
    Installed,
    Enabled,
    Disabled,
    UpdateAvailable,
    Revoked,
    Failed,
    Incompatible,
    Quarantined,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransferKind {
    Fetch,
    Download,
    Publish,
    Pin,
    Plugin,
    Report,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransferState {
    Queued,
    Resolving,
    Transferring,
    Paused,
    Verifying,
    Committing,
    Completed,
    Failed,
    Cancelled,
    IntegrityFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NetworkPolicyV1 {
    pub wifi_only: bool,
    #[serde(default)]
    pub cellular_limit_bytes: Option<u64>,
    pub max_concurrency: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransferTaskV1 {
    pub schema_version: u16,
    pub task_id: String,
    pub kind: TransferKind,
    pub target_cid: String,
    pub state: TransferState,
    /// Scheduler priority in the inclusive range -100..=100. Higher queued
    /// values start first; active transfers are never silently preempted.
    #[serde(default)]
    pub priority: i16,
    #[serde(default)]
    pub bytes_total: Option<u64>,
    pub bytes_completed: u64,
    pub speed_bytes_per_second: u64,
    #[serde(default)]
    pub providers: Vec<String>,
    pub retry_count: u32,
    #[serde(default)]
    pub next_retry_at: Option<i64>,
    pub network_policy: NetworkPolicyV1,
    #[serde(default)]
    pub destination: Option<String>,
    #[serde(default)]
    pub error: Option<ErrorEnvelopeV1>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NodeStatusV1 {
    pub schema_version: u16,
    pub peer_id: String,
    pub lifecycle_state: String,
    #[serde(default)]
    pub transports: Vec<String>,
    /// Dialable libp2p multiaddresses, including the `/p2p/<PeerId>` suffix.
    #[serde(default)]
    pub listen_addresses: Vec<String>,
    /// Currently authenticated and connected peer identifiers.
    #[serde(default)]
    pub peers: Vec<String>,
    pub connected_peers: u32,
    pub routing_status: String,
    pub repository_bytes: u64,
    pub cache_bytes: u64,
    pub pinned_bytes: u64,
    pub bytes_up: u64,
    pub bytes_down: u64,
    #[serde(default)]
    pub limitations: Vec<String>,
    #[serde(default)]
    pub last_error: Option<ErrorEnvelopeV1>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderHealthState {
    Healthy,
    Degraded,
    Unknown,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderHealthV1 {
    pub schema_version: u16,
    pub cid: String,
    pub observed_providers: u32,
    #[serde(default)]
    pub last_success_at: Option<i64>,
    #[serde(default)]
    pub latency_ms: Option<u64>,
    pub local_pin: bool,
    #[serde(default)]
    pub configured_pin_services: Vec<String>,
    pub health: ProviderHealthState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ErrorEnvelopeV1 {
    pub schema_version: u16,
    pub code: String,
    pub message: String,
    pub subsystem: String,
    pub operation: String,
    pub retryable: bool,
    #[serde(default)]
    pub unsupported_reason: Option<String>,
    #[serde(default)]
    pub details: BTreeMap<String, String>,
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub causes: Vec<String>,
}

impl ErrorEnvelopeV1 {
    pub fn unsupported(
        subsystem: impl Into<String>,
        operation: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: SCHEMA_V1,
            code: "unsupported".into(),
            message: "the requested capability is not supported".into(),
            subsystem: subsystem.into(),
            operation: operation.into(),
            retryable: false,
            unsupported_reason: Some(reason.into()),
            details: BTreeMap::new(),
            request_id: None,
            causes: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EventEnvelopeV1<T> {
    pub schema_version: u16,
    pub sequence: u64,
    pub timestamp: i64,
    pub event_type: String,
    pub entity_id: String,
    #[serde(default)]
    pub request_id: Option<String>,
    pub payload: T,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AudioMediaType {
    Pcm,
    Dsd,
    Encoded,
    Control,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AudioFormatSpecV1 {
    pub media_type: AudioMediaType,
    pub sample_type: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub channel_layout: String,
    pub packing: String,
    pub endian: String,
    #[serde(default)]
    pub bit_exact: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AudioPortSpecV1 {
    pub port_id: String,
    pub media_type: AudioMediaType,
    pub format: AudioFormatSpecV1,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AudioNodeType {
    Decoder,
    Processor,
    Analyzer,
    Resampler,
    Transition,
    Mixer,
    Output,
    Passthrough,
    EncodedPassthrough,
    FormatConverter,
    Delay,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NodeFailurePolicy {
    Bypass,
    RollbackGraph,
    Stop,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AudioNodeSpecV1 {
    pub node_id: String,
    pub node_type: AudioNodeType,
    pub plugin_id: String,
    pub plugin_version: String,
    #[serde(default)]
    pub inputs: Vec<AudioPortSpecV1>,
    #[serde(default)]
    pub outputs: Vec<AudioPortSpecV1>,
    pub latency_frames: u32,
    pub tail_frames: u32,
    pub realtime_safe: bool,
    pub failure_policy: NodeFailurePolicy,
    #[serde(default)]
    pub state_cid: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AudioEdgeSpecV1 {
    pub from_node: String,
    pub from_port: String,
    pub to_node: String,
    pub to_port: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AudioGraphMode {
    Normal,
    LowLatency,
    BitPerfect,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AudioGraphSpecV1 {
    pub schema_version: u16,
    pub graph_id: String,
    pub version: u64,
    pub created_by: String,
    pub nodes: Vec<AudioNodeSpecV1>,
    pub edges: Vec<AudioEdgeSpecV1>,
    pub output_node: String,
    pub mode: AudioGraphMode,
    pub allow_format_conversion: bool,
    pub cpu_budget_micros: u64,
    pub memory_budget_bytes: u64,
    pub latency_budget_frames: u32,
}

impl Validate for AudioGraphSpecV1 {
    fn validate(&self) -> Result<(), ProtocolError> {
        require_v1(self.schema_version)?;
        required(&self.graph_id, "graph_id")?;
        required(&self.output_node, "output_node")?;
        if self.nodes.is_empty() {
            return Err(ProtocolError::MissingField("nodes"));
        }
        let ids: BTreeSet<&str> = self.nodes.iter().map(|n| n.node_id.as_str()).collect();
        if ids.len() != self.nodes.len() {
            return Err(ProtocolError::InvalidField {
                field: "nodes",
                reason: "node IDs must be unique".into(),
            });
        }
        if !ids.contains(self.output_node.as_str()) {
            return Err(ProtocolError::InvalidField {
                field: "output_node",
                reason: "output node does not exist".into(),
            });
        }
        for node in &self.nodes {
            required(&node.node_id, "node_id")?;
            required(&node.plugin_id, "plugin_id")?;
            required(&node.plugin_version, "plugin_version")?;
            for port in node.inputs.iter().chain(&node.outputs) {
                required(&port.port_id, "port_id")?;
                if port.media_type != port.format.media_type {
                    return Err(ProtocolError::InvalidField {
                        field: "media_type",
                        reason: format!(
                            "node `{}` port `{}` disagrees with its format media type",
                            node.node_id, port.port_id
                        ),
                    });
                }
                match port.media_type {
                    AudioMediaType::Dsd
                        if !matches!(
                            port.format.sample_type.as_str(),
                            "dsd_u8" | "encoded_bytes"
                        ) =>
                    {
                        return Err(ProtocolError::InvalidField {
                            field: "sample_type",
                            reason: "DSD ports require dsd_u8 or encoded_bytes".into(),
                        });
                    }
                    AudioMediaType::Encoded if port.format.sample_type != "encoded_bytes" => {
                        return Err(ProtocolError::InvalidField {
                            field: "sample_type",
                            reason: "encoded ports require encoded_bytes".into(),
                        });
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rendition() -> MusicRenditionV1 {
        MusicRenditionV1 {
            rendition_id: "original".into(),
            content_cid: "bafy-content".into(),
            container: "flac".into(),
            codec: "flac".into(),
            profile: String::new(),
            sample_rate: 44_100,
            bit_depth: 24,
            channels: 2,
            channel_layout: "stereo".into(),
            duration_ms: 180_000,
            byte_length: 1_024,
            lossless: true,
            original: true,
            streamable: true,
        }
    }

    fn manifest() -> MusicManifestV1 {
        MusicManifestV1 {
            schema_version: SCHEMA_V1,
            work_id: "work-1".into(),
            release_id: "release-1".into(),
            title: "A".into(),
            artists: vec!["B".into()],
            album: String::new(),
            track_number: Some(1),
            disc_number: Some(1),
            duration_ms: 180_000,
            language: "zh-CN".into(),
            genres: vec!["electronic".into()],
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
            renditions: vec![rendition()],
            publisher_identity_cid: "bafy-publisher".into(),
            created_at: 1,
            updated_at: 1,
            publisher_signature: None,
        }
    }

    #[test]
    fn canonical_map_order_is_stable() {
        let a = serde_json::json!({"z": 1, "aa": 2, "b": 3});
        let b = serde_json::json!({"b": 3, "z": 1, "aa": 2});
        assert_eq!(
            canonical_dag_cbor(&a).unwrap(),
            canonical_dag_cbor(&b).unwrap()
        );
        assert_eq!(cid_v1_for(&a).unwrap(), cid_v1_for(&b).unwrap());
        assert!(cid_v1_for(&a).unwrap().starts_with('b'));
    }

    #[test]
    fn signed_bytes_exclude_signature() {
        let mut a = manifest();
        let mut b = a.clone();
        a.publisher_signature = Some("one".into());
        b.publisher_signature = Some("two".into());
        assert_eq!(a.unsigned_bytes().unwrap(), b.unsigned_bytes().unwrap());
    }

    #[test]
    fn canonical_decoder_round_trips_and_rejects_noncanonical_integer() {
        let value = BTreeMap::from([("answer".to_string(), 42u64)]);
        let bytes = canonical_dag_cbor(&value).unwrap();
        assert_eq!(
            decode_dag_cbor::<BTreeMap<String, u64>>(&bytes).unwrap(),
            value
        );
        assert!(decode_dag_cbor::<u64>(&[0x18, 0x01]).is_err());
    }

    #[test]
    fn manifest_validation_rejects_duplicate_renditions() {
        let mut value = manifest();
        value.renditions.push(rendition());
        assert!(value.validate().is_err());
    }

    #[test]
    fn limits_reject_deep_or_large_objects() {
        let value = serde_json::json!({"x": {"y": {"z": true}}});
        let limits = ObjectLimits {
            max_depth: 1,
            ..ObjectLimits::default()
        };
        assert!(matches!(
            canonical_dag_cbor_with_limits(&value, limits),
            Err(ProtocolError::LimitExceeded { .. })
        ));
    }

    #[test]
    fn floating_point_is_rejected() {
        assert_eq!(
            canonical_dag_cbor(&serde_json::json!({"bad": 1.5})),
            Err(ProtocolError::FloatingPoint)
        );
    }

    #[test]
    fn plugin_selects_only_matching_artifact() {
        let manifest = PluginManifestV1 {
            schema_version: SCHEMA_V1,
            plugin_id: "dev.test.output".into(),
            name: "Test".into(),
            version: "1.0.0".into(),
            publisher: "publisher".into(),
            plugin_kind: "audio_output".into(),
            interface_versions: BTreeMap::from([("audio_output".into(), "2".into())]),
            minimum_core_version: "2.0.0".into(),
            maximum_core_version: "2.x".into(),
            artifacts: vec![PluginArtifactV1 {
                artifact_cid: "bafy-artifact".into(),
                platform: "linux".into(),
                architecture: "x86_64".into(),
                runtime: PluginRuntime::Native,
                entrypoint: "libtest.so".into(),
                byte_length: 1,
                sha256: "00".repeat(32),
                provenance_cid: None,
                sbom_cid: None,
                sandbox_profile: "native-official".into(),
                required_host_capabilities: Vec::new(),
                hardware_requirements: Vec::new(),
            }],
            capabilities: Vec::new(),
            permissions: BTreeSet::new(),
            dependencies: Vec::new(),
            conflicts: Vec::new(),
            configuration_schema_cid: "bafy-schema".into(),
            state_schema_version: 1,
            license: "GPL-3.0-only".into(),
            release_notes_cid: None,
            previous_release_cid: None,
            signature: Some("sig".into()),
            revoked_at: None,
        };
        assert!(manifest.compatible_artifact("linux", "x86_64").is_some());
        assert!(manifest.compatible_artifact("windows", "x86_64").is_none());
    }

    #[test]
    fn maintainer_rotation_requires_both_key_proofs() {
        let mut event = MaintainerKeyEventV1 {
            schema_version: SCHEMA_V1,
            source_id: "community.example".into(),
            action: MaintainerKeyAction::Rotate,
            sequence: 0,
            previous_event_cid: None,
            current_public_key: "11".repeat(32),
            new_public_key: Some("22".repeat(32)),
            issued_at: 1,
            signature: Some("33".repeat(64)),
            new_key_proof: None,
        };
        assert!(matches!(
            event.validate(),
            Err(ProtocolError::MissingField("new_key_proof"))
        ));
        event.new_key_proof = Some("44".repeat(64));
        assert!(event.validate().is_ok());
    }

    #[test]
    fn anonymous_report_cannot_embed_reporter_identity() {
        let report = ModerationReportV1 {
            schema_version: SCHEMA_V1,
            report_id: "report-1".into(),
            target: "bafy-target".into(),
            reason_code: "copyright".into(),
            description: String::new(),
            evidence_cids: Vec::new(),
            reporter_identity: Some("bafy-identity".into()),
            reporter_public_key: "11".repeat(32),
            anonymous: true,
            recipient_source_id: "community.example".into(),
            created_at: 1,
            signature: Some("22".repeat(64)),
            encrypted_envelope: None,
        };
        assert!(matches!(
            report.validate(),
            Err(ProtocolError::InvalidField {
                field: "reporter_identity",
                ..
            })
        ));
    }
}
