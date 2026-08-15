//! IPFS 网络接入。
//!
//! [`IpfsClient`] 通过 IPFS 节点的 HTTP API（`/api/v0/`）执行：
//! - CID 查询（`block stat`）；
//! - 数据下载与流式传输（`cat`）；
//! - 数据上传（`add`）与 Pin 管理（`pin add/rm/ls`）；
//! - 内容签名校验（下载字节的真实 SHA-256 摘要校验）。
//!
//! 所有操作基于 `reqwest` 异步客户端，可作为消息总线中的异步任务并发执行。

use std::sync::Arc;

use serde::Deserialize;

/// IPFS 客户端错误。
#[derive(Debug, thiserror::Error)]
pub enum IpfsError {
    /// HTTP 请求失败。
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    /// 节点返回非成功状态。
    #[error("ipfs node returned status {0}")]
    Status(reqwest::StatusCode),
    /// 内容摘要校验失败（防篡改）。
    #[error("content digest mismatch: expected {expected}, actual {actual}")]
    DigestMismatch { expected: String, actual: String },
    /// 非法 CID。
    #[error("invalid cid `{0}`")]
    InvalidCid(String),
}

/// IPFS HTTP API 客户端。
#[derive(Clone)]
pub struct IpfsClient {
    http: reqwest::Client,
    /// 节点 API 基地址，例如 `http://127.0.0.1:5001`。
    base: Arc<str>,
}

impl IpfsClient {
    /// 创建指向给定 API 基地址的客户端。
    pub fn new(base: impl Into<String>) -> Self {
        let base: Arc<str> = base.into().into();
        Self {
            http: reqwest::Client::new(),
            base,
        }
    }

    /// 查询块状态（是否存在及其大小）。
    pub async fn block_stat(&self, cid: &str) -> Result<BlockStat, IpfsError> {
        let url = format!("{}/api/v0/block/stat?arg={}", self.base, cid);
        let resp = self.http.post(&url).send().await?;
        if resp.status() != reqwest::StatusCode::OK {
            return Err(IpfsError::Status(resp.status()));
        }
        Ok(resp.json().await?)
    }

    /// 检索 CID 对应的全部字节内容。
    pub async fn cat(&self, cid: &str) -> Result<Vec<u8>, IpfsError> {
        let url = format!("{}/api/v0/cat?arg={}", self.base, cid);
        let resp = self.http.post(&url).send().await?;
        if resp.status() != reqwest::StatusCode::OK {
            return Err(IpfsError::Status(resp.status()));
        }
        Ok(resp.bytes().await?.to_vec())
    }

    /// 流式检索并累积内容，同时校验 SHA-256 摘要（防篡改）。
    ///
    /// `expected_sha256` 为目标内容的十六进制 SHA-256 摘要；不匹配则返回
    /// [`IpfsError::DigestMismatch`]。
    pub async fn cat_verified(
        &self,
        cid: &str,
        expected_sha256: &str,
    ) -> Result<Vec<u8>, IpfsError> {
        let data = self.cat(cid).await?;
        let actual = sha256_hex(&data);
        if !actual.eq_ignore_ascii_case(expected_sha256) {
            return Err(IpfsError::DigestMismatch {
                expected: expected_sha256.to_string(),
                actual,
            });
        }
        Ok(data)
    }

    /// 流式检索 CID 对应内容（`cat`），返回字节流，供「边下载边播放」使用。
    ///
    /// 返回的流逐块产出字节，无需一次性将全部内容载入内存。
    pub async fn cat_stream(
        &self,
        cid: &str,
    ) -> Result<impl futures::Stream<Item = Result<bytes::Bytes, reqwest::Error>>, IpfsError> {
        let url = format!("{}/api/v0/cat?arg={}", self.base, cid);
        let resp = self.http.post(&url).send().await?;
        if resp.status() != reqwest::StatusCode::OK {
            return Err(IpfsError::Status(resp.status()));
        }
        Ok(resp.bytes_stream())
    }

    /// 上传字节内容，返回其 CID（`add`）。
    pub async fn add(&self, data: &[u8]) -> Result<String, IpfsError> {
        let url = format!("{}/api/v0/add?cid-version=1", self.base);
        let part = reqwest::multipart::Part::bytes(data.to_vec())
            .file_name("blob")
            .mime_str("application/octet-stream")
            .map_err(IpfsError::from)?;
        let form = reqwest::multipart::Form::new().part("file", part);
        let resp = self.http.post(&url).multipart(form).send().await?;
        if resp.status() != reqwest::StatusCode::OK {
            return Err(IpfsError::Status(resp.status()));
        }
        let added: AddResponse = resp.json().await?;
        Ok(added.hash)
    }

    /// Pin 一个 CID（`pin add`），返回被 pin 的 CID。
    pub async fn pin_add(&self, cid: &str) -> Result<Vec<String>, IpfsError> {
        self.pin_list_operation("add", cid).await
    }

    /// 取消 Pin（`pin rm`）。
    pub async fn pin_rm(&self, cid: &str) -> Result<Vec<String>, IpfsError> {
        self.pin_list_operation("rm", cid).await
    }

    /// 列出已 Pin 的 CID（`pin ls`）。
    pub async fn pin_ls(&self) -> Result<Vec<String>, IpfsError> {
        let url = format!("{}/api/v0/pin/ls?type=recursive", self.base);
        let resp = self.http.post(&url).send().await?;
        if resp.status() != reqwest::StatusCode::OK {
            return Err(IpfsError::Status(resp.status()));
        }
        // pin ls 返回 JSON 对象 { "<cid>": {"Type": "recursive"} }。
        let map: serde_json::Map<String, serde_json::Value> = resp.json().await?;
        Ok(map.into_iter().map(|(k, _)| k).collect())
    }

