import 'dart:async';
import 'dart:convert';
import 'dart:ffi';

import 'package:ffi/ffi.dart';
import 'package:flutter/foundation.dart'
    show defaultTargetPlatform, TargetPlatform;

import 'native_library_locator.dart';

/// 播放状态常量（与 backend/app-core/src/host.rs 导出的一致）。
class PlaybackEventType {
  static const int stopped = 0;
  static const int playing = 1;
  static const int paused = 2;
  static const int progress = 3;
  static const int error = 4;
}

/// 播放状态常量（与 host.rs 的 `jimmusic_host_state` 返回一致）。
class PlaybackState {
  static const int stopped = 0;
  static const int playing = 1;
  static const int paused = 2;
}

/// 一条来自 Rust Core 的播放事件（状态/进度）。
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

/// 事件回调 native 签名：`extern "C" fn(i32, f64)`。
typedef _EventCallbackNative = Void Function(Int32 eventType, Double value);

/// Rust 宿主桥：经 dart:ffi 加载 `libapp_core.so`（cdylib），调用其导出的
/// `jimmusic_host_*` C ABI，把播放指令下发到 Rust Core 的 PlaybackEngine，
/// 并接收播放状态/进度事件。
///
/// 这是「打通前后端桥」的前端侧：桌面/移动端用 dart:ffi 直接调用 Rust；Web 端
/// 不支持 dart:ffi 加载原生库（[available] 为 false），由上层回退到 just_audio。
class RustBridge {
  RustBridge._([String? libPath]) {
    _tryOpen(libPath);
  }

  /// 全局单例（默认按平台解析库名）。
  static final RustBridge instance = RustBridge._();

  /// 供测试/定制：以指定路径打开桥。
  factory RustBridge.openWith(String libPath) => RustBridge._(libPath);

  DynamicLibrary? _lib;
  NativeCallable<_EventCallbackNative>? _eventCallable;
  final StreamController<BridgeEvent> _events =
      StreamController<BridgeEvent>.broadcast();

  /// 桥是否可用（原生库已成功加载并完成符号绑定）。
  bool get available => _lib != null;
  bool _outputReady = false;

  /// 只有输出插件真实打开成功后，播放器才把音频交给 Rust Core。
  bool get readyForPlayback => available && _outputReady;

  /// 播放事件流（状态/进度）。
  Stream<BridgeEvent> get events => _events.stream;

  // ---- 绑定的 C 函数 ----
  int Function(Pointer<Utf8>)? _setOutput;
  int Function(Pointer<Utf8>)? _setQueue;
  int Function(int)? _playTrack;
  int Function(int, int)? _setCrossfade;
  int Function(Pointer<Utf8>, Pointer<Utf8>)? _playFile;
  int Function()? _next;
  int Function()? _previous;
  int Function()? _currentIndex;
  int Function()? _pause;
  int Function()? _resume;
  int Function()? _stop;
  int Function(double)? _seek;
  double Function()? _position;
  double Function()? _duration;
  int Function()? _state;
  int Function(Pointer<NativeFunction<_EventCallbackNative>>?)?
  _setEventCallback;
  int Function(Pointer<Uint8>, int)? _lastError;
  int Function(Pointer<Uint8>, int)? _outputSession;
  int Function(Pointer<Utf8>)? _startNode;
  int Function(int)? _setNodeForeground;
  int Function(Pointer<Utf8>)? _connectNode;
  int Function()? _stopNode;
  int Function(Pointer<Uint8>, int)? _nodeStatus;

  void _tryOpen(String? libPath) {
    try {
      final lib = libPath == null && defaultTargetPlatform == TargetPlatform.iOS
          ? DynamicLibrary.process()
          : DynamicLibrary.open(libPath ?? _resolveLibPath());
      _bind(lib);
      _lib = lib;
    } catch (_) {
      _lib = null;
    }
  }

  String _resolveLibPath() {
    switch (defaultTargetPlatform) {
      case TargetPlatform.linux:
        return resolveBundledLibrary('libapp_core.so');
      case TargetPlatform.macOS:
        return resolveBundledLibrary('libapp_core.dylib');
      case TargetPlatform.windows:
        return resolveBundledLibrary('app_core.dll');
      default:
        return 'libapp_core.so';
    }
  }

