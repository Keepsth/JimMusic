import 'dart:convert';

import 'package:flutter/foundation.dart';

import '../models/audio_output.dart';
import '../services/persistence_service.dart';
import '../services/native_library_locator.dart';
import '../services/rust_bridge.dart';

/// 音频输出后端选择提供者。
///
/// 前端维护「输出设备」选择：当前激活项持久化到本地；桌面端通过 Rust FFI
/// 实际加载随包输出插件。Web 端使用 `just_audio` 的浏览器输出；仓库中的
/// AudioWorklet/SharedArrayBuffer 桥尚未接入播放链路，因此不会显示成可选设备。
class AudioOutputProvider extends ChangeNotifier {
  String _activeId = 'auto';
  String? _error;
  Map<String, dynamic>? _session;

  /// 当前激活的输出后端 id。
  String get activeId => _activeId;
  String? get error => _error;
  Map<String, dynamic>? get session => _session;

  /// 可用输出后端（渲染选择界面）。
  List<AudioOutputDevice> get devices {
    if (kIsWeb) {
      return AudioOutputDevice.backends
          .where((device) => device.id == 'auto')
          .toList(growable: false);
    }
    return AudioOutputDevice.backends
        .where((device) {
          if (device.id == 'web-audio') return false;
          return true;
        })
        .toList(growable: false);
  }

  /// 当前激活的后端（回退到「自动」）。
  AudioOutputDevice get active =>
      AudioOutputDevice.byId(_activeId) ?? devices.first;

  /// 加载持久化的激活后端。
  Future<void> load() async {
    _activeId = await PersistenceService.loadActiveOutput();
    if (devices.every((device) => device.id != _activeId)) {
      _activeId = 'auto';
      await PersistenceService.saveActiveOutput(_activeId);
    }
    if (!kIsWeb && RustBridge.instance.available) {
      final effective = _activeId == 'auto' ? 'system' : _activeId;
      _loadNativeOutput(effective);
    }
    notifyListeners();
  }

  /// 切换激活的输出后端。
  Future<void> activate(String id) async {
    if (devices.every((device) => device.id != id)) return;
    if (_activeId == id) return;
    _error = null;
    final bridge = RustBridge.instance;
    if (!kIsWeb && !bridge.available) {
      _error = 'Rust Core 未加载，无法切换到 $id 输出';
      notifyListeners();
      return;
    }
    if (!kIsWeb && !_loadNativeOutput(id == 'auto' ? 'system' : id)) return;
    _activeId = id;
    notifyListeners();
    await PersistenceService.saveActiveOutput(id);
  }

  String _libraryName(String id) {
    final base = id == 'null'
        ? 'null_output'
        : id == 'system'
        ? 'alsa_output'
        : '${id.replaceAll('-', '_')}_output';
    switch (defaultTargetPlatform) {
      case TargetPlatform.windows:
        return resolveBundledLibrary('$base.dll');
      case TargetPlatform.macOS:
        return resolveBundledLibrary('lib$base.dylib');
      default:
        return resolveBundledLibrary('lib$base.so');
    }
  }

  bool _loadNativeOutput(String id) {
    final bridge = RustBridge.instance;
    final path = _libraryName(id);
    final code = bridge.setOutput(path);
    if (code == 0) {
      final raw = bridge.outputSession();
      try {
        _session = raw == null ? null : jsonDecode(raw) as Map<String, dynamic>;
      } catch (_) {
        _session = null;
        _error = '输出已打开，但会话证据无法解析';
        return false;
      }
      return true;
    }
    _session = null;
    _error = bridge.lastError() ?? '输出插件加载失败（$path，错误码 $code）';
    notifyListeners();
    return false;
  }
}
