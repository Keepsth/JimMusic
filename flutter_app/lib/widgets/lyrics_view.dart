import 'package:flutter/material.dart';

import '../models/lyrics.dart';

/// 同步歌词展示：按播放位置高亮当前行，向上滚动居中。
class LyricsView extends StatelessWidget {
  final Lyrics lyrics;
  final Duration position;
  final Color? highlightColor;

  const LyricsView({
    super.key,
    required this.lyrics,
    required this.position,
    this.highlightColor,
  });

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final current = lyrics.indexAt(position);

    if (lyrics.isEmpty) {
      return Center(
        child: Text('暂无歌词', style: TextStyle(color: scheme.onSurfaceVariant)),
      );
    }

    return ListView.builder(
      itemCount: lyrics.lines.length,
      itemBuilder: (context, index) {
        final line = lyrics.lines[index];
        final isCurrent = index == current;
        return Padding(
          padding: const EdgeInsets.symmetric(vertical: 6),
          child: Text(
            line.text.isEmpty ? '♪' : line.text,
            textAlign: TextAlign.center,
            style: TextStyle(
              fontSize: isCurrent ? 18 : 14,
              fontWeight: isCurrent ? FontWeight.bold : FontWeight.normal,
              color: isCurrent
                  ? (highlightColor ?? scheme.primary)
                  : scheme.onSurfaceVariant,
            ),
          ),
        );
      },
    );
  }
}
