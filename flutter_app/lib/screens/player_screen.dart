import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../models/lyrics.dart';
import '../models/playback_mode.dart';
import '../providers/control_plane_provider.dart';
import '../providers/music_player_provider.dart';
import '../widgets/geek_cover.dart';
import '../widgets/lyrics_view.dart';

/// 传输任务状态摘要（边下边播的缓存/网络状态，UI-001）。
({int bytesCompleted, int? bytesTotal, String state, String providers})
transferTaskSummary(Map<String, dynamic>? task) {
  final completed = (task?['bytes_completed'] as num?)?.toInt() ?? 0;
  final total = (task?['bytes_total'] as num?)?.toInt();
  return (
    bytesCompleted: completed,
    bytesTotal: total,
    state: '${task?['state'] ?? 'unknown'}',
    providers: (task?['providers'] as List<dynamic>? ?? const [])
        .join(', '),
  );
}

String _formatBytes(int bytes) {
  if (bytes >= 1024 * 1024) {
    return '${(bytes / (1024 * 1024)).toStringAsFixed(1)} MiB';
  }
  if (bytes >= 1024) return '${(bytes / 1024).toStringAsFixed(1)} KiB';
  return '$bytes B';
}

/// 播放页：专辑封面、播放控制、进度条、收藏。
class PlayerScreen extends StatelessWidget {
  const PlayerScreen({super.key});

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Scaffold(
      appBar: AppBar(
        leading: IconButton(
          onPressed: () => Navigator.pop(context),
          icon: Icon(
            Icons.keyboard_arrow_down,
            color: scheme.onSurface,
            size: 28,
          ),
        ),
        title: Consumer<MusicPlayerProvider>(
          builder: (context, player, child) {
            return Column(
              children: [
                Text(
                  player.currentMusic?.album ?? '',
                  style: TextStyle(
                    color: scheme.onSurface,
                    fontSize: 12,
                    fontWeight: FontWeight.w500,
                  ),
                ),
                Text(
                  '正在播放',
                  style: TextStyle(
                    color: scheme.onSurfaceVariant,
                    fontSize: 10,
                  ),
                ),
              ],
            );
          },
        ),
        centerTitle: true,
        actions: const [SizedBox(width: 48)],
      ),
      body: Consumer<MusicPlayerProvider>(
        builder: (context, player, child) {
          final music = player.currentMusic;
          if (music == null) {
            return Center(
              child: Text(
                '没有正在播放的音乐',
                style: TextStyle(color: scheme.onSurfaceVariant),
              ),
            );
          }

          final favorite = player.isFavorite(music);

          return SingleChildScrollView(
            padding: const EdgeInsets.all(24.0),
            child: Column(
              children: [
                const SizedBox(height: 20),

                // 专辑封面（极客风，零网络/零解码）
                GeekCover(
                  seed: music.id,
                  label: music.title,
                  size: 180,
                  borderRadius: BorderRadius.circular(4),
                ),

                const SizedBox(height: 32),

                // 歌曲信息 + 收藏
                Row(
                  children: [
                    Expanded(
                      child: Column(
                        children: [
                          Text(
                            music.title,
                            style: TextStyle(
                              color: scheme.onSurface,
                              fontSize: 20,
                              fontWeight: FontWeight.bold,
                            ),
                            textAlign: TextAlign.center,
                            maxLines: 2,
                            overflow: TextOverflow.ellipsis,
                          ),
                          const SizedBox(height: 8),
                          Text(
                            music.artist,
                            style: TextStyle(
                              color: scheme.onSurfaceVariant,
                              fontSize: 16,
                            ),
                            textAlign: TextAlign.center,
                          ),
                        ],
                      ),
                    ),
                    IconButton(
                      onPressed: () => player.toggleFavorite(music),
                      icon: Icon(
                        favorite ? Icons.favorite : Icons.favorite_border,
                        color: favorite
                            ? scheme.primary
                            : scheme.onSurfaceVariant,
                        size: 28,
                      ),
                    ),
                  ],
                ),

                const SizedBox(height: 16),

                // 来源 / 缓冲 / 网络状态（UI-001）：来自真实服务事件，不使用模拟数据。
                _PlaybackStatusLine(
                  player: player,
                  control: context.watch<ControlPlaneProvider>(),
                ),

                const SizedBox(height: 24),

                // 同步歌词
                SizedBox(
                  height: 120,
                  child: LyricsView(
                    lyrics: Lyrics.parseLrc(music.lyrics ?? ''),
                    position: Duration(seconds: player.currentPosition.toInt()),
                  ),
                ),

                const SizedBox(height: 24),

                // 进度条
                Column(
                  children: [
                    Slider(
                      value: player.currentPosition,
                      max: player.duration > 0 ? player.duration : 1,
                      onChanged: player.seekTo,
                      activeColor: scheme.primary,
                      inactiveColor: scheme.onSurfaceVariant.withValues(
                        alpha: 0.3,
                      ),
                      thumbColor: scheme.primary,
                    ),
                    Padding(
                      padding: const EdgeInsets.symmetric(horizontal: 16),
                      child: Row(
                        mainAxisAlignment: MainAxisAlignment.spaceBetween,
                        children: [
                          Text(
                            _formatDuration(player.currentPosition),
                            style: TextStyle(
                              color: scheme.onSurfaceVariant,
                              fontSize: 12,
                            ),
                          ),
                          Text(
                            music.duration,
                            style: TextStyle(
                              color: scheme.onSurfaceVariant,
                              fontSize: 12,
                            ),
                          ),
                        ],
                      ),
                    ),
                  ],
                ),

                const SizedBox(height: 24),

                // 播放控制按钮
                Row(
                  mainAxisAlignment: MainAxisAlignment.spaceEvenly,
                  children: [
                    IconButton(
                      onPressed: player.previous,
                      icon: Icon(
                        Icons.skip_previous,
                        color: scheme.onSurface,
                        size: 36,
                      ),
                    ),
                    Container(
                      width: 64,
                      height: 64,
                      decoration: BoxDecoration(
                        color: scheme.primary,
                        shape: BoxShape.circle,
                      ),
                      child: IconButton(
                        onPressed: player.isBuffering
                            ? null
                            : player.togglePlayPause,
                        icon: player.isBuffering
                            ? CircularProgressIndicator(color: scheme.onPrimary)
                            : Icon(
                                player.isPlaying
                                    ? Icons.pause
                                    : Icons.play_arrow,
                                color: scheme.onPrimary,
                                size: 32,
                              ),
                      ),
                    ),
                    IconButton(
                      onPressed: player.next,
                      icon: Icon(
                        Icons.skip_next,
                        color: scheme.onSurface,
                        size: 36,
                      ),
                    ),
                  ],
                ),

                const SizedBox(height: 12),
                // 播放模式切换（PLR-102）：顺序 / 列表循环 / 单曲循环 / 随机。
                Row(
                  mainAxisAlignment: MainAxisAlignment.center,
                  children: [
                    IconButton(
                      tooltip: '播放模式：${_modeLabel(player.playbackMode)}',
                      onPressed: player.cyclePlaybackMode,
                      icon: Icon(
                        _modeIcon(player.playbackMode),
                        color: scheme.onSurfaceVariant,
                        size: 20,
                      ),
                    ),
                    Text(
                      _modeLabel(player.playbackMode),
                      style: TextStyle(
                        color: scheme.onSurfaceVariant,
                        fontSize: 11,
                      ),
                    ),
                  ],
                ),
                const SizedBox(height: 8),
                Row(
                  children: [
                    IconButton(
                      tooltip: player.muted ? '取消静音' : '静音',
                      onPressed: () => player.setMuted(!player.muted),
                      icon: Icon(
                        player.muted ? Icons.volume_off : Icons.volume_up,
                      ),
                    ),
                    Expanded(
                      child: Slider(
                        value: player.volume,
                        onChanged: player.setVolume,
                      ),
                    ),
                  ],
                ),

                const SizedBox(height: 8),
              ],
            ),
          );
        },
      ),
    );
  }

  String _formatDuration(double seconds) {
    final duration = Duration(seconds: seconds.toInt());
    final minutes = duration.inMinutes;
    final remainingSeconds = duration.inSeconds % 60;
    return '${minutes.toString().padLeft(1, '0')}:${remainingSeconds.toString().padLeft(2, '0')}';
  }
}

