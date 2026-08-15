//! 基于 Tokio 广播通道的异步消息总线。
//!
//! 核心与插件之间通过事件解耦：播放/暂停/进度等状态变化以 [`Event`] 形式发布，
//! 任意数量的订阅者并发接收。发送不阻塞（即使无订阅者）。

use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;

/// 可跨 UI/FFI 边界呈现的播放错误上下文。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlaybackFailure {
    pub source: String,
    pub stage: String,
    pub code: String,
    pub retryable: bool,
    pub suggestion: String,
}

/// 总线事件载荷。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    /// 开始播放某曲目。
    Played {
        track_id: String,
    },
    /// 暂停。
    Paused {
        track_id: String,
    },
    /// 停止。
    Stopped,
    /// 自然播放完成（区别于手动 [`Event::Stopped`]，供自动切歌）。
    Completed {
        track_id: String,
    },
    /// The first PCM block of a new playlist timeline reached the output.
    TrackTransitioned {
        from_track_id: String,
        to_track_id: String,
        mode: String,
        overlap_frames: u32,
        duration_secs: f64,
    },
    /// 播放进度变化（0.0 ~ 1.0）。
    Progress {
        track_id: String,
        position: f64,
    },
    /// 播放链路失败；不得映射为 Played 或静默模拟成功。
    PlaybackFailed {
        track_id: String,
        error: PlaybackFailure,
    },
    /// 插件加载成功。
    PluginLoaded {
        name: String,
        version: String,
    },
    /// 插件卸载。
    PluginUnloaded {
        name: String,
    },
    /// 持久长任务状态变化。
    TransferChanged {
        task_id: String,
        state: String,
        bytes_completed: u64,
    },
    NodeChanged {
        state: String,
    },
    CommunitySourceChanged {
        source_id: String,
        state: String,
    },
    PolicyChanged {
        target: String,
        decision: String,
    },
    PluginChanged {
        plugin_id: String,
        state: String,
        version: Option<String>,
    },
    AudioGraphChanged {
        graph_id: String,
        generation: u64,
    },
    PublicationChanged {
        publisher_id: String,
        event_cid: String,
        sequence: u64,
    },
}

/// 可排序、可检测缺口的版本化事件信封。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VersionedEvent {
    pub schema_version: u16,
    pub sequence: u64,
    pub timestamp_ms: u64,
    pub event_type: &'static str,
    pub entity_id: Option<String>,
    pub event: Event,
}

/// 异步事件总线。
#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Event>,
    versioned_tx: broadcast::Sender<VersionedEvent>,
    sequence: Arc<AtomicU64>,
}

impl EventBus {
    /// 以指定容量创建总线。
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        let (versioned_tx, _) = broadcast::channel(capacity);
        Self {
            tx,
            versioned_tx,
            sequence: Arc::new(AtomicU64::new(0)),
        }
    }

    /// 订阅总线，返回可迭代的接收器。
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }

    pub fn subscribe_versioned(&self) -> broadcast::Receiver<VersionedEvent> {
        self.versioned_tx.subscribe()
    }

    pub fn latest_sequence(&self) -> u64 {
        self.sequence.load(Ordering::Acquire)
    }

    /// 发布事件。无订阅者时忽略（不阻塞，不返回错误）。
    pub fn publish(&self, event: Event) {
        let sequence = self.sequence.fetch_add(1, Ordering::AcqRel) + 1;
        let versioned = VersionedEvent {
            schema_version: 1,
            sequence,
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .min(u64::MAX as u128) as u64,
            event_type: event_type(&event),
            entity_id: entity_id(&event),
            event: event.clone(),
        };
        let _ = self.tx.send(event);
        let _ = self.versioned_tx.send(versioned);
    }
}

fn event_type(event: &Event) -> &'static str {
    match event {
        Event::Played { .. } | Event::Paused { .. } | Event::Stopped => "playback.state_changed",
        Event::Completed { .. } => "playback.completed",
        Event::TrackTransitioned { .. } => "playback.transitioned",
        Event::Progress { .. } => "playback.position",
        Event::PlaybackFailed { .. } => "playback.error",
        Event::PluginLoaded { .. } => "plugin.state_changed",
        Event::PluginUnloaded { .. } => "plugin.state_changed",
        Event::TransferChanged { .. } => "transfer.state_changed",
        Event::NodeChanged { .. } => "node.status_changed",
        Event::CommunitySourceChanged { .. } => "community_source.updated",
        Event::PolicyChanged { .. } => "policy.decision_changed",
        Event::PluginChanged { .. } => "plugin.state_changed",
        Event::AudioGraphChanged { .. } => "audio.graph_changed",
        Event::PublicationChanged { .. } => "publication.changed",
    }
}

fn entity_id(event: &Event) -> Option<String> {
    match event {
        Event::Played { track_id }
        | Event::Paused { track_id }
        | Event::Completed { track_id }
        | Event::Progress { track_id, .. }
        | Event::PlaybackFailed { track_id, .. } => Some(track_id.clone()),
        Event::TrackTransitioned { to_track_id, .. } => Some(to_track_id.clone()),
        Event::PluginLoaded { name, .. } | Event::PluginUnloaded { name } => Some(name.clone()),
        Event::TransferChanged { task_id, .. } => Some(task_id.clone()),
        Event::NodeChanged { .. } => None,
        Event::CommunitySourceChanged { source_id, .. } => Some(source_id.clone()),
        Event::PolicyChanged { target, .. } => Some(target.clone()),
        Event::PluginChanged { plugin_id, .. } => Some(plugin_id.clone()),
        Event::AudioGraphChanged { graph_id, .. } => Some(graph_id.clone()),
        Event::PublicationChanged { publisher_id, .. } => Some(publisher_id.clone()),
        Event::Stopped => None,
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(256)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn publish_and_receive() {
        let bus = EventBus::new(8);
        let mut rx = bus.subscribe();
        bus.publish(Event::PluginLoaded {
            name: "x".into(),
            version: "1".into(),
        });

        let event = rx.recv().await.expect("should receive");
        assert!(matches!(event, Event::PluginLoaded { .. }));
    }

    #[tokio::test]
    async fn multiple_subscribers_receive_all() {
        let bus = EventBus::new(8);
        let mut a = bus.subscribe();
        let mut b = bus.subscribe();
        bus.publish(Event::Stopped);

        assert_eq!(a.recv().await.unwrap(), Event::Stopped);
        assert_eq!(b.recv().await.unwrap(), Event::Stopped);
    }

    #[tokio::test]
    async fn versioned_events_are_monotonic_and_named() {
        let bus = EventBus::new(8);
        let mut receiver = bus.subscribe_versioned();
        bus.publish(Event::Played {
            track_id: "track".into(),
        });
        bus.publish(Event::Progress {
            track_id: "track".into(),
            position: 0.5,
        });
        let first = receiver.recv().await.unwrap();
        let second = receiver.recv().await.unwrap();
        assert_eq!(first.sequence, 1);
        assert_eq!(second.sequence, 2);
        assert_eq!(first.event_type, "playback.state_changed");
        assert_eq!(second.event_type, "playback.position");
        assert_eq!(bus.latest_sequence(), 2);
    }
}
