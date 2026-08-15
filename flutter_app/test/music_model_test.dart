import 'dart:typed_data';

import 'package:flutter_test/flutter_test.dart';

import 'package:flutter_app/models/music.dart';

void main() {
  test('fromBytes 从文件名派生标题与艺术家', () {
    final bytes = Uint8List.fromList([1, 2, 3, 4]);
    final m = Music.fromBytes(
      name: 'Artist - Song Title.mp3',
      bytes: bytes,
      mimeType: 'audio/mpeg',
      index: 0,
    );
    expect(m.title, 'Song Title');
    expect(m.artist, 'Artist');
    expect(m.audioBytes, bytes);
    expect(m.mimeType, 'audio/mpeg');
    expect(m.filePath, isNull);
  });

  test('fromBytes 无分隔符时标题为文件名 stem', () {
    final m = Music.fromBytes(name: 'song.wav', bytes: Uint8List(0));
    expect(m.title, 'song');
    expect(m.artist, '未知艺术家');
  });

  test('内存曲目 ID 在 native 与 JavaScript 可精确复现', () {
    final first = Music.fromBytes(name: 'a', bytes: Uint8List(0), index: 0);
    final second = Music.fromBytes(name: 'a', bytes: Uint8List(0), index: 0);
    expect(first.id, 'memory_e3bf46e8');
    expect(second.id, first.id);
  });

  test('copyWith 保留内存字节（收藏切换不丢 Web 音频数据）', () {
    final bytes = Uint8List.fromList([9, 8, 7]);
    final m = Music.fromBytes(
      name: 'a.mp3',
      bytes: bytes,
      mimeType: 'audio/mpeg',
    );
    final toggled = m.copyWith(isFavorite: true);
    expect(toggled.isFavorite, isTrue);
    expect(toggled.audioBytes, bytes);
    expect(toggled.mimeType, 'audio/mpeg');
  });
}
