import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../models/lyrics.dart';
import '../providers/music_player_provider.dart';
import '../widgets/geek_cover.dart';
import '../widgets/lyrics_view.dart';

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

          return Padding(
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

                const SizedBox(height: 20),
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

                const Spacer(),
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
