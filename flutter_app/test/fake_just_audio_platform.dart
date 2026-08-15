import 'dart:async';
import 'dart:io';

import 'package:just_audio_platform_interface/just_audio_platform_interface.dart';

/// 假 just_audio 平台：load 时真实请求 just_audio 的本地代理，
/// 从而完整走通 transferStreamAudioSource → 代理鉴权注入 → 控制面链路。
class FakeJustAudioPlatform extends JustAudioPlatform {
  FakeJustAudioPlatform(this.owner);
  final FakePlayerPlatform owner;

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

class FakePlayerPlatform extends AudioPlayerPlatform {
  FakePlayerPlatform() : super('fake-player');
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
