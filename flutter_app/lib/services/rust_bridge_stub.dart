import 'dart:async';

/// Web 平台 stub：Web 不支持 dart:ffi，桥不可用，所有调用安全降级为 no-op。
/// 上层（`music_player_provider`）据此回退到 just_audio。

class PlaybackEventType {
  static const int stopped = 0;
  static const int playing = 1;
  static const int paused = 2;
  static const int progress = 3;
  static const int error = 4;
}

class PlaybackState {
  static const int stopped = 0;
  static const int playing = 1;
  static const int paused = 2;
}

class BridgeEvent {
  final int eventType;
  final double value;
  const BridgeEvent(this.eventType, this.value);

  bool get isProgress => eventType == PlaybackEventType.progress;
  bool get isStopped => eventType == PlaybackEventType.stopped;
  bool get isPlaying => eventType == PlaybackEventType.playing;
  bool get isPaused => eventType == PlaybackEventType.paused;
  bool get isError => eventType == PlaybackEventType.error;
}

class RustBridge {
  RustBridge._();

  static final RustBridge instance = RustBridge._();

  factory RustBridge.openWith(String libPath) => RustBridge._();

  bool get available => false;
  bool get readyForPlayback => false;

  Stream<BridgeEvent> get events => const Stream<BridgeEvent>.empty();

  int? setOutput(String path) => null;
  int? setQueue(List<String> paths) => null;
  int? playTrack(int index) => null;
  int? setCrossfade(Duration duration, {bool equalPower = true}) => null;
  int? playFile(String trackId, String path) => null;
  int? next() => null;
  int? previous() => null;
  int currentIndex() => 0;
  int? pause() => null;
  int? resume() => null;
  int? stop() => null;
  int? seek(double positionSecs) => null;
  double position() => 0.0;
  double duration() => 0.0;
  int state() => PlaybackState.stopped;
  String? lastError() => null;
  String? outputSession() => null;
  int? startNode(String repositoryPath) => null;
  int? setNodeForeground(bool foreground) => null;
  int? connectNode(String address) => null;
  int? stopNode() => null;
  Map<String, dynamic>? nodeStatus() => null;

  void dispose() {}
}