  void _bind(DynamicLibrary lib) {
    _setOutput = lib
        .lookupFunction<
          Int32 Function(Pointer<Utf8>),
          int Function(Pointer<Utf8>)
        >('jimmusic_host_set_output');
    _setQueue = lib
        .lookupFunction<
          Int32 Function(Pointer<Utf8>),
          int Function(Pointer<Utf8>)
        >('jimmusic_host_set_queue');
    _playTrack = lib.lookupFunction<Int32 Function(Int32), int Function(int)>(
      'jimmusic_host_play_track',
    );
    _setCrossfade = lib
        .lookupFunction<Int32 Function(Uint32, Int32), int Function(int, int)>(
          'jimmusic_host_set_crossfade',
        );
    _playFile = lib
        .lookupFunction<
          Int32 Function(Pointer<Utf8>, Pointer<Utf8>),
          int Function(Pointer<Utf8>, Pointer<Utf8>)
        >('jimmusic_host_play_file');
    _next = lib.lookupFunction<Int32 Function(), int Function()>(
      'jimmusic_host_next',
    );
    _previous = lib.lookupFunction<Int32 Function(), int Function()>(
      'jimmusic_host_previous',
    );
    _currentIndex = lib.lookupFunction<Int32 Function(), int Function()>(
      'jimmusic_host_current_index',
    );
    _pause = lib.lookupFunction<Int32 Function(), int Function()>(
      'jimmusic_host_pause',
    );
    _resume = lib.lookupFunction<Int32 Function(), int Function()>(
      'jimmusic_host_resume',
    );
    _stop = lib.lookupFunction<Int32 Function(), int Function()>(
      'jimmusic_host_stop',
    );
    _seek = lib.lookupFunction<Int32 Function(Double), int Function(double)>(
      'jimmusic_host_seek',
    );
    _position = lib.lookupFunction<Double Function(), double Function()>(
      'jimmusic_host_position',
    );
    _duration = lib.lookupFunction<Double Function(), double Function()>(
      'jimmusic_host_duration',
    );
    _state = lib.lookupFunction<Int32 Function(), int Function()>(
      'jimmusic_host_state',
    );
    _setEventCallback = lib
        .lookupFunction<
          Int32 Function(Pointer<NativeFunction<_EventCallbackNative>>?),
          int Function(Pointer<NativeFunction<_EventCallbackNative>>?)
        >('jimmusic_host_set_event_callback');
    _lastError = lib
        .lookupFunction<
          UintPtr Function(Pointer<Uint8>, UintPtr),
          int Function(Pointer<Uint8>, int)
        >('jimmusic_host_last_error');
    _outputSession = lib
        .lookupFunction<
          UintPtr Function(Pointer<Uint8>, UintPtr),
          int Function(Pointer<Uint8>, int)
        >('jimmusic_host_output_session');
    _startNode = lib
        .lookupFunction<
          Int32 Function(Pointer<Utf8>),
          int Function(Pointer<Utf8>)
        >('jimmusic_node_start');
    _setNodeForeground = lib
        .lookupFunction<Int32 Function(Int32), int Function(int)>(
          'jimmusic_node_set_foreground',
        );
    _connectNode = lib
        .lookupFunction<
          Int32 Function(Pointer<Utf8>),
          int Function(Pointer<Utf8>)
        >('jimmusic_node_connect');
    _stopNode = lib.lookupFunction<Int32 Function(), int Function()>(
      'jimmusic_node_stop',
    );
    _nodeStatus = lib
        .lookupFunction<
          UintPtr Function(Pointer<Uint8>, UintPtr),
          int Function(Pointer<Uint8>, int)
        >('jimmusic_node_status');

    // 注册事件回调（listener 允许从任意线程回调，安全调度到当前 isolate）。
    _eventCallable = NativeCallable.listener(_onNativeEvent);
    _setEventCallback!(_eventCallable!.nativeFunction);
  }

  void _onNativeEvent(int eventType, double value) {
    _events.add(BridgeEvent(eventType, value));
  }

  /// 加载并激活音频输出插件（返回 0 表示成功，或错误码）。
  int? setOutput(String path) {
    if (_lib == null) return null;
    final p = path.toNativeUtf8();
    try {
      final result = _setOutput!(p);
      _outputReady = result == 0;
      return result;
    } finally {
      malloc.free(p);
    }
  }

  /// 设置播放队列（JSON 路径数组）。
  int? setQueue(List<String> paths) {
    if (_lib == null) return null;
    final p = jsonEncode(paths).toNativeUtf8();
    try {
      return _setQueue!(p);
    } finally {
      malloc.free(p);
    }
  }