    async fn pin_list_operation(&self, op: &str, cid: &str) -> Result<Vec<String>, IpfsError> {
        let url = format!("{}/api/v0/pin/{op}?arg={}", self.base, cid);
        let resp = self.http.post(&url).send().await?;
        if resp.status() != reqwest::StatusCode::OK {
            return Err(IpfsError::Status(resp.status()));
        }
        // pin add / rm 返回 JSON 对象 { "Pins": ["<cid>", ...] }。
        let map: serde_json::Map<String, serde_json::Value> = resp.json().await?;
        let pins = map
            .get("Pins")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        Ok(pins)
    }
}

/// `block/stat` 响应。
#[derive(Debug, Deserialize)]
pub struct BlockStat {
    /// 块键（CID）。
    #[serde(rename = "Key")]
    pub key: String,
    /// 块大小（字节）。
    #[serde(rename = "Size")]
    pub size: u64,
}

/// `add` 响应。
#[derive(Debug, Deserialize)]
pub struct AddResponse {
    /// 新增内容的 CID。
    #[serde(rename = "Hash")]
    pub hash: String,
}

/// 计算字节的十六进制 SHA-256 摘要。
pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(data);
    hex::encode(digest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn mock_server() -> MockServer {
        MockServer::start().await
    }

    #[tokio::test]
    async fn client_constructs() {
        let client = IpfsClient::new("http://127.0.0.1:5001");
        assert!(!client.base.is_empty());
    }

    #[test]
    fn sha256_hex_is_real_sha256() {
        // 已知向量的 SHA-256 摘要（空串与 "hello world"）。
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"hello world"),
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn sha256_hex_is_stable_and_distinct() {
        assert_eq!(sha256_hex(b"hello"), sha256_hex(b"hello"));
        assert_ne!(sha256_hex(b"hello"), sha256_hex(b"world"));
    }

    #[tokio::test]
    async fn cat_returns_bytes() {
        let server = mock_server().await;
        Mock::given(method("POST"))
            .and(path("/api/v0/cat"))
            .and(query_param("arg", "QmTest"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hello".to_vec()))
            .mount(&server)
            .await;

        let client = IpfsClient::new(server.uri());
        assert_eq!(client.cat("QmTest").await.unwrap(), b"hello");
    }

    #[tokio::test]
    async fn cat_verified_checks_digest() {
        let server = mock_server().await;
        Mock::given(method("POST"))
            .and(path("/api/v0/cat"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hello".to_vec()))
            .mount(&server)
            .await;

        let client = IpfsClient::new(server.uri());
        let expected = sha256_hex(b"hello");
        assert_eq!(
            client.cat_verified("QmTest", &expected).await.unwrap(),
            b"hello"
        );
        let err = client.cat_verified("QmTest", "deadbeef").await.unwrap_err();
        assert!(matches!(err, IpfsError::DigestMismatch { .. }));
    }

    #[tokio::test]
    async fn block_stat_returns_size() {
        let server = mock_server().await;
        Mock::given(method("POST"))
            .and(path("/api/v0/block/stat"))
            .and(query_param("arg", "QmTest"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Key": "QmTest",
                "Size": 5u64
            })))
            .mount(&server)
            .await;

        let client = IpfsClient::new(server.uri());
        let stat = client.block_stat("QmTest").await.unwrap();
        assert_eq!(stat.key, "QmTest");
        assert_eq!(stat.size, 5);
    }

    #[tokio::test]
    async fn add_returns_cid() {
        let server = mock_server().await;
        Mock::given(method("POST"))
            .and(path("/api/v0/add"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Hash": "QmAdded"
            })))
            .mount(&server)
            .await;

        let client = IpfsClient::new(server.uri());
        assert_eq!(client.add(b"data").await.unwrap(), "QmAdded");
    }

    #[tokio::test]
    async fn pin_add_and_ls() {
        let server = mock_server().await;
        Mock::given(method("POST"))
            .and(path("/api/v0/pin/add"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Pins": ["QmTest"]
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v0/pin/ls"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "QmTest": {"Type": "recursive"}
            })))
            .mount(&server)
            .await;

        let client = IpfsClient::new(server.uri());
        assert_eq!(client.pin_add("QmTest").await.unwrap(), vec!["QmTest"]);
        assert_eq!(client.pin_ls().await.unwrap(), vec!["QmTest"]);
    }

    #[tokio::test]
    async fn cat_stream_accumulates_bytes() {
        use futures::StreamExt;
        let server = mock_server().await;
        let body = vec![0u8; 100];
        Mock::given(method("POST"))
            .and(path("/api/v0/cat"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
            .mount(&server)
            .await;

        let client = IpfsClient::new(server.uri());
        let mut stream = client.cat_stream("QmTest").await.unwrap();
        let mut acc = Vec::new();
        while let Some(chunk) = stream.next().await {
            acc.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(acc, body);
    }

    #[tokio::test]
    async fn cat_non_ok_status_is_error() {
        let server = mock_server().await;
        Mock::given(method("POST"))
            .and(path("/api/v0/cat"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let client = IpfsClient::new(server.uri());
        let err = client.cat("QmMissing").await.unwrap_err();
        assert!(matches!(err, IpfsError::Status(s) if s == reqwest::StatusCode::NOT_FOUND));
    }
}
