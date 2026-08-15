import 'package:flutter/material.dart';
import 'package:flutter_app/models/music.dart';
import 'package:flutter_app/providers/music_player_provider.dart';
import 'package:flutter_app/widgets/policy_gate.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';

Music policyTrack(String id, {String? action, String? reason}) => Music(
  id: id,
  title: 'Track $id',
  artist: 'Artist',
  album: 'Album',
  duration: '1:00',
  manifestCid: 'bafymanifest-$id',
  policyAction: action,
  policyReason: reason,
);

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  group('applyPolicyToSearch（COM-006 搜索入口）', () {
    test('hide/block/revoke 移除、demote 降权、warn 保留', () {
      final results = applyPolicyToSearch([
        policyTrack('warn', action: 'warn'),
        policyTrack('hide', action: 'hide'),
        policyTrack('normal'),
        policyTrack('block', action: 'block'),
        policyTrack('demote', action: 'demote'),
        policyTrack('revoke', action: 'revoke'),
      ]);
      expect(results.map((music) => music.id), ['warn', 'normal', 'demote']);
    });

    test('无策略曲目保持原顺序', () {
      final results = applyPolicyToSearch([
        policyTrack('a'),
        policyTrack('b'),
        policyTrack('c'),
      ]);
      expect(results.map((music) => music.id), ['a', 'b', 'c']);
    });
  });

  Future<void> pumpPlayButton(
    WidgetTester tester,
    Music music,
    Future<void> Function() onPlay,
  ) async {
    final player = MusicPlayerProvider();
    await tester.pumpWidget(
      ChangeNotifierProvider<MusicPlayerProvider>.value(
        value: player,
        child: MaterialApp(
          home: Builder(
            builder: (context) => Scaffold(
              body: Center(
                child: ElevatedButton(
                  onPressed: () =>
                      playTrackWithPolicy(context, music, onPlay: onPlay),
                  child: const Text('play'),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }

  testWidgets('block 直接拒绝且不执行播放', (tester) async {
    var calls = 0;
    await pumpPlayButton(
      tester,
      policyTrack('x', action: 'block', reason: '版权'),
      () async => calls++,
    );
    await tester.tap(find.text('play'));
    await tester.pumpAndSettle();
    expect(find.text('社区策略阻止播放'), findsOneWidget);
    expect(find.textContaining('版权'), findsOneWidget);
    expect(calls, 0);
    await tester.tap(find.text('知道了'));
    await tester.pumpAndSettle();
  });

  testWidgets('revoke 同样拒绝', (tester) async {
    var calls = 0;
    await pumpPlayButton(
      tester,
      policyTrack('x', action: 'revoke'),
      () async => calls++,
    );
    await tester.tap(find.text('play'));
    await tester.pumpAndSettle();
    expect(find.text('社区策略阻止播放'), findsOneWidget);
    expect(calls, 0);
  });

  testWidgets('warn 确认后继续播放', (tester) async {
    var calls = 0;
    await pumpPlayButton(
      tester,
      policyTrack('x', action: 'warn', reason: '标记'),
      () async => calls++,
    );
    await tester.tap(find.text('play'));
    await tester.pumpAndSettle();
    expect(find.text('社区策略警告'), findsOneWidget);
    await tester.tap(find.text('继续播放'));
    await tester.pumpAndSettle();
    expect(calls, 1);
  });

  testWidgets('warn 取消不播放', (tester) async {
    var calls = 0;
    await pumpPlayButton(
      tester,
      policyTrack('x', action: 'warn'),
      () async => calls++,
    );
    await tester.tap(find.text('play'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('取消'));
    await tester.pumpAndSettle();
    expect(calls, 0);
  });

  testWidgets('无策略直接播放', (tester) async {
    var calls = 0;
    await pumpPlayButton(tester, policyTrack('x'), () async => calls++);
    await tester.tap(find.text('play'));
    await tester.pumpAndSettle();
    expect(calls, 1);
  });

  testWidgets('详情入口展示策略信息（COM-006 详情入口）', (tester) async {
    final music = policyTrack('d', action: 'warn', reason: '社区标记');
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => Scaffold(
            body: Center(
              child: ElevatedButton(
                onPressed: () => showTrackDetailDialog(context, music),
                child: const Text('detail'),
              ),
            ),
          ),
        ),
      ),
    );
    await tester.tap(find.text('detail'));
    await tester.pumpAndSettle();
    expect(find.textContaining('社区策略：警告'), findsOneWidget);
    expect(find.textContaining('社区标记'), findsOneWidget);
    expect(find.text('本地覆盖'), findsOneWidget);
    await tester.tap(find.text('关闭'));
    await tester.pumpAndSettle();
  });

  testWidgets('详情入口对强制策略不提供本地覆盖', (tester) async {
    final music = policyTrack('d', action: 'block', reason: '强制');
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => Scaffold(
            body: Center(
              child: ElevatedButton(
                onPressed: () => showTrackDetailDialog(context, music),
                child: const Text('detail'),
              ),
            ),
          ),
        ),
      ),
    );
    await tester.tap(find.text('detail'));
    await tester.pumpAndSettle();
    expect(find.textContaining('社区策略：阻止'), findsOneWidget);
    expect(find.text('本地覆盖'), findsNothing);
  });
}
