//! 核心事件总线 → UI 桥插件的事件转发。
//!
//! 对应需求 3.3「事件总线：播放/暂停/进度回调」：
//! [`event_to_op`] 将核心 [`Event`] 映射为 UI 桥插件的操作名与二进制入参，
//! [`forward_event_to_ui`] 通过 [`LoadedPlugin::invoke`] 将事件推送给 UI 桥插件。

use crate::event::Event;
use crate::plugin::{LoadedPlugin, PluginError};

/// 播放状态编码（与 ui-bridge 的 `on_state` 约定一致）：0=停止 / 1=播放 / 2=暂停。
const STATE_STOPPED: u8 = 0;
const STATE_PLAYING: u8 = 1;
const STATE_PAUSED: u8 = 2;

/// 将核心播放事件映射为 UI 桥插件调用 `(op, input)`。
///
/// - [`Event::Played`] → `("on_state", [1])`
/// - [`Event::Paused`] → `("on_state", [2])`
/// - [`Event::Stopped`] → `("on_state", [0])`
/// - [`Event::Progress`] → `("on_progress", <8 字节小端 f64>)`
/// - 其它事件（插件加载/卸载）→ `None`（不转发）。
pub fn event_to_op(event: &Event) -> Option<(&'static str, Vec<u8>)> {
    match event {
        Event::Played { .. } => Some(("on_state", vec![STATE_PLAYING])),
        Event::Paused { .. } => Some(("on_state", vec![STATE_PAUSED])),
        Event::Stopped => Some(("on_state", vec![STATE_STOPPED])),
        Event::Completed { .. } => Some(("on_state", vec![STATE_STOPPED])),
        Event::Progress { position, .. } => Some(("on_progress", position.to_le_bytes().to_vec())),
        Event::PlaybackFailed { error, .. } => {
            let payload = serde_json::json!({
                "source": error.source,
                "stage": error.stage,
                "code": error.code,
                "retryable": error.retryable,
                "suggestion": error.suggestion,
            });
            Some(("on_error", serde_json::to_vec(&payload).unwrap_or_default()))
        }
        _ => None,
    }
}

/// 将单个核心事件转发到 UI 桥插件。
///
/// 对无需转发的插件事件（如插件加载/卸载）静默忽略。
pub fn forward_event_to_ui(plugin: &LoadedPlugin, event: &Event) -> Result<(), PluginError> {
    if let Some((op, input)) = event_to_op(event) {
        plugin.invoke(op, &input)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_playback_events_to_ops() {
        assert_eq!(
            event_to_op(&Event::Played {
                track_id: "a".into()
            }),
            Some(("on_state", vec![1]))
        );
        assert_eq!(
            event_to_op(&Event::Paused {
                track_id: "a".into()
            }),
            Some(("on_state", vec![2]))
        );
        assert_eq!(event_to_op(&Event::Stopped), Some(("on_state", vec![0])));
    }

    #[test]
    fn maps_progress_to_little_endian_f64() {
        let ev = Event::Progress {
            track_id: "a".into(),
            position: 0.5,
        };
        let (op, input) = event_to_op(&ev).unwrap();
        assert_eq!(op, "on_progress");
        assert_eq!(input, 0.5f64.to_le_bytes().to_vec());
    }

    #[test]
    fn ignores_non_playback_events() {
        assert_eq!(
            event_to_op(&Event::PluginLoaded {
                name: "x".into(),
                version: "1".into()
            }),
            None
        );
    }
}
