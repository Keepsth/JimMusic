import 'dart:math';

import 'package:flutter_app/models/playback_mode.dart';
import 'package:flutter_app/providers/music_player_provider.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  setUp(() {
    SharedPreferences.setMockInitialValues({});
  });

  group('evaluateAdvance（PLR-102 队列边界）', () {
    test('顺序模式：绕回队首视为结束并停止', () {
      expect(
        evaluateAdvance(
          isAdvance: true,
          currentIndex: 2,
          advancedIndex: 0,
          mode: PlaybackMode.sequence,
        ),
        PlaybackDecision.stop,
      );
      // 正常 +1 推进则接受。
      expect(
        evaluateAdvance(
          isAdvance: true,
          currentIndex: 0,
          advancedIndex: 1,
          mode: PlaybackMode.sequence,
        ),
        PlaybackDecision.accept,
      );
    });

    test('单曲循环：切歌时回到当前曲目', () {
      expect(
        evaluateAdvance(
          isAdvance: true,
          currentIndex: 1,
          advancedIndex: 2,
          mode: PlaybackMode.repeatOne,
        ),
        PlaybackDecision.replayCurrent,
      );
    });

    test('列表循环与随机接受推进；非切歌事件恒接受', () {
      for (final mode in [PlaybackMode.repeatAll, PlaybackMode.shuffle]) {
        expect(
          evaluateAdvance(
            isAdvance: true,
            currentIndex: 2,
            advancedIndex: 0,
            mode: mode,
          ),
          PlaybackDecision.accept,
        );
      }
      expect(
        evaluateAdvance(
          isAdvance: false,
          currentIndex: 0,
          advancedIndex: 0,
          mode: PlaybackMode.sequence,
        ),
        PlaybackDecision.accept,
      );
    });
  });

  group('shuffledPlaylist', () {
    test('固定种子洗牌确定且不丢元素', () {
      final source = List.generate(8, (index) => index);
      final first = shuffledPlaylist(source, Random(42));
      final second = shuffledPlaylist(source, Random(42));
      expect(first, second);
      expect(first.toSet(), source.toSet());
      expect(first.length, source.length);
    });
  });

  group('MusicPlayerProvider.playbackMode', () {
    test('模式切换按 顺序→列表→单曲→随机→顺序 循环并持久化', () async {
      final provider = MusicPlayerProvider();
      addTearDown(provider.dispose);
      await provider.ready;
      expect(provider.playbackMode, PlaybackMode.sequence);

      await provider.cyclePlaybackMode();
      expect(provider.playbackMode, PlaybackMode.repeatAll);
      await provider.cyclePlaybackMode();
      expect(provider.playbackMode, PlaybackMode.repeatOne);
      await provider.cyclePlaybackMode();
      expect(provider.playbackMode, PlaybackMode.shuffle);
      await provider.cyclePlaybackMode();
      expect(provider.playbackMode, PlaybackMode.sequence);

      // 持久化后重启恢复。
      await provider.cyclePlaybackMode();
      final restored = MusicPlayerProvider();
      addTearDown(restored.dispose);
      await restored.ready;
      expect(restored.playbackMode, PlaybackMode.repeatAll);
      expect(restored.playerState, PlayerState.stopped);
    });
  });
}
