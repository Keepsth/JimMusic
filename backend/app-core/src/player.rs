//! 播放引擎（Playback Engine）—— 整合版。
//!
//! [`Player`] 是 Core 的**统一播放入口**：管理播放队列、当前曲目与自动切歌，并把真实
//! 音频播放委托给底层 [`crate::PlaybackEngine`]（解码 → 有界缓冲 → 输出插件）。
//!
//! 播放状态与进度由底层 PlaybackEngine 经 [`EventBus`] 发布的事件驱动（[`Event::Played`]/
//! [`Event::Paused`]/[`Event::Progress`]/[`Event::Completed`]），本层订阅这些事件以同步
//! 队列视图并触发自动切歌，**不再维护模拟进度 ticker**。
//!
//! 无文件路径、文件丢失或解码失败都会发布结构化错误，不再把逻辑状态伪装为真实播放。

use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::engine::{CrossfadeCurve, PcmSink, PlaybackEngine, PlaylistTrack, PlaylistTransition};
use crate::event::{Event, EventBus, PlaybackFailure};
use crate::media::Track;

/// 播放状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackState {
    Stopped,
    Playing,
    Paused,
}

/// 播放引擎内部状态。
#[derive(Debug)]
struct PlayerState {
    queue: Vec<Track>,
    current: usize,
    position_secs: f64,
    duration_secs: f64,
    state: PlaybackState,
    /// 当前曲目是否经底层引擎真实播放（false 为逻辑播放）。
    real_playing: bool,
    sequence_end: Option<usize>,
    transition: PlaylistTransition,
}

/// 统一播放引擎：队列 + 真实音频播放 + 自动切歌，以 [`EventBus`] 解耦订阅者。
pub struct Player {
    bus: EventBus,
    engine: Arc<PlaybackEngine>,
    state: Mutex<PlayerState>,
    /// Serializes user commands with natural-completion auto-advance.
    command: Mutex<()>,
    /// Metadata/decode startup is asynchronous and must be cancellable by stop/queue changes.
    pending_start: Mutex<Option<JoinHandle<()>>>,
    watcher: StdMutex<Option<JoinHandle<()>>>,
}

impl Player {
    /// 创建播放引擎并绑定事件总线（内部持有底层 [`PlaybackEngine`]）。
    pub fn new(bus: EventBus) -> Arc<Self> {
        let engine = PlaybackEngine::new(bus.clone());
        Arc::new(Self {
            bus,
            engine,
            state: Mutex::new(PlayerState {
                queue: Vec::new(),
                current: 0,
                position_secs: 0.0,
                duration_secs: 0.0,
                state: PlaybackState::Stopped,
                real_playing: false,
                sequence_end: None,
                transition: PlaylistTransition::GAPLESS,
            }),
            command: Mutex::new(()),
            pending_start: Mutex::new(None),
            watcher: StdMutex::new(None),
        })
    }

    /// 确保事件订阅任务已启动（懒加载，同步锁；需在 Tokio 运行时上下文中首次调用）。
    fn ensure_watcher(self: &Arc<Self>) {
        let mut guard = self.watcher.lock().expect("watcher lock poisoned");
        if guard.is_some() {
            return;
        }
        let mut rx = self.bus.subscribe();
        let player = self.clone();
        *guard = Some(tokio::spawn(async move {
            while let Ok(ev) = rx.recv().await {
                player.on_event(ev).await;
            }
        }));
    }