/// 来源标签 + 缓冲位置 + 边下边播传输进度（UI-001）。
class _PlaybackStatusLine extends StatelessWidget {
  const _PlaybackStatusLine({required this.player, required this.control});

  final MusicPlayerProvider player;
  final ControlPlaneProvider control;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final buffered = player.bufferedPosition;
    final duration = player.duration;
    final showBuffer = !player.supportsNativeTransitions &&
        duration > 0 &&
        buffered > 0 &&
        buffered < duration;

    final taskId = player.transferTaskId;
    Map<String, dynamic>? task;
    if (taskId != null) {
      for (final candidate in control.transfers) {
        if (candidate['task_id'] == taskId) {
          task = candidate;
          break;
        }
      }
    }
    final summary = transferTaskSummary(task);

    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            Icon(Icons.album, size: 14, color: scheme.onSurfaceVariant),
            const SizedBox(width: 4),
            Text(
              '来源：${player.sourceLabel}',
              style: TextStyle(
                color: scheme.onSurfaceVariant,
                fontSize: 12,
              ),
            ),
            const Spacer(),
            if (player.isBuffering)
              SizedBox(
                width: 12,
                height: 12,
                child: CircularProgressIndicator(
                  strokeWidth: 2,
                  color: scheme.primary,
                ),
              ),
          ],
        ),
        if (showBuffer)
          Padding(
            padding: const EdgeInsets.only(top: 6),
            child: ClipRRect(
              borderRadius: BorderRadius.circular(2),
              child: LinearProgressIndicator(
                value: (buffered / duration).clamp(0.0, 1.0),
                minHeight: 3,
                backgroundColor: scheme.onSurfaceVariant.withValues(alpha: 0.2),
              ),
            ),
          ),
        if (taskId != null) ...[
          const SizedBox(height: 4),
          Text(
            '下载：${summary.state} · '
            '${_formatBytes(summary.bytesCompleted)}'
            '${summary.bytesTotal == null ? '' : ' / ${_formatBytes(summary.bytesTotal!)}'}'
            '${summary.providers.isEmpty ? '' : ' · ${summary.providers}'}',
            style: TextStyle(
              color: scheme.onSurfaceVariant,
              fontSize: 12,
            ),
          ),
        ],
      ],
    );
  }
}

IconData _modeIcon(PlaybackMode mode) => switch (mode) {
  PlaybackMode.sequence => Icons.arrow_right_alt,
  PlaybackMode.repeatAll => Icons.repeat,
  PlaybackMode.repeatOne => Icons.repeat_one,
  PlaybackMode.shuffle => Icons.shuffle,
};

String _modeLabel(PlaybackMode mode) => switch (mode) {
  PlaybackMode.sequence => '顺序播放',
  PlaybackMode.repeatAll => '列表循环',
  PlaybackMode.repeatOne => '单曲循环',
  PlaybackMode.shuffle => '随机播放',
};
