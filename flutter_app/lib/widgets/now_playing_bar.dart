import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../providers/music_player_provider.dart';
import 'geek_cover.dart';

/// 底部迷你播放控制条。
class NowPlayingBar extends StatelessWidget {
  final VoidCallback onTap;

  const NowPlayingBar({super.key, required this.onTap});

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Consumer<MusicPlayerProvider>(
      builder: (context, player, child) {
        final music = player.currentMusic;
        if (music == null) return const SizedBox.shrink();

        return GestureDetector(
          onTap: onTap,
          child: Container(
            height: 52,
            decoration: BoxDecoration(
              color: scheme.surfaceContainerHighest,
              border: Border(
                top: BorderSide(color: scheme.outlineVariant, width: 1),
              ),
            ),
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 8),
              child: Row(
                children: [
                  GeekCover(seed: music.id, label: music.title, size: 40),
                  const SizedBox(width: 8),
                  Expanded(
                    child: Column(
                      mainAxisAlignment: MainAxisAlignment.center,
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          music.title,
                          style: TextStyle(
                            color: scheme.onSurface,
                            fontSize: 13,
                            fontWeight: FontWeight.w500,
                            fontFamilyFallback: const ['monospace'],
                          ),
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                        ),
                        Text(
                          music.artist,
                          style: TextStyle(
                            color: scheme.onSurfaceVariant,
                            fontSize: 11,
                          ),
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                        ),
                      ],
                    ),
                  ),
                  IconButton(
                    onPressed: player.togglePlayPause,
                    icon: player.isBuffering
                        ? SizedBox(
                            width: 18,
                            height: 18,
                            child: CircularProgressIndicator(
                              strokeWidth: 2,
                              color: scheme.primary,
                            ),
                          )
                        : Icon(
                            player.isPlaying ? Icons.pause : Icons.play_arrow,
                            color: scheme.primary,
                            size: 22,
                          ),
                  ),
                  IconButton(
                    onPressed: player.next,
                    icon: Icon(
                      Icons.skip_next,
                      color: scheme.onSurface,
                      size: 22,
                    ),
                  ),
                ],
              ),
            ),
          ),
        );
      },
    );
  }
}
