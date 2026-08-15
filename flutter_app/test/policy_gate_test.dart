import 'package:flutter/material.dart';
import 'package:flutter_app/models/music.dart';
import 'package:flutter_app/providers/control_plane_provider.dart';
import 'package:flutter_app/providers/music_player_provider.dart';
import 'package:flutter_app/services/control_api.dart';
import 'package:flutter_app/widgets/policy_gate.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:provider/provider.dart';

/// 记录申诉 mutation 的假控制面 API。
class _AppealFakeApi extends ControlApi {
  _AppealFakeApi() : super(endpoint: 'http://127.0.0.1:9/v1', token: 'test');

  final List<(String, Object?)> posts = [];

  @override
  Future<dynamic> post(
    String path, [
    Object? body,
    Map<String, String>? headers,
  ]) async {
    posts.add((path, body));
    return {};
  }
}

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

  testWidgets('详情入口可提交匿名申诉（SEC-009）', (tester) async {
    final api = _AppealFakeApi();
    final control = ControlPlaneProvider()..debugApiFactory = (_, _) => api;
    final music = policyTrack('d', action: 'block', reason: '强制');
    await tester.pumpWidget(
      MultiProvider(
        providers: [
          ChangeNotifierProvider<ControlPlaneProvider>.value(value: control),
        ],
        child: MaterialApp(
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
      ),
    );
    await tester.tap(find.text('detail'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('申诉'));
    await tester.pumpAndSettle();
    expect(find.text('申诉社区策略'), findsOneWidget);

    // 空说明被拒绝。
    await tester.tap(find.text('提交申诉'));
    await tester.pump();
    expect(find.text('申诉说明不能为空'), findsOneWidget);

    await tester.enterText(
      find.widgetWithText(TextField, '申诉说明 *'),
      '该作品为我本人原创',
    );
    await tester.tap(find.text('提交申诉'));
    await tester.pumpAndSettle();
    expect(api.posts, hasLength(1));
    expect(api.posts.single.$1, '/policy/bafymanifest-d/appeal');
    expect(find.text('申诉已提交，等待社区源审核'), findsOneWidget);
    // 让 SnackBar 计时器结束，避免 pending timer。
    await tester.pump(const Duration(seconds: 5));
    control.dispose();
  });
}
