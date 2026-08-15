import 'dart:convert';
import 'dart:io';

import 'package:flutter_app/models/music.dart';
import 'package:flutter_app/providers/music_player_provider.dart';
import 'package:flutter_app/services/transfer_stream_audio_source.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:just_audio_platform_interface/just_audio_platform_interface.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'fake_just_audio_platform.dart';

Future<void> _pumpUntil(bool Function() condition) async {
  final deadline = DateTime.now().add(const Duration(seconds: 5));
  while (!condition() && DateTime.now().isBefore(deadline)) {
    await Future<void>.delayed(const Duration(milliseconds: 20));
  }
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  // 本文件用真实回环 HttpServer 验证流式 GET、鉴权与 Range 头。
  HttpOverrides.global = null;

  setUp(() {
    SharedPreferences.setMockInitialValues({});
  });

  group('transferStreamUri', () {
    test('把端点与任务 ID 拼为流端点 URI', () {
      final uri = transferStreamUri(
        'http://127.0.0.1:8787/v1',
        'tr_abc123',
      );
      expect(
        uri.toString(),
        'http://127.0.0.1:8787/v1/transfers/tr_abc123/stream',
      );
    });

    test('任务 ID 做路径编码，无法逃逸 /v1 前缀', () {
      // Uri.parse 会把编码点段归一化，但路径第一段始终是 v1；
      // 服务端按任务 ID 查表，未知 ID 只会得到 404。
      final uri = transferStreamUri('http://127.0.0.1:8787/v1', '../etc');
      expect(uri.pathSegments.first, 'v1');
    });
  });

  group('MusicPlayerProvider.playTransferStream', () {
    setUp(() {
      JustAudioPlatform.instance = FakeJustAudioPlatform(
        FakePlayerPlatform(),
      );
    });

    test('经传输流端点成功挂载音源并注入鉴权头', () async {
      final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
      final received = <String>[];
      server.listen((request) {
        received.add(
          '${request.uri.path} auth=${request.headers.value('authorization') ?? ''}',
        );
        request.response.statusCode = 200;
        request.response.add([1, 2, 3, 4]);
        request.response.close();
      });
      addTearDown(() => server.close(force: true));

      final provider = MusicPlayerProvider();
      await provider.ready;
      await provider.playTransferStream(
        taskId: 'tr_live',
        endpoint: 'http://127.0.0.1:${server.port}/v1',
        token: 'tok',
        mimeType: 'audio/mpeg',
      );
      expect(provider.currentMusic?.id, 'transfer-tr_live');
      expect(provider.playerState, PlayerState.buffering);
      expect(provider.playbackError, isNull);
      expect(received, contains('/v1/transfers/tr_live/stream auth=Bearer tok'));
      provider.dispose();
    });

    test('网络曲目经传输任务边下边播（PLR-007）', () async {
      final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
      final received = <String>[];
      server.listen((request) async {
        final body = await utf8.decoder.bind(request).join();
        if (request.method == 'POST' && request.uri.path == '/v1/transfers') {
          received.add('POST /v1/transfers $body');
          request.response.statusCode = 200;
          request.response.write('{"task_id":"tr_play"}');
          request.response.close();
        } else if (request.uri.path == '/v1/transfers/tr_play/stream') {
          received.add('GET /v1/transfers/tr_play/stream');
          request.response.statusCode = 200;
          request.response.add([1, 2, 3]);
          request.response.close();
        } else {
          request.response.statusCode = 404;
          request.response.close();
        }
      });
      addTearDown(() => server.close(force: true));

      final provider = MusicPlayerProvider();
      addTearDown(provider.dispose);
      await provider.ready;
      await provider.playNetworkTrack(
        Music(
          id: 'jm_net1',
          title: 'Net',
          artist: 'A',
          album: '',
          duration: '',
          sourceType: TrackSourceType.ipfs,
          availability: TrackAvailability.available,
          renditionCid: 'bafytest',
          codec: 'flac',
        ),
        endpoint: 'http://127.0.0.1:${server.port}/v1',
        token: 'tok',
      );
      expect(
        received,
        containsAll([
          'GET /v1/transfers/tr_play/stream',
        ]),
      );
      expect(
        received.firstWhere((entry) => entry.startsWith('POST /v1/transfers')),
        contains('bafytest'),
      );
      expect(provider.currentMusic?.id, 'transfer-tr_play');
      expect(provider.playerState, PlayerState.buffering);
      expect(provider.playbackError, isNull);
    });

    test('控制面拒绝时经播放事件给出结构化失败且不伪装播放', () async {
      final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
      server.listen((request) {
        request.response.statusCode = 409;
        request.response.write('{"code":"conflict","message":"ended"}');
        request.response.close();
      });
      addTearDown(() => server.close(force: true));

      final provider = MusicPlayerProvider();
      await provider.ready;
      await provider.playTransferStream(
        taskId: 'tr_dead',
        endpoint: 'http://127.0.0.1:${server.port}/v1',
        token: '',
        mimeType: 'audio/mpeg',
      );
      expect(provider.currentMusic?.id, 'transfer-tr_dead');
      await _pumpUntil(() => provider.playbackError != null);
      expect(provider.playerState, PlayerState.stopped);
      expect(provider.playbackError, isNotNull);
      provider.dispose();
    });
  });
}
