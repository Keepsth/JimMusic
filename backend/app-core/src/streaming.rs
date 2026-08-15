//! 流式下载与流式解码（需求 3.6：边下载边播放）。
//!
//! [`download_and_decode`] 将 IPFS 的流式下载（[`IpfsClient::cat_stream`]）与
//! Symphonia 的从内存解码（`symphonia_decoder::decode_from_reader`）串联：
//! 逐块下载 CID 内容到内存后直接解码为 PCM，无需先落盘。

use std::io::Cursor;

use bytes::BytesMut;
use futures::StreamExt;

use crate::ipfs::{IpfsClient, IpfsError};

/// 流式下载 + 解码错误。
#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    /// 下载失败。
    #[error("download error: {0}")]
    Download(#[from] IpfsError),
    /// 解码失败。
    #[error("decode error: {0}")]
    Decode(#[from] symphonia_decoder::DecodeError),
}

/// 从 IPFS 流式下载 CID 内容并解码为 PCM 样本（边下载边播放的简化实现）。
///
/// 返回 [`symphonia_decoder::DecodedAudio`]：`sample_rate` / `channels` / `frames` /
/// `samples`（i16 交错 PCM）。
pub async fn download_and_decode(
    client: &IpfsClient,
    cid: &str,
) -> Result<symphonia_decoder::DecodedAudio, StreamError> {
    let mut stream = client.cat_stream(cid).await?;
    let mut buf = BytesMut::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(IpfsError::from)?;
        buf.extend_from_slice(&chunk);
    }

    let reader = Cursor::new(buf.to_vec());
    let audio = symphonia_decoder::decode_from_reader(reader)?;
    Ok(audio)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// 生成一个最小有效 WAV（8000Hz、单声道、16-bit、含 N 个采样）。
    fn wav_bytes(n: usize, sample_rate: u32) -> Vec<u8> {
        let data_len = (n * 2) as u32;
        let byte_rate = sample_rate * 2;
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // PCM
        out.extend_from_slice(&1u16.to_le_bytes()); // mono
        out.extend_from_slice(&sample_rate.to_le_bytes());
        out.extend_from_slice(&byte_rate.to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        for i in 0..n {
            let s = ((i as f64).sin() * 1000.0) as i16;
            out.extend_from_slice(&s.to_le_bytes());
        }
        out
    }

    #[tokio::test]
    async fn download_and_decode_wav() {
        let server = MockServer::start().await;
        let bytes = wav_bytes(800, 8000);
        Mock::given(method("POST"))
            .and(path("/api/v0/cat"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes))
            .mount(&server)
            .await;

        let client = IpfsClient::new(server.uri());
        let audio = download_and_decode(&client, "QmTest").await.unwrap();
        assert_eq!(audio.sample_rate, 8000);
        assert_eq!(audio.channels, 1);
        assert_eq!(audio.frames, 800);
    }

    #[tokio::test]
    async fn download_failure_maps_to_stream_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v0/cat"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let client = IpfsClient::new(server.uri());
        let result = download_and_decode(&client, "QmMissing").await;
        assert!(matches!(result, Err(StreamError::Download(_))));
    }
}