    /// 处理来自底层引擎（或本层）的播放事件，同步状态并触发自动切歌。
    async fn on_event(self: &Arc<Self>, ev: Event) {
        match ev {
            Event::Played { .. } => {
                let mut s = self.state.lock().await;
                s.state = PlaybackState::Playing;
                s.duration_secs = s
                    .queue
                    .get(s.current)
                    .and_then(|t| t.duration)
                    .unwrap_or(0.0);
            }
            Event::Paused { .. } => {
                self.state.lock().await.state = PlaybackState::Paused;
            }
            Event::Stopped => {
                let mut s = self.state.lock().await;
                s.state = PlaybackState::Stopped;
                s.position_secs = 0.0;
                s.real_playing = false;
                s.sequence_end = None;
            }
            Event::Progress { position, .. } => {
                let mut s = self.state.lock().await;
                s.position_secs = position * s.duration_secs;
            }
            Event::Completed { track_id } => {
                // 与 stop/切歌串行，并忽略已经被新命令取代的旧曲目完成事件。
                let _command = self.command.lock().await;
                let should_advance = {
                    let mut s = self.state.lock().await;
                    let current_matches = s
                        .queue
                        .get(s.current)
                        .is_some_and(|track| track_id_of(track) == track_id);
                    if s.real_playing
                        && current_matches
                        && s.sequence_end.is_some_and(|end| s.current < end)
                    {
                        false
                    } else {
                        let should_advance = s.real_playing && current_matches;
                        if should_advance {
                            s.state = PlaybackState::Stopped;
                            s.position_secs = 0.0;
                            s.real_playing = false;
                            s.sequence_end = None;
                        }
                        should_advance
                    }
                };
                if should_advance {
                    let has_more = {
                        let s = self.state.lock().await;
                        s.current + 1 < s.queue.len()
                    };
                    if has_more {
                        self.advance_next_locked().await;
                    }
                }
            }
            Event::TrackTransitioned {
                to_track_id,
                duration_secs,
                ..
            } => {
                let mut s = self.state.lock().await;
                if let Some(index) = s
                    .queue
                    .iter()
                    .position(|track| track_id_of(track) == to_track_id)
                {
                    s.current = index;
                    s.position_secs = 0.0;
                    s.duration_secs = duration_secs;
                    s.state = PlaybackState::Playing;
                    s.real_playing = true;
                }
            }
            Event::PlaybackFailed { .. } => {
                let mut s = self.state.lock().await;
                s.state = PlaybackState::Stopped;
                s.real_playing = false;
                s.sequence_end = None;
            }
            _ => {}
        }
    }

    /// 循环切到下一首并播放。
    async fn advance_next_locked(self: &Arc<Self>) {
        let next = {
            let s = self.state.lock().await;
            if s.queue.is_empty() {
                return;
            }
            (s.current + 1) % s.queue.len()
        };
        self.play_track_locked(next).await;
    }

    /// 设置播放队列（复位到第 0 首）。
    pub async fn set_queue(&self, tracks: Vec<Track>) {
        let _command = self.command.lock().await;
        let pending = self.pending_start.lock().await.take().is_some_and(|task| {
            task.abort();
            true
        });
        let was_real = self.state.lock().await.real_playing;
        if pending || was_real {
            self.engine.stop().await;
        }
        let mut s = self.state.lock().await;
        s.queue = tracks;
        s.current = 0;
        s.position_secs = 0.0;
        s.duration_secs = s.queue.first().and_then(|t| t.duration).unwrap_or(0.0);
        s.state = PlaybackState::Stopped;
        s.real_playing = false;
        s.sequence_end = None;
    }

    /// 当前播放状态。
    pub async fn state(&self) -> PlaybackState {
        self.state.lock().await.state
    }

    /// 当前曲目（若有）。
    pub async fn current_track(&self) -> Option<Track> {
        let s = self.state.lock().await;
        s.queue.get(s.current).cloned()
    }

    /// 当前曲目索引（队列为空时返回 0）。
    pub async fn current_index(&self) -> usize {
        self.state.lock().await.current
    }

    /// 队列长度。
    pub async fn queue_len(&self) -> usize {
        self.state.lock().await.queue.len()
    }

    /// 当前播放位置（秒）。
    pub async fn position_secs(&self) -> f64 {
        self.state.lock().await.position_secs
    }

    /// 当前曲目时长（秒）。
    pub async fn duration_secs(&self) -> f64 {
        self.state.lock().await.duration_secs
    }

    /// 播放（或恢复）当前曲目。
    pub async fn play(self: &Arc<Self>) {
        let current = self.state.lock().await.current;
        self.play_track(current).await;
    }

    /// 播放队列中指定索引的曲目。
    pub async fn play_track(self: &Arc<Self>, index: usize) {
        self.ensure_watcher();
        let _command = self.command.lock().await;
        self.play_track_locked(index).await;
    }

