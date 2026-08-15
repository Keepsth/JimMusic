import 'dart:convert';
import 'dart:io';
import 'dart:math' as math;
import 'dart:typed_data';

import 'package:flutter_test/flutter_test.dart';
import 'package:flutter_app/services/rust_bridge.dart';

/// 定位 Rust 构建产物（flutter_app 与 backend 为同级目录）。
String? _findLib(String name) {
  for (final c in [
    '../backend/target/debug/$name',
    '../backend/target/release/$name',
    'backend/target/debug/$name',
  ]) {
    if (File(c).existsSync()) return c;
  }
  return null;
}

/// 生成一个 1 秒、8kHz、单声道 WAV。
Uint8List _wavBytes(int n) {
  const sr = 8000;
  final dataLen = n * 2;
  final out = BytesBuilder();
  out.add(ascii.encode('RIFF'));
  out.add(_u32le(36 + dataLen));
  out.add(ascii.encode('WAVE'));
  out.add(ascii.encode('fmt '));
  out.add(_u32le(16));
  out.add(_u16le(1)); // PCM
  out.add(_u16le(1)); // mono
  out.add(_u32le(sr));
  out.add(_u32le(sr * 2)); // byte rate
  out.add(_u16le(2)); // block align
  out.add(_u16le(16)); // bits per sample
  out.add(ascii.encode('data'));
  out.add(_u32le(dataLen));
  for (var i = 0; i < n; i++) {
    final s = (math.sin(i.toDouble()) * 1000.0).toInt();
    out.add(_u16le(s & 0xffff));
  }
  return out.toBytes();
}

Uint8List _u32le(int v) =>
    Uint8List(4)..buffer.asByteData().setUint32(0, v, Endian.little);
Uint8List _u16le(int v) =>
    Uint8List(2)..buffer.asByteData().setUint16(0, v, Endian.little);

void main() {
  test('Rust 桥加载并通过 null-output 走通播放链路', () async {
    final coreLib = _findLib('libapp_core.so');
    final outLib = _findLib('libnull_output.so');
    if (coreLib == null || outLib == null) {
      // ignore: avoid_print
      print('Rust cdylibs 未构建，跳过 FFI 桥测试');
      return;
    }

    final bridge = RustBridge.openWith(coreLib);
    expect(bridge.available, isTrue, reason: '应能加载 $coreLib');

    // 激活 null-output 输出插件。
    final outCode = bridge.setOutput(outLib);
    expect(outCode, 0, reason: 'set_output 应返回 0');
    final outputSession = jsonDecode(bridge.outputSession()!);
    expect(outputSession['capability_source'], 'opened_null_session');
    expect(outputSession['negotiated_format']['sample_rate'], 44100);
    expect(outputSession['software_buffer_frames'], 1024);

    // 同一桥内启动应用内 Rust IPFS 节点，并验证生命周期状态不是静态平台字符串。
    final nodeDirectory = Directory.systemTemp.createTempSync(
      'jimmusic_node_ffi_',
    );
    expect(bridge.startNode(nodeDirectory.path), 0);
    final nodeStatus = bridge.nodeStatus()!;
    expect(nodeStatus['implementation'], 'rust-ipfs');
    expect(nodeStatus['lifecycle_state'], 'foreground');
    expect(nodeStatus['peer_id'], isNotEmpty);
    expect(nodeStatus['persists_after_app_close'], isFalse);
    expect(bridge.setNodeForeground(false), 0);
    expect(bridge.nodeStatus()!['lifecycle_state'], 'background_degraded');

    // 生成 WAV 并订阅事件。
    final wav = File('${Directory.systemTemp.path}/jimmusic_ffi_test.wav');
    wav.writeAsBytesSync(_wavBytes(8000));

    final events = <BridgeEvent>[];
    final sub = bridge.events.listen(events.add);

    final playCode = bridge.playFile('ffi-test', wav.path);
    expect(playCode, 0, reason: 'play_file 应返回 0');

    // 等待停止事件（播放完成）。
    final deadline = DateTime.now().add(const Duration(seconds: 10));
    while (DateTime.now().isBefore(deadline)) {
      if (events.any((e) => e.isStopped)) break;
      await Future<void>.delayed(const Duration(milliseconds: 20));
    }

    expect(events.any((e) => e.isPlaying), isTrue, reason: '应收到 playing 事件');
    expect(events.any((e) => e.isProgress), isTrue, reason: '应收到 progress 事件');
    expect(events.any((e) => e.isStopped), isTrue, reason: '应收到 stopped 事件');

    // Player 单曲队列会自动循环，显式停止后断言状态复位。
    bridge.stop();
    await Future<void>.delayed(const Duration(milliseconds: 100));
    expect(bridge.state(), PlaybackState.stopped, reason: '最终状态应为停止');

    await sub.cancel();
    expect(bridge.stopNode(), 0);
    nodeDirectory.deleteSync(recursive: true);
    wav.deleteSync();
  });
}
