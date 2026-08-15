//! 播放引擎（Playback Engine，需求 3.1）。
//!
//! [`PlaybackEngine`] 是 Core 的音频中枢：串联**解码器**（PCM 生产者）与**音频输出插件**
//! （PCM 消费者），中间以有界 [`PcmQueue`] 做速率匹配与背压，并维护播放状态机
//! （stopped / playing / paused / seeking），经 [`EventBus`] 广播播放/暂停/进度事件，
//! 支持运行时切换输出插件（停止 → 关闭旧输出 → 加载新输出 → 恢复播放）。
//!
//! 输出后端抽象为 [`PcmSink`]：真实路径由 [`FfiSink`] 封装 [`OutputStream`]，
//! 测试路径可用内存实现，从而在无音频设备环境下验证完整流水线。

use std::collections::VecDeque;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{watch, Mutex};
use tokio::task::JoinHandle;

use crate::audio::{PcmChunk, PcmQueue, PlaybackChunkMetadata, TrackBoundary};
use crate::event::{Event, EventBus, PlaybackFailure};
use crate::output::{OutputError, OutputStream};

/// 每个 PCM 块的采样帧数（解码器切块粒度）。
const CHUNK_FRAMES: usize = 512;
/// 有界缓冲队列容量（块数）。
const QUEUE_CAPACITY: usize = 8;
/// 背压重试间隔（输出缓冲满时）。
const BACKPRESSURE_RETRY: Duration = Duration::from_millis(1);
const MAX_CROSSFADE_SECS: f64 = 30.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrossfadeCurve {
    Linear,
    EqualPower,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlaylistTransition {
    pub crossfade_secs: f64,
    pub curve: CrossfadeCurve,
}

impl PlaylistTransition {
    pub const GAPLESS: Self = Self {
        crossfade_secs: 0.0,
        curve: CrossfadeCurve::EqualPower,
    };

    pub fn crossfade(seconds: f64, curve: CrossfadeCurve) -> Self {
        Self {
            crossfade_secs: seconds.clamp(0.0, MAX_CROSSFADE_SECS),
            curve,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PlaylistTrack {
    pub track_id: String,
    pub path: PathBuf,
}

#[derive(Clone)]
struct PreparedPlaylistTrack {
    track_id: String,
    path: PathBuf,
    sample_rate: u32,
    channels: u16,
    duration_secs: f64,
    total_frames: u64,
}

/// 播放引擎状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EngineState {
    /// 停止（默认）。
    #[default]
    Stopped,
    Playing,
    Paused,
}

/// PCM 消费者（输出后端）抽象。真实实现为 [`FfiSink`]。
pub trait PcmSink: Send + Sync {
    /// 推模型写入交错 PCM，返回实际入队帧数（`0` 表示缓冲满 → 背压）。
    fn write(&self, samples: &[i16], frames: u32) -> Result<u32, OutputError>;
    /// 开始播放。
    fn play(&self) -> Result<(), OutputError>;
    /// 暂停。
    fn pause(&self) -> Result<(), OutputError>;
    /// 停止。
    fn stop(&self) -> Result<(), OutputError>;
    /// 冲刷缓冲。
    fn flush(&self) -> Result<(), OutputError>;
    /// 当前缓冲帧数。
    fn buffered_frames(&self) -> u32;
}

/// 基于 [`OutputStream`] 的 FFI 输出后端。
pub struct FfiSink {
    stream: OutputStream,
}

impl FfiSink {
    /// 包装一个已打开的输出流。
    pub fn new(stream: OutputStream) -> Self {
        Self { stream }
    }

    /// 底层输出流。
    pub fn stream(&self) -> &OutputStream {
        &self.stream
    }
}

impl PcmSink for FfiSink {
    fn write(&self, samples: &[i16], frames: u32) -> Result<u32, OutputError> {
        self.stream.write(samples, frames)
    }

    fn play(&self) -> Result<(), OutputError> {
        self.stream.play()
    }

    fn pause(&self) -> Result<(), OutputError> {
        self.stream.pause()
    }

    fn stop(&self) -> Result<(), OutputError> {
        self.stream.stop()
    }

    fn flush(&self) -> Result<(), OutputError> {
        self.stream.flush()
    }

    fn buffered_frames(&self) -> u32 {
        self.stream.buffered_frames()
    }
}

/// 引擎内部状态。
#[derive(Clone)]
enum EngineSource {
    Pcm(Arc<[i16]>),
    File(PathBuf),
    Sequence(Arc<[PreparedPlaylistTrack]>, PlaylistTransition),
}

#[derive(Default)]
struct EngineInner {
    state: EngineState,
    /// 当前曲目 ID（供事件载荷）。
    track_id: Option<String>,
    /// 当前曲目采样率/声道/时长（秒）。
    sample_rate: u32,
    channels: u16,
    duration_secs: f64,
    /// 当前播放位置（秒）。
    position_secs: f64,
    /// PCM 测试/调用方源或可重新打开的文件源。文件 PCM 永不整首常驻内存。
    source: Option<EngineSource>,
}

/// 播放引擎。
pub struct PlaybackEngine {
    bus: EventBus,
    sink: Mutex<Option<Arc<dyn PcmSink>>>,
    /// 供泵任务观察的播放状态（暂停/恢复/停止信号）。
    state_tx: watch::Sender<EngineState>,
    inner: Arc<Mutex<EngineInner>>,
    producer: Mutex<Option<JoinHandle<()>>>,
    pump: Mutex<Option<JoinHandle<()>>>,
}

impl PlaybackEngine {
    /// 创建播放引擎并绑定事件总线。
    pub fn new(bus: EventBus) -> Arc<Self> {
        let (state_tx, _) = watch::channel(EngineState::Stopped);
        Arc::new(Self {
            bus,
            sink: Mutex::new(None),
            state_tx,
            inner: Arc::new(Mutex::new(EngineInner::default())),
            producer: Mutex::new(None),
            pump: Mutex::new(None),
        })
    }

    /// 当前状态。
    pub async fn state(&self) -> EngineState {
        self.inner.lock().await.state
    }

    /// 当前播放位置（秒）。
    pub async fn position_secs(&self) -> f64 {
        self.inner.lock().await.position_secs
    }

    /// 设置输出后端（未播放时直接替换；播放中请使用 [`Self::switch_output`]）。
    pub async fn set_output(&self, sink: Arc<dyn PcmSink>) {
        *self.sink.lock().await = Some(sink);
    }

    /// 播放一段已解码 PCM（16-bit 交错）。这是引擎的核心入口。
    pub async fn play_pcm(
        self: &Arc<Self>,
        track_id: String,
        sample_rate: u32,
        channels: u16,
        samples: Vec<i16>,
        duration_secs: f64,
    ) {
        self.stop_inner().await;
        {
            let mut inner = self.inner.lock().await;
            inner.track_id = Some(track_id.clone());
            inner.sample_rate = sample_rate;
            inner.channels = channels.max(1);
            inner.duration_secs = duration_secs.max(0.0);
            inner.position_secs = 0.0;
            inner.source = Some(EngineSource::Pcm(Arc::from(samples)));
        }
        self.begin_playback(track_id, 0).await;
    }

    /// 播放一个音频文件：只读取元数据，PCM 在专用阻塞线程中增量解码到有界队列。
    pub async fn play_file(
        self: &Arc<Self>,
        track_id: String,
        path: impl AsRef<Path>,
    ) -> Result<(), OutputError> {
        let path = path.as_ref().to_path_buf();
        let metadata = tokio::task::spawn_blocking({
            let path = path.clone();
            move || symphonia_decoder::read_metadata(&path)
        })
        .await
        .map_err(|e| OutputError::Decode(e.to_string()))?
        .map_err(|e| OutputError::Decode(e.to_string()))?;
        self.stop_inner().await;
        {
            let mut inner = self.inner.lock().await;
            inner.track_id = Some(track_id.clone());
            inner.sample_rate = metadata.sample_rate.unwrap_or(44_100);
            inner.channels = metadata.channels.unwrap_or(2).max(1);
            inner.duration_secs = metadata.duration.unwrap_or(0.0).max(0.0);
            inner.position_secs = 0.0;
            inner.source = Some(EngineSource::File(path));
        }
        self.begin_playback(track_id, 0).await;
        Ok(())
    }

    /// Plays the remaining queue through one uninterrupted output session.
    /// Adjacent tracks are opened as two decoder timelines; a zero-duration
    /// transition is sample-contiguous gapless playback, while a positive
    /// duration mixes a bounded tail/head window before the next decoder
    /// continues. Format mismatches are rejected instead of silently lying
    /// about gapless or resampling in the output backend.
    pub async fn play_file_sequence(
        self: &Arc<Self>,
        tracks: Vec<PlaylistTrack>,
        transition: PlaylistTransition,
    ) -> Result<(), OutputError> {
        if tracks.is_empty() {
            return Err(OutputError::Decode("playlist is empty".into()));
        }
        let prepared = tokio::task::spawn_blocking(move || {
            let mut prepared = Vec::with_capacity(tracks.len());
            for track in tracks {
                let metadata = symphonia_decoder::read_metadata(&track.path)
                    .map_err(|error| OutputError::Decode(error.to_string()))?;
                let sample_rate = metadata.sample_rate.unwrap_or(44_100);
                let channels = metadata.channels.unwrap_or(2).max(1);
                let duration_secs = metadata.duration.unwrap_or(0.0).max(0.0);
                prepared.push(PreparedPlaylistTrack {
                    track_id: track.track_id,
                    path: track.path,
                    sample_rate,
                    channels,
                    duration_secs,
                    total_frames: (duration_secs * sample_rate as f64).round().max(1.0) as u64,
                });
            }
            let first = &prepared[0];
            if prepared.iter().any(|track| {
                track.sample_rate != first.sample_rate || track.channels != first.channels
            }) {
                return Err(OutputError::Decode(
                    "gapless/crossfade queue requires a common sample rate and channel layout"
                        .into(),
                ));
            }
            Ok::<_, OutputError>(prepared)
        })
        .await
        .map_err(|error| OutputError::Decode(error.to_string()))??;
        self.stop_inner().await;
        let first_track_id = prepared[0].track_id.clone();
        let first_sample_rate = prepared[0].sample_rate;
        let first_channels = prepared[0].channels;
        let first_duration_secs = prepared[0].duration_secs;
        {
            let mut inner = self.inner.lock().await;
            inner.track_id = Some(first_track_id.clone());
            inner.sample_rate = first_sample_rate;
            inner.channels = first_channels;
            inner.duration_secs = first_duration_secs;
            inner.position_secs = 0.0;
            inner.source = Some(EngineSource::Sequence(Arc::from(prepared), transition));
        }
        self.begin_playback(first_track_id, 0).await;
        Ok(())
    }

    /// 暂停播放。
    pub async fn pause(&self) {
        if let Some(sink) = self.sink.lock().await.as_ref() {
            let _ = sink.pause();
        }
        let mut inner = self.inner.lock().await;
        if inner.state == EngineState::Playing {
            inner.state = EngineState::Paused;
            self.state_tx.send_replace(EngineState::Paused);
            if let Some(id) = inner.track_id.clone() {
                drop(inner);
                self.bus.publish(Event::Paused { track_id: id });
            }
        }
    }

    /// 恢复播放。
    pub async fn resume(&self) {
        let sink = self.sink.lock().await.as_ref().cloned();
        let mut inner = self.inner.lock().await;
        if inner.state == EngineState::Paused {
            let Some(sink) = sink else {
                let track_id = inner.track_id.clone().unwrap_or_default();
                inner.state = EngineState::Stopped;
                self.state_tx.send_replace(EngineState::Stopped);
                drop(inner);
                self.publish_start_failure(track_id, "output_unavailable", true);
                return;
            };
            if sink.play().is_err() {
                let track_id = inner.track_id.clone().unwrap_or_default();
                inner.state = EngineState::Stopped;
                self.state_tx.send_replace(EngineState::Stopped);
                drop(inner);
                self.publish_start_failure(track_id, "device_start_failed", true);
                return;
            }
            inner.state = EngineState::Playing;
            self.state_tx.send_replace(EngineState::Playing);
            if let Some(id) = inner.track_id.clone() {
                drop(inner);
                self.bus.publish(Event::Played { track_id: id });
            }
        }
    }

    /// 停止播放。
    pub async fn stop(self: &Arc<Self>) {
        self.stop_inner().await;
        self.bus.publish(Event::Stopped);
    }

    /// 跳转到指定位置（秒）。若正在播放，则从新位置恢复播放。
    pub async fn seek(self: &Arc<Self>, position_secs: f64) {
        let (track_id, was_playing, duration, sample_rate, pos) = {
            let mut inner = self.inner.lock().await;
            let pos = position_secs.clamp(0.0, inner.duration_secs.max(0.0));
            inner.position_secs = pos;
            (
                inner.track_id.clone(),
                inner.state == EngineState::Playing,
                inner.duration_secs,
                inner.sample_rate,
                pos,
            )
        };

        let ratio = if duration > 0.0 {
            (pos / duration).clamp(0.0, 1.0)
        } else {
            0.0
        };
        if let Some(id) = &track_id {
            self.bus.publish(Event::Progress {
                track_id: id.clone(),
                position: ratio,
            });
        }

        if was_playing {
            let offset = (pos * sample_rate as f64) as usize;
            if let Some(id) = track_id {
                self.begin_playback(id, offset).await;
            }
        }
    }

    /// 运行时切换输出插件：停止 → 关闭旧输出 → 加载新输出 → 恢复播放。
    pub async fn switch_output(self: &Arc<Self>, sink: Arc<dyn PcmSink>) {
        let (was_playing, track_id, position_secs, sample_rate) = {
            let inner = self.inner.lock().await;
            (
                inner.state == EngineState::Playing,
                inner.track_id.clone(),
                inner.position_secs,
                inner.sample_rate,
            )
        };

        // 停止并关闭旧输出（替换 sink 时旧 OutputStream 被 drop → close）。
        self.stop_inner().await;
        *self.sink.lock().await = Some(sink);

        // 恢复播放（从当前位置）。
        if was_playing {
            let offset = (position_secs * sample_rate as f64) as usize;
            if let Some(id) = track_id {
                self.begin_playback(id, offset).await;
            }
        }
    }

    /// 开始（或恢复）播放。只有输出设备成功启动且任务就绪后才广播 Played。
    async fn begin_playback(self: &Arc<Self>, track_id: String, offset_frames: usize) {
        // 中止旧任务（不复位状态）。
        self.stop_tasks().await;
        if let Err((code, retryable)) = self.start_playback(offset_frames, track_id.clone()).await {
            self.inner.lock().await.state = EngineState::Stopped;
            self.state_tx.send_replace(EngineState::Stopped);
            self.publish_start_failure(track_id, code, retryable);
            return;
        }
        self.inner.lock().await.state = EngineState::Playing;
        self.state_tx.send_replace(EngineState::Playing);
        self.bus.publish(Event::Played { track_id });
    }

    fn publish_start_failure(&self, track_id: String, code: &str, retryable: bool) {
        self.bus.publish(Event::PlaybackFailed {
            track_id,
            error: PlaybackFailure {
                source: "audio_output".into(),
                stage: "open".into(),
                code: code.into(),
                retryable,
                suggestion: "Select an available audio output and retry".into(),
            },
        });
    }

    /// 中止生产者与泵任务并停止/冲刷输出设备（不改变引擎状态）。
    async fn stop_tasks(&self) {
        if let Some(h) = self.pump.lock().await.take() {
            h.abort();
        }
        if let Some(h) = self.producer.lock().await.take() {
            h.abort();
        }
        if let Some(sink) = self.sink.lock().await.as_ref() {
            let _ = sink.stop();
            let _ = sink.flush();
        }
    }

    /// 内部：完全停止（中止任务、关闭输出、复位状态为 Stopped），不发布事件。
    async fn stop_inner(&self) {
        self.stop_tasks().await;
        self.inner.lock().await.state = EngineState::Stopped;
        self.state_tx.send_replace(EngineState::Stopped);
    }

    /// 启动解码（生产者）与播放（泵）任务。`offset_frames` 为起始采样帧偏移。
    async fn start_playback(
        self: &Arc<Self>,
        offset_frames: usize,
        track_id: String,
    ) -> Result<(), (&'static str, bool)> {
        let sink = self.sink.lock().await.as_ref().cloned();
        let (sample_rate, channels, duration_secs, source) = {
            let inner = self.inner.lock().await;
            (
                inner.sample_rate,
                inner.channels,
                inner.duration_secs,
                inner.source.clone(),
            )
        };

        let (Some(sink), Some(source)) = (sink, source) else {
            return Err(("output_unavailable", true));
        };
        sink.play().map_err(|_| ("device_start_failed", true))?;

        let total_frames = match &source {
            EngineSource::Pcm(samples) => (samples.len() / channels as usize).max(1) as u64,
            EngineSource::File(_) => (duration_secs * sample_rate as f64).round().max(1.0) as u64,
            EngineSource::Sequence(tracks, _) => tracks[0].total_frames,
        };
        let (queue, mut receiver) = PcmQueue::channel(QUEUE_CAPACITY);
        let producer_failed = Arc::new(AtomicBool::new(false));

        // 生产者：文件源在阻塞线程逐包解码；PCM 调用方源按固定块切片。两者都只通过
        // 有界队列向实时侧供数，内存与曲目时长无关。
        let producer_bus = self.bus.clone();
        let producer_inner = self.inner.clone();
        let failed_flag = producer_failed.clone();
        let producer_track_id = track_id.clone();
        let producer = tokio::task::spawn_blocking(move || match source {
            EngineSource::Pcm(samples) => {
                let ch = channels as usize;
                let start = offset_frames.saturating_mul(ch).min(samples.len());
                let mut remaining: &[i16] = &samples[start..];
                let mut frame = offset_frames as u64;
                while !remaining.is_empty() {
                    let take = (CHUNK_FRAMES * ch).min(remaining.len());
                    let chunk = PcmChunk::new(sample_rate, channels, remaining[..take].to_vec())
                        .with_playback(PlaybackChunkMetadata {
                            track_id: producer_track_id.clone(),
                            track_start_frame: frame,
                            track_total_frames: total_frames,
                            duration_secs,
                            boundary_before: None,
                        });
                    if queue.blocking_push(chunk).is_err() {
                        break;
                    }
                    frame = frame.saturating_add((take / ch) as u64);
                    remaining = &remaining[take..];
                }
            }
            EngineSource::File(path) => {
                let decode_result = (|| {
                    let mut decoder = symphonia_decoder::StreamingDecoder::open(&path)?;
                    decoder.skip_to_frame(offset_frames as u64, CHUNK_FRAMES)?;
                    while let Some(decoded) = decoder.next_chunk(CHUNK_FRAMES)? {
                        let chunk =
                            PcmChunk::new(decoded.sample_rate, decoded.channels, decoded.samples)
                                .with_playback(PlaybackChunkMetadata {
                                    track_id: producer_track_id.clone(),
                                    track_start_frame: decoded.start_frame,
                                    track_total_frames: total_frames,
                                    duration_secs,
                                    boundary_before: None,
                                });
                        if queue.blocking_push(chunk).is_err() {
                            break;
                        }
                    }
                    Ok::<(), symphonia_decoder::DecodeError>(())
                })();
                if let Err(error) = decode_result {
                    failed_flag.store(true, Ordering::Release);
                    producer_inner.blocking_lock().state = EngineState::Stopped;
                    producer_bus.publish(Event::PlaybackFailed {
                        track_id: producer_track_id,
                        error: PlaybackFailure {
                            source: path.to_string_lossy().into_owned(),
                            stage: "decode".into(),
                            code: "decode_failed".into(),
                            retryable: false,
                            suggestion: format!(
                                "Verify the file or install a compatible decoder: {error}"
                            ),
                        },
                    });
                }
            }
            EngineSource::Sequence(tracks, transition) => {
                if let Err(error) = produce_playlist(&queue, &tracks, transition, offset_frames) {
                    failed_flag.store(true, Ordering::Release);
                    producer_inner.blocking_lock().state = EngineState::Stopped;
                    producer_bus.publish(Event::PlaybackFailed {
                        track_id: producer_track_id,
                        error: PlaybackFailure {
                            source: "playlist".into(),
                            stage: "transition_decode".into(),
                            code: "transition_decode_failed".into(),
                            retryable: false,
                            suggestion: error,
                        },
                    });
                }
            }
        });

        // 泵：消费队列，以推模型写入输出后端（背压重试），并广播进度。
        let mut state_rx = self.state_tx.subscribe();
        let pump_bus = self.bus.clone();
        let pump_sink = sink.clone();
        let inner = self.inner.clone();
        let pump_failed = producer_failed.clone();
        let pump_state_tx = self.state_tx.clone();
        let pump = tokio::spawn(async move {
            let mut frames_written = offset_frames as u64;
            let mut active_track_id = track_id;
            let mut active_total_frames = total_frames;
            let mut active_sample_rate = sample_rate;
            loop {
                // 等待恢复播放（暂停时挂起；停止时退出）。
                while *state_rx.borrow() != EngineState::Playing {
                    if *state_rx.borrow() == EngineState::Stopped {
                        return;
                    }
                    if state_rx.changed().await.is_err() {
                        return;
                    }
                }

                let chunk = tokio::select! {
                    _ = state_rx.changed() => continue,
                    item = receiver.recv() => match item {
                        Some(c) => c,
                        None => break,
                    },
                };

                if let Some(playback) = &chunk.playback {
                    if let Some(boundary) = &playback.boundary_before {
                        pump_bus.publish(Event::Progress {
                            track_id: boundary.from_track_id.clone(),
                            position: 1.0,
                        });
                        active_track_id = boundary.to_track_id.clone();
                        active_total_frames = playback.track_total_frames.max(1);
                        active_sample_rate = chunk.sample_rate;
                        frames_written = playback.track_start_frame;
                        {
                            let mut current = inner.lock().await;
                            current.track_id = Some(active_track_id.clone());
                            current.sample_rate = chunk.sample_rate;
                            current.channels = chunk.channels;
                            current.duration_secs = playback.duration_secs;
                            current.position_secs =
                                frames_written as f64 / active_sample_rate as f64;
                            if let Some(EngineSource::Sequence(tracks, transition)) =
                                current.source.clone()
                            {
                                if let Some(index) = tracks
                                    .iter()
                                    .position(|track| track.track_id == active_track_id)
                                {
                                    current.source = Some(EngineSource::Sequence(
                                        Arc::from(tracks[index..].to_vec()),
                                        transition,
                                    ));
                                }
                            }
                        }
                        pump_bus.publish(Event::TrackTransitioned {
                            from_track_id: boundary.from_track_id.clone(),
                            to_track_id: boundary.to_track_id.clone(),
                            mode: boundary.mode.clone(),
                            overlap_frames: boundary.overlap_frames,
                            duration_secs: playback.duration_secs,
                        });
                        pump_bus.publish(Event::Played {
                            track_id: active_track_id.clone(),
                        });
                    } else {
                        active_track_id = playback.track_id.clone();
                        active_total_frames = playback.track_total_frames.max(1);
                        active_sample_rate = chunk.sample_rate;
                        frames_written = playback.track_start_frame;
                    }
                }

                // 写完整块（处理背压与部分写入）。
                let mut written_samples = 0usize;
                while written_samples < chunk.samples.len() {
                    if *state_rx.borrow() == EngineState::Stopped {
                        return;
                    }
                    let remaining = &chunk.samples[written_samples..];
                    let remaining_frames = (remaining.len() / channels as usize) as u32;
                    match pump_sink.write(remaining, remaining_frames) {
                        Ok(0) => tokio::time::sleep(BACKPRESSURE_RETRY).await,
                        Ok(n) => {
                            written_samples += (n as usize).saturating_mul(channels as usize);
                        }
                        Err(_) => {
                            // 设备错误：停止。
                            inner.lock().await.state = EngineState::Stopped;
                            pump_state_tx.send_replace(EngineState::Stopped);
                            pump_bus.publish(Event::PlaybackFailed {
                                track_id: active_track_id.clone(),
                                error: PlaybackFailure {
                                    source: "audio_output".into(),
                                    stage: "write".into(),
                                    code: "device_write_failed".into(),
                                    retryable: true,
                                    suggestion: "Reconnect the device or choose another output"
                                        .into(),
                                },
                            });
                            return;
                        }
                    }
                }
                frames_written = frames_written.saturating_add(chunk.frames() as u64);
                inner.lock().await.position_secs =
                    frames_written as f64 / active_sample_rate as f64;

                // 广播进度（按已写帧数比例）。
                let position = (frames_written as f64 / active_total_frames as f64).clamp(0.0, 1.0);
                pump_bus.publish(Event::Progress {
                    track_id: active_track_id.clone(),
                    position,
                });
            }

            if pump_failed.load(Ordering::Acquire) {
                return;
            }
            // 数据播放完毕：停止并广播「完成」事件（区别于手动 stop，供自动切歌）。
            inner.lock().await.state = EngineState::Stopped;
            pump_state_tx.send_replace(EngineState::Stopped);
            pump_bus.publish(Event::Completed {
                track_id: active_track_id,
            });
        });

        *self.producer.lock().await = Some(producer);
        *self.pump.lock().await = Some(pump);
        Ok(())
    }
}

fn produce_playlist(
    queue: &PcmQueue,
    tracks: &[PreparedPlaylistTrack],
    transition: PlaylistTransition,
    first_offset_frames: usize,
) -> Result<(), String> {
    let channels = tracks[0].channels as usize;
    let requested_overlap =
        (transition.crossfade_secs * tracks[0].sample_rate as f64).round() as usize;
    let mut prefetched_frames = 0usize;
    let mut boundary_for_current: Option<TrackBoundary> = None;

    for (index, track) in tracks.iter().enumerate() {
        let start_frame = if index == 0 {
            first_offset_frames
        } else {
            prefetched_frames
        };
        prefetched_frames = 0;
        let mut decoder = symphonia_decoder::StreamingDecoder::open(&track.path)
            .map_err(|error| error.to_string())?;
        decoder
            .skip_to_frame(start_frame as u64, CHUNK_FRAMES)
            .map_err(|error| error.to_string())?;

        let has_next = index + 1 < tracks.len();
        let reserve_frames = if has_next { requested_overlap } else { 0 };
        let reserve_samples = reserve_frames.saturating_mul(channels);
        let mut tail = VecDeque::<i16>::new();
        let mut frame_cursor = start_frame as u64;

        while let Some(decoded) = decoder
            .next_chunk(CHUNK_FRAMES)
            .map_err(|error| error.to_string())?
        {
            tail.extend(decoded.samples);
            while tail.len() > reserve_samples.saturating_add(CHUNK_FRAMES * channels) {
                let samples: Vec<_> = tail.drain(..CHUNK_FRAMES * channels).collect();
                frame_cursor = push_playlist_samples(
                    queue,
                    track,
                    frame_cursor,
                    samples,
                    &mut boundary_for_current,
                )?;
            }
        }

        let Some(next) = tracks.get(index + 1) else {
            let samples: Vec<_> = tail.into_iter().collect();
            push_playlist_samples(
                queue,
                track,
                frame_cursor,
                samples,
                &mut boundary_for_current,
            )?;
            continue;
        };

        if requested_overlap == 0 {
            let samples: Vec<_> = tail.into_iter().collect();
            push_playlist_samples(
                queue,
                track,
                frame_cursor,
                samples,
                &mut boundary_for_current,
            )?;
            boundary_for_current = Some(TrackBoundary {
                from_track_id: track.track_id.clone(),
                to_track_id: next.track_id.clone(),
                mode: "gapless".into(),
                overlap_frames: 0,
            });
            continue;
        }

        let current_frames = tail.len() / channels;
        let head = decode_head(&next.path, requested_overlap)?;
        let head_frames = head.len() / channels;
        let overlap_frames = requested_overlap.min(current_frames).min(head_frames);
        if overlap_frames == 0 {
            let samples: Vec<_> = tail.into_iter().collect();
            push_playlist_samples(
                queue,
                track,
                frame_cursor,
                samples,
                &mut boundary_for_current,
            )?;
            boundary_for_current = Some(TrackBoundary {
                from_track_id: track.track_id.clone(),
                to_track_id: next.track_id.clone(),
                mode: "gapless".into(),
                overlap_frames: 0,
            });
            continue;
        }

        let prefix_samples = tail.len() - overlap_frames * channels;
        if prefix_samples > 0 {
            let samples: Vec<_> = tail.drain(..prefix_samples).collect();
            push_playlist_samples(
                queue,
                track,
                frame_cursor,
                samples,
                &mut boundary_for_current,
            )?;
        }
        let outgoing: Vec<_> = tail.into_iter().collect();
        let incoming = &head[..overlap_frames * channels];
        let mixed = mix_crossfade(
            &outgoing,
            incoming,
            overlap_frames,
            channels,
            transition.curve,
        );
        let mut crossfade_boundary = Some(TrackBoundary {
            from_track_id: track.track_id.clone(),
            to_track_id: next.track_id.clone(),
            mode: match transition.curve {
                CrossfadeCurve::Linear => "crossfade_linear",
                CrossfadeCurve::EqualPower => "crossfade_equal_power",
            }
            .into(),
            overlap_frames: overlap_frames as u32,
        });
        push_playlist_samples(queue, next, 0, mixed, &mut crossfade_boundary)?;
        prefetched_frames = overlap_frames;
        boundary_for_current = None;
    }
    Ok(())
}

fn decode_head(path: &Path, frames: usize) -> Result<Vec<i16>, String> {
    let mut decoder =
        symphonia_decoder::StreamingDecoder::open(path).map_err(|error| error.to_string())?;
    let channels = decoder.channels() as usize;
    let mut samples = Vec::with_capacity(frames.saturating_mul(channels));
    while samples.len() < frames.saturating_mul(channels) {
        let remaining_frames = (frames.saturating_mul(channels) - samples.len()) / channels;
        let Some(chunk) = decoder
            .next_chunk(remaining_frames.clamp(1, CHUNK_FRAMES))
            .map_err(|error| error.to_string())?
        else {
            break;
        };
        samples.extend(chunk.samples);
    }
    samples.truncate(frames.saturating_mul(channels));
    Ok(samples)
}

fn push_playlist_samples(
    queue: &PcmQueue,
    track: &PreparedPlaylistTrack,
    mut frame_cursor: u64,
    samples: Vec<i16>,
    boundary: &mut Option<TrackBoundary>,
) -> Result<u64, String> {
    let channels = track.channels as usize;
    for block in samples.chunks(CHUNK_FRAMES * channels) {
        let frames = block.len() / channels;
        let chunk = PcmChunk::new(track.sample_rate, track.channels, block.to_vec()).with_playback(
            PlaybackChunkMetadata {
                track_id: track.track_id.clone(),
                track_start_frame: frame_cursor,
                track_total_frames: track.total_frames,
                duration_secs: track.duration_secs,
                boundary_before: boundary.take(),
            },
        );
        queue
            .blocking_push(chunk)
            .map_err(|error| error.to_string())?;
        frame_cursor = frame_cursor.saturating_add(frames as u64);
    }
    Ok(frame_cursor)
}

fn mix_crossfade(
    outgoing: &[i16],
    incoming: &[i16],
    frames: usize,
    channels: usize,
    curve: CrossfadeCurve,
) -> Vec<i16> {
    let samples = frames.saturating_mul(channels);
    let mut mixed = Vec::with_capacity(samples);
    for frame in 0..frames {
        let progress = (frame as f64 + 0.5) / frames.max(1) as f64;
        let (out_gain, in_gain) = match curve {
            CrossfadeCurve::Linear => (1.0 - progress, progress),
            CrossfadeCurve::EqualPower => (
                (progress * std::f64::consts::FRAC_PI_2).cos(),
                (progress * std::f64::consts::FRAC_PI_2).sin(),
            ),
        };
        for channel in 0..channels {
            let index = frame * channels + channel;
            let value = outgoing[index] as f64 * out_gain + incoming[index] as f64 * in_gain;
            mixed.push(value.round().clamp(i16::MIN as f64, i16::MAX as f64) as i16);
        }
    }
    mixed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use tokio::sync::broadcast;

    /// 内存输出后端：记录写入帧数，可配置缓冲上限以触发背压。
    struct MockSink {
        buffered: std::sync::Mutex<usize>,
        total_written: AtomicU32,
        /// 缓冲上限（帧）；写入后缓冲增长，直到 flush 前可能触发背压。
        capacity_frames: usize,
    }

    struct FailingStartSink;

    #[derive(Default)]
    struct RecordingSink {
        samples: std::sync::Mutex<Vec<i16>>,
    }

    impl RecordingSink {
        fn samples(&self) -> Vec<i16> {
            self.samples.lock().unwrap().clone()
        }
    }

    impl PcmSink for RecordingSink {
        fn write(&self, samples: &[i16], frames: u32) -> Result<u32, OutputError> {
            self.samples.lock().unwrap().extend_from_slice(samples);
            Ok(frames)
        }

        fn play(&self) -> Result<(), OutputError> {
            Ok(())
        }

        fn pause(&self) -> Result<(), OutputError> {
            Ok(())
        }

        fn stop(&self) -> Result<(), OutputError> {
            Ok(())
        }

        fn flush(&self) -> Result<(), OutputError> {
            Ok(())
        }

        fn buffered_frames(&self) -> u32 {
            0
        }
    }

    /// 写入阶段持续失败的输出（设备热拔插/丢失场景）。
    struct FailingWriteSink;

    impl PcmSink for FailingWriteSink {
        fn write(&self, _samples: &[i16], _frames: u32) -> Result<u32, OutputError> {
            Err(OutputError::Operation(-5))
        }
        fn play(&self) -> Result<(), OutputError> {
            Ok(())
        }
        fn pause(&self) -> Result<(), OutputError> {
            Ok(())
        }
        fn stop(&self) -> Result<(), OutputError> {
            Ok(())
        }
        fn flush(&self) -> Result<(), OutputError> {
            Ok(())
        }
        fn buffered_frames(&self) -> u32 {
            0
        }
    }

    impl PcmSink for FailingStartSink {
        fn write(&self, _samples: &[i16], _frames: u32) -> Result<u32, OutputError> {
            Ok(0)
        }
        fn play(&self) -> Result<(), OutputError> {
            Err(OutputError::Operation(-1))
        }
        fn pause(&self) -> Result<(), OutputError> {
            Ok(())
        }
        fn stop(&self) -> Result<(), OutputError> {
            Ok(())
        }
        fn flush(&self) -> Result<(), OutputError> {
            Ok(())
        }
        fn buffered_frames(&self) -> u32 {
            0
        }
    }

    impl MockSink {
        fn new(capacity_frames: usize) -> Self {
            Self {
                buffered: std::sync::Mutex::new(0),
                total_written: AtomicU32::new(0),
                capacity_frames,
            }
        }
    }

    impl PcmSink for MockSink {
        fn write(&self, _samples: &[i16], frames: u32) -> Result<u32, OutputError> {
            let mut b = self.buffered.lock().unwrap();
            let space = self.capacity_frames.saturating_sub(*b);
            let accept = (frames as usize).min(space) as u32;
            *b += accept as usize;
            self.total_written.fetch_add(accept, Ordering::SeqCst);
            Ok(accept)
        }

        fn play(&self) -> Result<(), OutputError> {
            Ok(())
        }
        fn pause(&self) -> Result<(), OutputError> {
            Ok(())
        }
        fn stop(&self) -> Result<(), OutputError> {
            Ok(())
        }
        fn flush(&self) -> Result<(), OutputError> {
            *self.buffered.lock().unwrap() = 0;
            Ok(())
        }
        fn buffered_frames(&self) -> u32 {
            *self.buffered.lock().unwrap() as u32
        }
    }

    fn engine_with_bus() -> (Arc<PlaybackEngine>, broadcast::Receiver<Event>) {
        let bus = EventBus::new(256);
        let rx = bus.subscribe();
        (PlaybackEngine::new(bus), rx)
    }

    /// 消费广播事件直到收到 [`Event::Completed`]（自然播放完成，或超时）。
    async fn wait_completed(rx: &mut broadcast::Receiver<Event>) {
        loop {
            let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .expect("timed out waiting for Completed")
                .expect("channel closed");
            if matches!(ev, Event::Completed { .. }) {
                break;
            }
        }
    }

    fn write_constant_wav(path: &Path, sample: i16, frames: usize) {
        use std::io::Write;

        const SAMPLE_RATE: u32 = 8_000;
        let data_len = (frames * std::mem::size_of::<i16>()) as u32;
        let mut file = std::fs::File::create(path).unwrap();
        file.write_all(b"RIFF").unwrap();
        file.write_all(&(36 + data_len).to_le_bytes()).unwrap();
        file.write_all(b"WAVE").unwrap();
        file.write_all(b"fmt ").unwrap();
        file.write_all(&16u32.to_le_bytes()).unwrap();
        file.write_all(&1u16.to_le_bytes()).unwrap();
        file.write_all(&1u16.to_le_bytes()).unwrap();
        file.write_all(&SAMPLE_RATE.to_le_bytes()).unwrap();
        file.write_all(&(SAMPLE_RATE * 2).to_le_bytes()).unwrap();
        file.write_all(&2u16.to_le_bytes()).unwrap();
        file.write_all(&16u16.to_le_bytes()).unwrap();
        file.write_all(b"data").unwrap();
        file.write_all(&data_len.to_le_bytes()).unwrap();
        for _ in 0..frames {
            file.write_all(&sample.to_le_bytes()).unwrap();
        }
    }

    async fn run_two_track_sequence(transition: PlaylistTransition) -> (Vec<i16>, Vec<Event>) {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first.wav");
        let second = directory.path().join("second.wav");
        write_constant_wav(&first, 10_000, 800);
        write_constant_wav(&second, -10_000, 800);

        let (engine, mut rx) = engine_with_bus();
        let sink = Arc::new(RecordingSink::default());
        engine.set_output(sink.clone()).await;
        engine
            .play_file_sequence(
                vec![
                    PlaylistTrack {
                        track_id: "first".into(),
                        path: first,
                    },
                    PlaylistTrack {
                        track_id: "second".into(),
                        path: second,
                    },
                ],
                transition,
            )
            .await
            .unwrap();

        let mut events = Vec::new();
        loop {
            let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .expect("timed out waiting for playlist completion")
                .expect("event channel closed");
            let completed = matches!(event, Event::Completed { .. });
            events.push(event);
            if completed {
                break;
            }
        }
        assert_eq!(engine.state().await, EngineState::Stopped);
        (sink.samples(), events)
    }

    #[tokio::test]
    async fn gapless_sequence_is_sample_contiguous_in_one_output_session() {
        let (samples, events) = run_two_track_sequence(PlaylistTransition::GAPLESS).await;

        assert_eq!(
            samples.len(),
            1_600,
            "gapless must add no silence or overlap"
        );
        assert!(samples[..800].iter().all(|sample| *sample == 10_000));
        assert!(samples[800..].iter().all(|sample| *sample == -10_000));
        assert!(events.iter().any(|event| matches!(
            event,
            Event::TrackTransitioned {
                from_track_id,
                to_track_id,
                mode,
                overlap_frames: 0,
                ..
            } if from_track_id == "first" && to_track_id == "second" && mode == "gapless"
        )));
        assert!(matches!(
            events.last(),
            Some(Event::Completed { track_id }) if track_id == "second"
        ));
    }

    #[tokio::test]
    async fn crossfade_overlaps_bounded_tail_and_head_frames() {
        let (samples, events) =
            run_two_track_sequence(PlaylistTransition::crossfade(0.025, CrossfadeCurve::Linear))
                .await;

        assert_eq!(samples.len(), 1_400, "200 frames must be overlapped once");
        assert!(samples[..600].iter().all(|sample| *sample == 10_000));
        assert!(samples[800..].iter().all(|sample| *sample == -10_000));
        let fade = &samples[600..800];
        assert!(fade.windows(2).all(|pair| pair[0] >= pair[1]));
        assert!(fade.first().is_some_and(|sample| *sample > 9_000));
        assert!(fade.last().is_some_and(|sample| *sample < -9_000));
        assert!(events.iter().any(|event| matches!(
            event,
            Event::TrackTransitioned {
                mode,
                overlap_frames: 200,
                ..
            } if mode == "crossfade_linear"
        )));
    }

    #[test]
    fn equal_power_crossfade_clamps_instead_of_overflowing() {
        let mixed = mix_crossfade(
            &[i16::MAX; 8],
            &[i16::MAX; 8],
            8,
            1,
            CrossfadeCurve::EqualPower,
        );
        assert_eq!(mixed.len(), 8);
        assert!(mixed.iter().all(|sample| *sample == i16::MAX));
    }

    #[tokio::test]
    async fn play_pcm_drives_sink_to_completion() {
        let (engine, mut rx) = engine_with_bus();
        let sink = Arc::new(MockSink::new(usize::MAX));
        engine.set_output(sink.clone()).await;

        // 4096 样本，单声道 = 4096 帧。
        let samples: Vec<i16> = (0..4096).map(|i| (i % 100) as i16).collect();
        engine
            .play_pcm("t".into(), 44_100, 1, samples, 4096.0 / 44_100.0)
            .await;

        // 首事件应为 Played。
        assert!(matches!(rx.recv().await.unwrap(), Event::Played { .. }));
        wait_completed(&mut rx).await;
        assert_eq!(engine.state().await, EngineState::Stopped);
        assert_eq!(sink.total_written.load(Ordering::SeqCst), 4096);
    }

    async fn wait_playback_failed(rx: &mut broadcast::Receiver<Event>) -> PlaybackFailure {
        loop {
            let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .expect("timed out waiting for PlaybackFailed")
                .expect("channel closed");
            if let Event::PlaybackFailed { error, .. } = event {
                return error;
            }
        }
    }

    #[tokio::test]
    async fn playback_failure_matrix_distinguishes_device_loss_and_corruption() {
        // 1) 设备热拔插/丢失：写入阶段失败 → device_write_failed，可重试。
        let (engine, mut rx) = engine_with_bus();
        engine.set_output(Arc::new(FailingWriteSink)).await;
        engine
            .play_pcm("device-loss".into(), 44_100, 1, vec![0; 64], 1.0)
            .await;
        let error = wait_playback_failed(&mut rx).await;
        assert_eq!(error.source, "audio_output");
        assert_eq!(error.stage, "write");
        assert_eq!(error.code, "device_write_failed");
        assert!(error.retryable, "device loss must be retryable");
        assert!(!error.suggestion.is_empty());

        // 2) 文件损坏/解码失败：结构化解码错误（元数据阶段或流内解码阶段），
        //    不可自动重试。
        let dir = tempfile::tempdir().unwrap();
        let broken = dir.path().join("broken.wav");
        write_constant_wav(&broken, 0, 1000);
        // 数据区声明 1000 帧，实际截断到一半：头部可解析，解码中途失败。
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&broken)
            .unwrap();
        file.set_len(44 + 1000).unwrap();
        drop(file);
        let (engine, mut rx) = engine_with_bus();
        engine.set_output(Arc::new(RecordingSink::default())).await;
        let started = engine
            .play_file("corrupt".into(), broken.to_string_lossy().into_owned())
            .await;
        if let Err(OutputError::Decode(_)) = started {
            // 元数据阶段即识别损坏。
        } else {
            let error = wait_playback_failed(&mut rx).await;
            assert_eq!(error.stage, "decode");
            assert_eq!(error.code, "decode_failed");
            assert!(!error.retryable, "corrupt files must not auto-retry");
            assert!(error.suggestion.contains("decoder"));
        }

        // 3) 打开阶段失败 → device_start_failed（换输出设备后可重试）。
        let (engine, mut rx) = engine_with_bus();
        engine.set_output(Arc::new(FailingStartSink)).await;
        engine
            .play_pcm("open-fail".into(), 44_100, 1, vec![0; 64], 1.0)
            .await;
        let error = wait_playback_failed(&mut rx).await;
        assert_eq!(error.stage, "open");
        assert_eq!(error.code, "device_start_failed");
        assert!(error.retryable, "choosing another output must be retryable");
        assert!(!error.suggestion.is_empty());
    }

    #[tokio::test]
    async fn failed_output_never_reports_playing() {
        let (engine, mut rx) = engine_with_bus();
        engine.set_output(Arc::new(FailingStartSink)).await;
        engine
            .play_pcm("t".into(), 44_100, 1, vec![0; 64], 1.0)
            .await;

        assert_eq!(engine.state().await, EngineState::Stopped);
        assert!(matches!(
            rx.recv().await.unwrap(),
            Event::PlaybackFailed { error, .. } if error.code == "device_start_failed"
        ));
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn pause_and_resume_transition() {
        let (engine, mut rx) = engine_with_bus();
        let sink = Arc::new(MockSink::new(usize::MAX));
        engine.set_output(sink).await;

        engine
            .play_pcm("t".into(), 44_100, 1, vec![0i16; 1024], 1024.0 / 44_100.0)
            .await;
        let _ = rx.recv().await; // Played

        engine.pause().await;
        assert_eq!(engine.state().await, EngineState::Paused);
        assert!(matches!(rx.recv().await.unwrap(), Event::Paused { .. }));

        engine.resume().await;
        assert_eq!(engine.state().await, EngineState::Playing);
    }

    #[tokio::test]
    async fn stop_publishes_stopped_and_resets() {
        let (engine, mut rx) = engine_with_bus();
        let sink = Arc::new(MockSink::new(usize::MAX));
        engine.set_output(sink).await;

        engine
            .play_pcm("t".into(), 44_100, 1, vec![0i16; 2048], 1.0)
            .await;
        let _ = rx.recv().await; // Played

        engine.stop().await;
        assert_eq!(engine.state().await, EngineState::Stopped);
        assert!(matches!(rx.recv().await.unwrap(), Event::Stopped));
    }

    #[tokio::test]
    async fn switch_output_stops_and_resumes() {
        let (engine, mut rx) = engine_with_bus();
        let sink_a = Arc::new(MockSink::new(usize::MAX));
        let sink_b = Arc::new(MockSink::new(usize::MAX));
        engine.set_output(sink_a.clone()).await;

        engine
            .play_pcm("t".into(), 44_100, 1, vec![0i16; 2048], 1.0)
            .await;
        let _ = rx.recv().await; // Played

        engine.switch_output(sink_b.clone()).await;
        assert_eq!(engine.state().await, EngineState::Playing);
        // 切换后重新广播 Played。
        assert!(matches!(rx.recv().await.unwrap(), Event::Played { .. }));
    }

    #[tokio::test]
    async fn seek_publishes_progress() {
        let (engine, mut rx) = engine_with_bus();
        let sink = Arc::new(MockSink::new(usize::MAX));
        engine.set_output(sink).await;

        engine
            .play_pcm("t".into(), 44_100, 1, vec![0i16; 44_100], 1.0)
            .await;
        let _ = rx.recv().await; // Played

        engine.seek(0.5).await;
        // 位置更新。
        assert!((engine.position_secs().await - 0.5).abs() < 1e-9);
        // seek 首先发布 Progress(position≈0.5)。
        let ev = rx.recv().await.unwrap();
        assert!(matches!(ev, Event::Progress { position, .. } if (position - 0.5).abs() < 0.05));
    }

    #[tokio::test]
    async fn backpressure_retries_until_drained() {
        let (engine, mut rx) = engine_with_bus();
        // 缓冲仅 128 帧，远小于 1024 帧，写入将触发背压。
        let sink = Arc::new(MockSink::new(128));
        engine.set_output(sink.clone()).await;

        engine
            .play_pcm("t".into(), 44_100, 1, vec![0i16; 1024], 1.0)
            .await;
        let _ = rx.recv().await; // Played

        // 模拟设备消费：后台周期性 flush，直到全部 1024 帧写入完成。
        let drainer = tokio::spawn({
            let sink = sink.clone();
            async move {
                while sink.total_written.load(Ordering::SeqCst) < 1024 {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                    sink.flush().unwrap();
                }
            }
        });

        wait_completed(&mut rx).await;
        assert_eq!(engine.state().await, EngineState::Stopped);
        drainer.await.unwrap();
        // 全部 1024 帧最终被写满（背压重试后不丢失）。
        assert_eq!(sink.total_written.load(Ordering::SeqCst), 1024);
    }
}