    async fn play_track_locked(self: &Arc<Self>, index: usize) {
        if let Some(task) = self.pending_start.lock().await.take() {
            task.abort();
        }

        let (track, sequence, transition) = {
            let mut s = self.state.lock().await;
            if index >= s.queue.len() {
                return;
            }
            s.current = index;
            s.position_secs = 0.0;
            s.duration_secs = s.queue.get(index).and_then(|t| t.duration).unwrap_or(0.0);
            s.state = PlaybackState::Playing;
            let sequence: Vec<_> = s
                .queue
                .iter()
                .skip(index)
                .take_while(|track| {
                    !track.path.is_empty() && std::path::Path::new(&track.path).exists()
                })
                .map(|track| PlaylistTrack {
                    track_id: track_id_of(track),
                    path: track.path.clone().into(),
                })
                .collect();
            s.sequence_end = (sequence.len() > 1).then(|| index + sequence.len() - 1);
            (s.queue.get(index).cloned(), sequence, s.transition)
        };
        let Some(t) = track else { return };

        // 有真实文件 → 经底层引擎真实播放（引擎发布 Played/Progress/Completed）。
        if !t.path.is_empty() && std::path::Path::new(&t.path).exists() {
            {
                self.state.lock().await.real_playing = true;
            }
            let engine = self.engine.clone();
            let id = track_id_of(&t);
            let path = t.path.clone();
            let bus = self.bus.clone();
            let task = tokio::spawn(async move {
                let result = if sequence.len() > 1 {
                    engine.play_file_sequence(sequence, transition).await
                } else {
                    engine.play_file(id.clone(), path.clone()).await
                };
                if let Err(error) = result {
                    tracing::warn!("playback start failed: {error}");
                    bus.publish(Event::PlaybackFailed {
                        track_id: id,
                        error: PlaybackFailure {
                            source: path,
                            stage: "playlist_prepare".into(),
                            code: "playlist_prepare_failed".into(),
                            retryable: false,
                            suggestion: error.to_string(),
                        },
                    });
                }
            });
            *self.pending_start.lock().await = Some(task);
        } else {
            let track_id = track_id_of(&t);
            {
                let mut s = self.state.lock().await;
                s.state = PlaybackState::Stopped;
                s.real_playing = false;
            }
            self.bus.publish(Event::PlaybackFailed {
                track_id,
                error: PlaybackFailure {
                    source: t.path,
                    stage: "source".into(),
                    code: "source_unavailable".into(),
                    retryable: true,
                    suggestion: "Locate the missing file or choose another rendition".into(),
                },
            });
        }
    }

    /// Configures the double-timeline playlist transition. Zero seconds means
    /// sample-contiguous gapless playback; positive values use a bounded
    /// crossfade window and never ask the output backend to infer boundaries.
    pub async fn set_crossfade(&self, seconds: f64, equal_power: bool) {
        let mut state = self.state.lock().await;
        state.transition = PlaylistTransition::crossfade(
            seconds,
            if equal_power {
                CrossfadeCurve::EqualPower
            } else {
                CrossfadeCurve::Linear
            },
        );
    }

    pub async fn crossfade_secs(&self) -> f64 {
        self.state.lock().await.transition.crossfade_secs
    }

    /// 暂停播放。
    pub async fn pause(&self) {
        let _command = self.command.lock().await;
        let (was_playing, track_id, real) = {
            let mut s = self.state.lock().await;
            let was = s.state == PlaybackState::Playing;
            if was {
                s.state = PlaybackState::Paused;
            }
            (was, s.queue.get(s.current).map(track_id_of), s.real_playing)
        };
        if real {
            self.engine.pause().await; // 真实播放：引擎发布 Paused
        } else if was_playing {
            if let Some(id) = track_id {
                self.bus.publish(Event::PlaybackFailed {
                    track_id: id,
                    error: PlaybackFailure {
                        source: "playback".into(),
                        stage: "pause".into(),
                        code: "no_active_audio_session".into(),
                        retryable: true,
                        suggestion: "Start a playable track before pausing".into(),
                    },
                });
            }
        }
    }

    /// 恢复播放。
    pub async fn resume(&self) {
        let _command = self.command.lock().await;
        let (was_paused, track_id, real) = {
            let mut s = self.state.lock().await;
            let was = s.state == PlaybackState::Paused;
            if was {
                s.state = PlaybackState::Playing;
            }
            (was, s.queue.get(s.current).map(track_id_of), s.real_playing)
        };
        if real {
            self.engine.resume().await; // 真实播放：引擎发布 Played
        } else if was_paused {
            if let Some(id) = track_id {
                self.bus.publish(Event::PlaybackFailed {
                    track_id: id,
                    error: PlaybackFailure {
                        source: "playback".into(),
                        stage: "resume".into(),
                        code: "no_active_audio_session".into(),
                        retryable: true,
                        suggestion: "Reload the track and retry".into(),
                    },
                });
            }
        }
    }

    /// 停止播放并复位到起点。
    pub async fn stop(&self) {
        let _command = self.command.lock().await;
        if let Some(task) = self.pending_start.lock().await.take() {
            task.abort();
        }
        {
            let mut s = self.state.lock().await;
            let was_real = s.real_playing;
            s.state = PlaybackState::Stopped;
            s.position_secs = 0.0;
            s.real_playing = false;
            if !was_real {
                drop(s);
                self.bus.publish(Event::Stopped);
                return;
            }
        }
        self.engine.stop().await; // 真实播放：引擎发布 Stopped
    }

