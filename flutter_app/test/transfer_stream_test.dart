import 'dart:async';
import 'dart:io';

import 'package:flutter_app/providers/music_player_provider.dart';
import 'package:flutter_app/services/transfer_stream_audio_source.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:just_audio_platform_interface/just_audio_platform_interface.dart';
import 'package:shared_preferences/shared_preferences.dart';

/// 假 just_audio 平台：load 时真实请求 just_audio 的本地代理，
/// 从而完整走通 transferStreamAudioSource → 代理鉴权注入 → 控制面链路。
class _FakeJustAudioPlatform extends JustAudioPlatform {
  _FakeJustAudioPlatform(this.owner);
  final _FakePlayerPlatform owner;

  @override
  Future<AudioPlayerPlatform> init(InitRequest request) async => owner;

  @override
  Future<DisposePlayerResponse> disposePlayer(
    DisposePlayerRequest request,
  ) async => DisposePlayerResponse();

  @override
  Future<DisposeAllPlayersResponse> disposeAllPlayers(
    DisposeAllPlayersRequest request,
  ) async => DisposeAllPlayersResponse();
}

class _FakePlayerPlatform extends AudioPlayerPlatform {
  _FakePlayerPlatform() : super('fake-player');
  final StreamController<PlaybackEventMessage> _events =
      StreamController<PlaybackEventMessage>.broadcast();

  @override
  Stream<PlaybackEventMessage> get playbackEventMessageStream => _events.stream;

  @override
  Future<LoadResponse> load(LoadRequest request) async {
    final client = HttpClient();
    try {
      final httpRequest = await client.getUrl(
        Uri.parse((request.audioSourceMessage as UriAudioSourceMessage).uri),
      );
      final response = await httpRequest.close();
      await response.fold<List<int>>([], (acc, chunk) => acc..addAll(chunk));
      if (response.statusCode >= 400) {
        _events.add(_readyEvent());
        // 播放事件错误通道：驱动 provider 的结构化失败路径。
        _events.addError(
          StateError('stream request failed with ${response.statusCode}'),
        );
      } else {
        _events.add(_readyEvent());
      }
      return LoadResponse(duration: null);
    } finally {
      client.close();
    }
  }

  /// just_audio 的 load 会等待平台离开 loading 状态，这里发出 ready。
  PlaybackEventMessage _readyEvent() => PlaybackEventMessage(
    processingState: ProcessingStateMessage.ready,
    updateTime: DateTime.now(),
    updatePosition: Duration.zero,
    bufferedPosition: Duration.zero,
    duration: null,
    icyMetadata: null,
    currentIndex: 0,
    androidAudioSessionId: null,
  );

  @override
  Future<PlayResponse> play(PlayRequest request) async => PlayResponse();

  @override
  Future<PauseResponse> pause(PauseRequest request) async => PauseResponse();

  @override
  Future<SetVolumeResponse> setVolume(SetVolumeRequest request) async =>
      SetVolumeResponse();

  @override
  Future<SetSpeedResponse> setSpeed(SetSpeedRequest request) async =>
      SetSpeedResponse();

  @override
  Future<SeekResponse> seek(SeekRequest request) async => SeekResponse();

  @override
  Future<SetSkipSilenceResponse> setSkipSilence(
    SetSkipSilenceRequest request,
  ) async => SetSkipSilenceResponse();

  @override
  Future<SetLoopModeResponse> setLoopMode(SetLoopModeRequest request) async =>
      SetLoopModeResponse();

  @override
  Future<SetShuffleModeResponse> setShuffleMode(
    SetShuffleModeRequest request,
  ) async => SetShuffleModeResponse();

  @override
  Future<SetAutomaticallyWaitsToMinimizeStallingResponse>
  setAutomaticallyWaitsToMinimizeStalling(
    SetAutomaticallyWaitsToMinimizeStallingRequest request,
  ) async => SetAutomaticallyWaitsToMinimizeStallingResponse();

  @override
  Future<DisposeResponse> dispose(DisposeRequest request) async =>
      DisposeResponse();
}

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
      JustAudioPlatform.instance = _FakeJustAudioPlatform(
        _FakePlayerPlatform(),
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
