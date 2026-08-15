import 'package:flutter/material.dart';
import 'package:flutter_app/models/music.dart';
import 'package:flutter_app/providers/control_plane_provider.dart';
import 'package:flutter_app/providers/music_player_provider.dart';
import 'package:flutter_app/screens/player_screen.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  setUp(() {
    SharedPreferences.setMockInitialValues({});
  });

  group('transferTaskSummary', () {
    test('空任务返回 unknown 状态', () {
      final summary = transferTaskSummary(null);
      expect(summary.state, 'unknown');
      expect(summary.bytesCompleted, 0);
      expect(summary.bytesTotal, isNull);
      expect(summary.providers, isEmpty);
    });

    test('解析字节进度与 Provider 列表', () {
      final summary = transferTaskSummary({
        'task_id': 'tr_x',
        'state': 'transferring',
        'bytes_completed': 4096,
        'bytes_total': 16384,
        'providers': ['configured-ipfs', 'embedded-bitswap'],
      });
      expect(summary.state, 'transferring');
      expect(summary.bytesCompleted, 4096);
      expect(summary.bytesTotal, 16384);
      expect(summary.providers, 'configured-ipfs, embedded-bitswap');
    });
  });

  testWidgets('播放页显示真实来源与边下边播下载状态', (tester) async {
    final player = MusicPlayerProvider();
    final control = ControlPlaneProvider();
    addTearDown(player.dispose);
    addTearDown(control.dispose);
    await player.ready;
    player.debugSetCurrentTrack(
      Music(
        id: 'transfer-tr_ui',
        title: '串流测试',
        artist: 'IPFS 边下边播',
        album: '网络专辑',
        duration: '1:00',
        sourceType: TrackSourceType.ipfs,
        availability: TrackAvailability.available,
      ),
    );
    expect(player.sourceLabel, 'IPFS 边下边播');
    expect(player.transferTaskId, 'tr_ui');

    control.debugSetTransfers([
      {
        'task_id': 'tr_ui',
        'state': 'transferring',
        'bytes_completed': 4096,
        'bytes_total': 16384,
        'providers': ['configured-ipfs'],
      },
    ]);

    await tester.pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<MusicPlayerProvider>.value(value: player),
          ChangeNotifierProvider<ControlPlaneProvider>.value(value: control),
        ],
        child: const MaterialApp(home: PlayerScreen()),
      ),
    );
    await tester.pump();

    expect(find.textContaining('来源：IPFS 边下边播'), findsOneWidget);
    expect(
      find.textContaining('下载：transferring · 4.0 KiB / 16.0 KiB'),
      findsOneWidget,
    );
    expect(find.textContaining('configured-ipfs'), findsOneWidget);
  });

  testWidgets('本地文件曲目显示本地来源标签', (tester) async {
    final player = MusicPlayerProvider();
    final control = ControlPlaneProvider();
    addTearDown(player.dispose);
    addTearDown(control.dispose);
    await player.ready;
    player.debugSetCurrentTrack(
      Music(
        id: 'local-1',
        title: '本地曲目',
        artist: 'Artist',
        album: 'Album',
        duration: '3:00',
        filePath: '/music/local.mp3',
        sourceType: TrackSourceType.localFile,
        availability: TrackAvailability.available,
      ),
    );
    expect(player.sourceLabel, '本地文件');
    expect(player.transferTaskId, isNull);

    await tester.pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<MusicPlayerProvider>.value(value: player),
          ChangeNotifierProvider<ControlPlaneProvider>.value(value: control),
        ],
        child: const MaterialApp(home: PlayerScreen()),
      ),
    );
    await tester.pump();
    expect(find.textContaining('来源：本地文件'), findsOneWidget);
    // 非传输曲目不显示下载状态行。
    expect(find.textContaining('下载：'), findsNothing);
  });
}
