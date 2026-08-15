import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:flutter_app/models/music.dart';
import 'package:flutter_app/providers/music_player_provider.dart';

Music track(String id, String title) => Music(
  id: id,
  title: title,
  artist: 'Artist',
  album: 'Album',
  duration: '1:00',
  availability: TrackAvailability.missing,
  unavailableReason: 'test source is intentionally missing',
);

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  setUp(() {
    SharedPreferences.setMockInitialValues({});
  });

  test('媒体库默认为空且导入后持久化', () async {
    final provider = MusicPlayerProvider();
    await provider.loadForTest();
    expect(provider.library, isEmpty);

    await provider.mergeLibraryTracks([track('one', 'One')]);
    final restored = MusicPlayerProvider();
    await restored.loadForTest();
    expect(restored.library.map((music) => music.id), ['one']);
  });

  test('收藏切换与持久化', () async {
    final provider = MusicPlayerProvider();
    await provider.loadForTest();
    final music = track('one', 'One');
    await provider.mergeLibraryTracks([music]);

    await provider.toggleFavorite(music);
    expect(provider.isFavorite(music), isTrue);
    expect(provider.favorites.length, 1);

    await provider.toggleFavorite(music);
    expect(provider.isFavorite(music), isFalse);
    expect(provider.favorites, isEmpty);
  });

  test('播放列表创建与增删', () async {
    final provider = MusicPlayerProvider();
    await provider.loadForTest();
    final music = track('one', 'One');
    await provider.mergeLibraryTracks([music]);

    await provider.createPlaylist('我的最爱');
    await provider.addToNamedPlaylist('我的最爱', music);
    expect(provider.tracksInPlaylist('我的最爱').length, 1);

    await provider.removeFromNamedPlaylist('我的最爱', music.id);
    expect(provider.tracksInPlaylist('我的最爱'), isEmpty);
    await provider.deletePlaylist('我的最爱');
    expect(provider.playlists.containsKey('我的最爱'), isFalse);
  });

  test('搜索过滤真实媒体库', () async {
    final provider = MusicPlayerProvider();
    await provider.loadForTest();
    await provider.mergeLibraryTracks([
      track('one', 'Shape'),
      track('two', 'Other'),
    ]);

    provider.setSearchQuery('shape');
    expect(provider.filteredLibrary.map((music) => music.id), ['one']);
    provider.setSearchQuery('');
    expect(provider.filteredLibrary.length, 2);
  });

  test('不可用音源不会伪装成播放成功', () async {
    final provider = MusicPlayerProvider();
    await provider.loadForTest();
    final music = track('missing', 'Missing');
    await provider.mergeLibraryTracks([music]);

    await provider.play(music);
    expect(provider.playerState, PlayerState.stopped);
    expect(provider.isPlaying, isFalse);
    expect(provider.playbackError, contains('intentionally missing'));
  });

  test('恢复会话保留音量但绝不自动播放', () async {
    final provider = MusicPlayerProvider();
    await provider.loadForTest();
    await provider.setVolume(0.35);
    await provider.setMuted(true);

    final restored = MusicPlayerProvider();
    await restored.loadForTest();
    expect(restored.volume, closeTo(0.35, 0.001));
    expect(restored.muted, isTrue);
    expect(restored.playerState, PlayerState.stopped);
  });

  test('无缝与交叉淡化设置会持久化', () async {
    final provider = MusicPlayerProvider();
    await provider.loadForTest();
    await provider.setCrossfade(
      const Duration(milliseconds: 3500),
      equalPower: false,
    );

    final restored = MusicPlayerProvider();
    await restored.loadForTest();
    expect(restored.crossfadeMilliseconds, 3500);
    expect(restored.crossfadeEqualPower, isFalse);

    await restored.setCrossfade(Duration.zero);
    expect(restored.crossfadeMilliseconds, 0);
  });
}
