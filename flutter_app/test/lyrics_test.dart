import 'package:flutter_test/flutter_test.dart';

import 'package:flutter_app/models/lyrics.dart';

void main() {
  test('解析 LRC 并排序', () {
    const lrc = '''
[00:15.00] 第二行
[00:12.34] 第一行
[00:12.34] 同行
[ti:标题]
''';
    final lyrics = Lyrics.parseLrc(lrc);
    expect(lyrics.lines.length, 3);
    expect(lyrics.lines[0].text, '第一行');
    expect(
      lyrics.lines[0].time,
      const Duration(minutes: 0, seconds: 12, milliseconds: 340),
    );
    expect(lyrics.lines[2].time, const Duration(minutes: 0, seconds: 15));
  });

  test('按时间索引当前歌词行', () {
    final lyrics = Lyrics.parseLrc('[00:10.00] 十秒\n[00:20.00] 二十秒\n');
    expect(lyrics.indexAt(const Duration(seconds: 5)), -1);
    expect(lyrics.indexAt(const Duration(seconds: 10)), 0);
    expect(lyrics.indexAt(const Duration(seconds: 19)), 0);
    expect(lyrics.indexAt(const Duration(seconds: 20)), 1);
    expect(lyrics.lineAt(const Duration(seconds: 25))?.text, '二十秒');
  });

  test('空歌词', () {
    expect(Lyrics.parseLrc('').isEmpty, isTrue);
    expect(Lyrics.parseLrc('[ti:标题]').isEmpty, isTrue);
  });
}
