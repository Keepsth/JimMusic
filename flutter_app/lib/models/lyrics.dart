/// 歌词行：一个时间点 + 对应文本。
class LyricsLine {
  final Duration time;
  final String text;

  const LyricsLine(this.time, this.text);
}

/// 同步歌词（LRC 格式解析与时间索引）。
///
/// 对应需求 3.3「资源管理：歌词同步」——按播放位置定位当前歌词行。
class Lyrics {
  final List<LyricsLine> lines;

  const Lyrics(this.lines);

  /// 是否为空。
  bool get isEmpty => lines.isEmpty;

  /// 解析 LRC 文本。支持形如 `[mm:ss.xx] 歌词` 的时间标签，忽略元数据行。
  static Lyrics parseLrc(String lrc) {
    final regex = RegExp(r'\[(\d{1,2}):(\d{1,2})(?:\.(\d{1,3}))?\]\s*(.*)');
    final parsed = <LyricsLine>[];
    for (final raw in lrc.split('\n')) {
      final m = regex.firstMatch(raw);
      if (m == null) continue;
      final min = int.parse(m.group(1)!);
      final sec = int.parse(m.group(2)!);
      final fracStr = m.group(3) ?? '0';
      final ms = int.parse(fracStr.padRight(3, '0'));
      final text = (m.group(4) ?? '').trim();
      parsed.add(
        LyricsLine(
          Duration(minutes: min, seconds: sec, milliseconds: ms),
          text,
        ),
      );
    }
    parsed.sort((a, b) => a.time.compareTo(b.time));
    return Lyrics(parsed);
  }

  /// 返回给定时间点应显示的歌词行索引；尚未开始返回 -1。
  int indexAt(Duration t) {
    var idx = -1;
    for (var i = 0; i < lines.length; i++) {
      if (lines[i].time <= t) {
        idx = i;
      } else {
        break;
      }
    }
    return idx;
  }

  /// 给定时间点的当前歌词行（可为 null）。
  LyricsLine? lineAt(Duration t) {
    final i = indexAt(t);
    return i < 0 ? null : lines[i];
  }
}
