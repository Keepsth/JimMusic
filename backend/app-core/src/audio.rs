//! 有界 PCM 缓冲队列（需求 3.1「播放引擎」：解码与播放的速率匹配与背压）。
//!
//! 解码器插件是 PCM 生产者，音频输出插件是 PCM 消费者；两者速率天然不一致。
//! [`PcmQueue`] 在两者之间放置一个有界通道：
//! - 队列满时 [`PcmQueue::push`] 阻塞（背压），迫使生产者放慢解码；
//! - 消费者端（`tokio::sync::mpsc::Receiver`）在队列空时 `recv().await` 阻塞；
//! - 释放任一端即关闭：消费者 drop 接收端后，生产者 `push` 返回 [`PcmQueueClosed`]；
//!   生产者 drop 发送端后，消费者 `recv` 返回 `None`。
//!
//! 实现基于 Tokio 有界 MPSC 通道，无额外依赖，跨平台（含移动端/Web）。

use tokio::sync::mpsc;

/// A real playlist boundary carried with the first PCM block of the new
/// timeline. The output pump emits state events only when this block reaches
/// the sink, not when the decoder happens to prefetch it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackBoundary {
    pub from_track_id: String,
    pub to_track_id: String,
    pub mode: String,
    pub overlap_frames: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlaybackChunkMetadata {
    pub track_id: String,
    pub track_start_frame: u64,
    pub track_total_frames: u64,
    pub duration_secs: f64,
    pub boundary_before: Option<TrackBoundary>,
}

/// 一段交错 PCM 数据块（解码器产出、输出插件消费的最小单元）。
#[derive(Debug, Clone)]
pub struct PcmChunk {
    /// 采样率（Hz）。同一播放会话内应恒定。
    pub sample_rate: u32,
    /// 声道数。
    pub channels: u16,
    /// 交错 PCM 样本（i16）。`samples.len() % channels == 0`。
    pub samples: Vec<i16>,
    /// Optional playlist timing metadata. Decoder/output plugin ABI chunks can
    /// leave this empty; the playback engine attaches it for queue timelines.
    pub playback: Option<PlaybackChunkMetadata>,
}

impl PcmChunk {
    /// 新建一个 PCM 块。
    pub fn new(sample_rate: u32, channels: u16, samples: Vec<i16>) -> Self {
        Self {
            sample_rate,
            channels,
            samples,
            playback: None,
        }
    }

    pub fn with_playback(mut self, playback: PlaybackChunkMetadata) -> Self {
        self.playback = Some(playback);
        self
    }

    /// 采样帧数（`samples.len() / channels`）。
    pub fn frames(&self) -> usize {
        let ch = self.channels.max(1) as usize;
        self.samples.len() / ch
    }

    /// 是否为空块。
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }
}

/// 队列已关闭错误（接收端被释放后仍尝试写入）。
#[derive(Debug, thiserror::Error)]
#[error("pcm queue is closed")]
pub struct PcmQueueClosed;

/// 有界 PCM 缓冲队列的生产者端（解码器 → 输出插件）。
///
/// 消费者端为同 `channel` 创建的 [`mpsc::Receiver`]，由播放引擎的泵任务独占。
pub struct PcmQueue {
    sender: mpsc::Sender<PcmChunk>,
    capacity: usize,
}

impl PcmQueue {
    /// 创建有界队列，返回（生产者句柄，消费者接收端）。
    pub fn channel(capacity: usize) -> (Self, mpsc::Receiver<PcmChunk>) {
        let cap = capacity.max(1);
        let (sender, receiver) = mpsc::channel(cap);
        (
            Self {
                sender,
                capacity: cap,
            },
            receiver,
        )
    }

    /// 队列容量（块数）。
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 接收端是否已被释放（消费已停止）。
    pub fn is_closed(&self) -> bool {
        self.sender.is_closed()
    }

    /// 写入一个 PCM 块。队列满时阻塞（背压）；接收端已释放时返回 [`PcmQueueClosed`]。
    pub async fn push(&self, chunk: PcmChunk) -> Result<(), PcmQueueClosed> {
        self.sender.send(chunk).await.map_err(|_| PcmQueueClosed)
    }

    /// 从专用阻塞解码线程写入。不得在 Tokio 异步工作线程或实时音频线程调用。
    pub fn blocking_push(&self, chunk: PcmChunk) -> Result<(), PcmQueueClosed> {
        self.sender.blocking_send(chunk).map_err(|_| PcmQueueClosed)
    }

    /// 尝试非阻塞写入。队列满或已关闭时返回 `Err`（不阻塞）。
    pub fn try_push(&self, chunk: PcmChunk) -> Result<(), PcmQueueClosed> {
        self.sender.try_send(chunk).map_err(|_| PcmQueueClosed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn chunk(tag: i16) -> PcmChunk {
        PcmChunk::new(44_100, 1, vec![tag])
    }

    #[tokio::test]
    async fn receiver_gets_pushed_in_order() {
        let (q, mut rx) = PcmQueue::channel(4);
        q.push(chunk(1)).await.unwrap();
        q.push(chunk(2)).await.unwrap();

        assert_eq!(rx.recv().await.unwrap().samples, vec![1]);
        assert_eq!(rx.recv().await.unwrap().samples, vec![2]);
    }

    #[tokio::test]
    async fn push_blocks_when_full_then_unblocks() {
        let (q, mut rx) = PcmQueue::channel(2);
        q.push(chunk(1)).await.unwrap();
        q.push(chunk(2)).await.unwrap();

        // 队列已满，第三次 push 应因背压而阻塞。
        let pushed = tokio::spawn({
            let q = std::sync::Arc::new(q);
            let q = q.clone();
            async move { q.push(chunk(3)).await.unwrap() }
        });

        // 短时间内不应完成（背压生效）。
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!pushed.is_finished());

        // 消费一个，解除背压。
        let _ = rx.recv().await.unwrap();
        pushed.await.unwrap();
    }

    #[tokio::test]
    async fn try_push_fails_when_full() {
        let (q, _rx) = PcmQueue::channel(1);
        q.try_push(chunk(1)).unwrap();
        assert!(q.try_push(chunk(2)).is_err());
    }

    #[tokio::test]
    async fn dropping_receiver_closes_queue() {
        let (q, rx) = PcmQueue::channel(2);
        assert!(!q.is_closed());
        drop(rx);
        assert!(q.is_closed());
        // 关闭后 push 报错。
        assert!(q.push(chunk(1)).await.is_err());
    }

    #[tokio::test]
    async fn frames_computes_interleaved_count() {
        // 2 声道，4 个样本 = 2 帧。
        let c = PcmChunk::new(48_000, 2, vec![1, 2, 3, 4]);
        assert_eq!(c.frames(), 2);
        // 空块帧数为 0。
        assert_eq!(PcmChunk::new(0, 0, vec![]).frames(), 0);
    }
}
