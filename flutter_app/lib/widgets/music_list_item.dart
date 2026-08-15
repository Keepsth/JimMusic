import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../models/music.dart';
import '../providers/music_player_provider.dart';
import 'geek_cover.dart';

/// 音乐列表项：展示曲目信息、收藏状态与可选操作。
class MusicListItem extends StatelessWidget {
  final Music music;
  final bool isPlaying;
  final VoidCallback onTap;

  /// 可选的额外 trailing 控件（如播放列表移除按钮）。
  final Widget? trailingExtra;

  /// 是否显示收藏心形按钮（默认显示）。
  final bool showFavorite;

  const MusicListItem({
    super.key,
    required this.music,
    required this.isPlaying,
    required this.onTap,
    this.trailingExtra,
    this.showFavorite = true,
  });

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final primaryText = scheme.onSurface;
    final secondaryText = scheme.onSurfaceVariant;

    return Consumer<MusicPlayerProvider>(
      builder: (context, player, _) {
        final favorite = player.isFavorite(music);
        return ListTile(
          dense: true,
          contentPadding: const EdgeInsets.symmetric(
            horizontal: 12,
            vertical: 2,
          ),
          leading: GeekCover(
            seed: music.id,
            label: music.title,
            size: 44,
            borderRadius: BorderRadius.circular(2),
          ),
          title: Text(
            music.title,
            style: TextStyle(
              color: isPlaying ? scheme.primary : primaryText,
              fontWeight: isPlaying ? FontWeight.bold : FontWeight.normal,
              fontFamilyFallback: const ['monospace'],
              fontSize: 14,
            ),
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
          ),
          subtitle: Text(
            music.availability == TrackAvailability.available
                ? (music.album.isNotEmpty
                      ? '${music.artist} • ${music.album}'
                      : music.artist)
                : '${music.artist} • ${music.unavailableReason ?? music.availability.name}',
            style: TextStyle(
              color: music.availability == TrackAvailability.available
                  ? secondaryText
                  : scheme.error,
              fontSize: 11,
            ),
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
          ),
          trailing: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Tooltip(
                message: '来源：${_sourceLabel(music.sourceType)}',
                child: Icon(
                  _sourceIcon(music.sourceType),
                  size: 14,
                  color: secondaryText,
                ),
              ),
              const SizedBox(width: 6),
              Text(
                music.duration,
                style: TextStyle(color: secondaryText, fontSize: 11),
              ),
              if (showFavorite) ...[
                const SizedBox(width: 2),
                IconButton(
                  icon: Icon(
                    favorite ? Icons.favorite : Icons.favorite_border,
                    color: favorite ? scheme.primary : secondaryText,
                    size: 18,
                  ),
                  onPressed: () => player.toggleFavorite(music),
                ),
              ],
              if (music.availability != TrackAvailability.available)
                Icon(Icons.cloud_off_outlined, color: scheme.error, size: 18),
              if (isPlaying)
                Icon(Icons.play_arrow, color: scheme.primary, size: 18),
              if (trailingExtra != null) trailingExtra!,
            ],
          ),
          onTap: music.availability == TrackAvailability.available
              ? onTap
              : null,
        );
      },
    );
  }
}

/// UI-002：本地、IPFS 与社区条目在列表中可区分。
String _sourceLabel(TrackSourceType type) => switch (type) {
  TrackSourceType.localFile => '本地文件',
  TrackSourceType.localMemory => '内存导入',
  TrackSourceType.cached => '本地缓存',
  TrackSourceType.ipfs => 'IPFS',
  TrackSourceType.community => '社区来源',
};

IconData _sourceIcon(TrackSourceType type) => switch (type) {
  TrackSourceType.localFile || TrackSourceType.localMemory => Icons.folder,
  TrackSourceType.cached => Icons.cached,
  TrackSourceType.ipfs => Icons.lan,
  TrackSourceType.community => Icons.groups,
};