  /// 播放队列第 index 首。
  int? playTrack(int index) => _lib == null ? null : _playTrack!(index);

  /// 0 ms means gapless. Positive duration enables double-timeline crossfade;
  /// equalPower=false selects a linear curve.
  int? setCrossfade(Duration duration, {bool equalPower = true}) => _lib == null
      ? null
      : _setCrossfade!(duration.inMilliseconds, equalPower ? 1 : 0);

  /// 下一首。
  int? next() => _lib == null ? null : _next!();

  /// 上一首。
  int? previous() => _lib == null ? null : _previous!();

  /// 当前曲目索引。
  int currentIndex() => _lib == null ? 0 : _currentIndex!();

  /// 播放本地音频文件（异步解码，立即返回；0 表示指令已接受）。
  int? playFile(String trackId, String path) {
    if (_lib == null) return null;
    final t = trackId.toNativeUtf8();
    final p = path.toNativeUtf8();
    try {
      return _playFile!(t, p);
    } finally {
      malloc.free(t);
      malloc.free(p);
    }
  }

  /// 暂停（0 表示成功）。
  int? pause() => _lib == null ? null : _pause!();

  /// 恢复播放。
  int? resume() => _lib == null ? null : _resume!();

  /// 停止。
  int? stop() => _lib == null ? null : _stop!();

  /// 跳转到指定位置（秒）。
  int? seek(double positionSecs) => _lib == null ? null : _seek!(positionSecs);

  /// 当前播放位置（秒）。
  double position() => _lib == null ? 0.0 : _position!();

  /// 当前曲目时长（秒）。
  double duration() => _lib == null ? 0.0 : _duration!();

  /// 当前播放状态（0/1/2）。
  int state() => _lib == null ? PlaybackState.stopped : _state!();

  /// 最近一次来自 Core 的结构化错误 JSON；无错误时返回 null。
  String? lastError() {
    if (_lib == null || _lastError == null) return null;
    final length = _lastError!(nullptr, 0);
    if (length <= 0) return null;
    final buffer = malloc<Uint8>(length);
    try {
      final written = _lastError!(buffer, length);
      if (written != length) return null;
      return utf8.decode(buffer.asTypedList(length), allowMalformed: true);
    } finally {
      malloc.free(buffer);
    }
  }

  /// Evidence reported by the currently opened output session, as JSON.
  String? outputSession() {
    if (_lib == null || _outputSession == null) return null;
    final length = _outputSession!(nullptr, 0);
    if (length <= 0) return null;
    final buffer = malloc<Uint8>(length);
    try {
      final written = _outputSession!(buffer, length);
      if (written != length) return null;
      return utf8.decode(buffer.asTypedList(length), allowMalformed: true);
    } finally {
      malloc.free(buffer);
    }
  }

  /// Starts the persistent, app-embedded Rust IPFS node.
  int? startNode(String repositoryPath) {
    if (_lib == null) return null;
    final path = repositoryPath.toNativeUtf8();
    try {
      return _startNode!(path);
    } finally {
      malloc.free(path);
    }
  }

  /// Announces foreground/background lifecycle without promising that a mobile
  /// OS will keep the process alive in the background.
  int? setNodeForeground(bool foreground) =>
      _lib == null ? null : _setNodeForeground!(foreground ? 1 : 0);

  /// Dials a libp2p multiaddress directly through the embedded node.
  int? connectNode(String address) {
    if (_lib == null) return null;
    final value = address.toNativeUtf8();
    try {
      return _connectNode!(value);
    } finally {
      malloc.free(value);
    }
  }

  int? stopNode() => _lib == null ? null : _stopNode!();

  Map<String, dynamic>? nodeStatus() {
    if (_lib == null || _nodeStatus == null) return null;
    final length = _nodeStatus!(nullptr, 0);
    if (length <= 0) return null;
    final buffer = malloc<Uint8>(length);
    try {
      final written = _nodeStatus!(buffer, length);
      if (written != length) return null;
      final value = jsonDecode(
        utf8.decode(buffer.asTypedList(length), allowMalformed: true),
      );
      return value is Map<String, dynamic> ? value : null;
    } finally {
      malloc.free(buffer);
    }
  }

  /// 释放资源。
  void dispose() {
    stopNode();
    _events.close();
  }
}