    /// 跳转到指定位置（秒）。逻辑层发布进度，真实层同时 seek。
    pub async fn seek(&self, position_secs: f64) {
        let _command = self.command.lock().await;
        let (duration, track_id, real) = {
            let mut s = self.state.lock().await;
            s.position_secs = position_secs.clamp(0.0, s.duration_secs.max(0.0));
            (
                s.duration_secs,
                s.queue.get(s.current).map(track_id_of),
                s.real_playing,
            )
        };
        // 逻辑层发布进度（保证 UI 立即响应；真实播放时引擎也会 seek）。
        if let Some(id) = track_id {
            let ratio = if duration > 0.0 {
                (position_secs / duration).clamp(0.0, 1.0)
            } else {
                0.0
            };
            self.bus.publish(Event::Progress {
                track_id: id,
                position: ratio,
            });
        }
        if real {
            self.engine.seek(position_secs).await;
        }
    }

    /// 下一首（循环）。
    pub async fn next(self: &Arc<Self>) {
        let _command = self.command.lock().await;
        self.advance_next_locked().await;
    }

    /// 上一首（循环）。
    pub async fn previous(self: &Arc<Self>) {
        let _command = self.command.lock().await;
        let prev = {
            let s = self.state.lock().await;
            if s.queue.is_empty() {
                return;
            }
            (s.current + s.queue.len() - 1) % s.queue.len()
        };
        self.play_track_locked(prev).await;
    }

    /// 设置音频输出后端（委托底层引擎）。
    pub async fn set_output(&self, sink: Arc<dyn PcmSink>) {
        self.engine.set_output(sink).await;
    }
}

fn track_id_of(t: &Track) -> String {
    if t.path.is_empty() {
        t.title.clone()
    } else {
        t.path.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_track(title: &str) -> Track {
        Track {
            path: format!("/music/{title}.mp3"),
            title: title.to_string(),
            artist: Some("Artist".into()),
            album: None,
            duration: Some(30.0),
            sample_rate: Some(44100),
            channels: Some(2),
        }
    }

    #[tokio::test]
    async fn missing_source_publishes_structured_failure() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe();
        let player = Player::new(bus);
        player.set_queue(vec![make_track("a")]).await;

        player.play().await;
        let ev = rx.recv().await.unwrap();
        assert!(matches!(
            ev,
            Event::PlaybackFailed {
                error: PlaybackFailure { code, .. },
                ..
            } if code == "source_unavailable"
        ));
        assert_eq!(player.state().await, PlaybackState::Stopped);
    }

    #[tokio::test]
    async fn stop_publishes_stopped_event() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe();
        let player = Player::new(bus);
        player.set_queue(vec![make_track("a")]).await;

        player.play().await;
        let _ = rx.recv().await; // PlaybackFailed
        player.stop().await;
        let ev = rx.recv().await.unwrap();
        assert_eq!(ev, Event::Stopped);
        assert_eq!(player.state().await, PlaybackState::Stopped);
    }

    #[tokio::test]
    async fn next_and_previous_cycle() {
        let bus = EventBus::new(16);
        let player = Player::new(bus);
        player
            .set_queue(vec![make_track("a"), make_track("b")])
            .await;

        player.play_track(0).await;
        assert_eq!(player.current_track().await.unwrap().title, "a");

        player.next().await;
        assert_eq!(player.current_track().await.unwrap().title, "b");

        player.previous().await;
        assert_eq!(player.current_track().await.unwrap().title, "a");
    }

    #[tokio::test]
    async fn seek_clamps_and_publishes_progress() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe();
        let player = Player::new(bus);
        player.set_queue(vec![make_track("a")]).await;

        player.seek(15.0).await;
        let ev = rx.recv().await.unwrap();
        if let Event::Progress { position, .. } = ev {
            assert!((position - 0.5).abs() < 1e-9);
        } else {
            panic!("expected Progress event");
        }
        assert!((player.position_secs().await - 15.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn resume_without_audio_session_reports_failure() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe();
        let player = Player::new(bus);
        player.set_queue(vec![make_track("a")]).await;

        player.state.lock().await.state = PlaybackState::Paused;
        player.resume().await;
        let ev = rx.recv().await.unwrap();
        assert!(matches!(
            ev,
            Event::PlaybackFailed {
                error: PlaybackFailure { code, .. },
                ..
            } if code == "no_active_audio_session"
        ));
    }
}
