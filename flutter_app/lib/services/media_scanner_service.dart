import 'package:file_picker/file_picker.dart';
import 'package:flutter/foundation.dart' show kIsWeb;

import '../models/music.dart';

/// 媒体库扫描服务：从本地文件选取音频文件并构造曲目。
///
/// 跨平台处理：
/// - 桌面/移动端：`path` 为真实文件路径 → [`Music.fromFilePath`]；
/// - Web 端：浏览器不暴露文件路径，且**访问 `file.path` 会直接抛异常**，因此
///   必须以 `kIsWeb` 分流，只读 `bytes` → [`Music.fromBytes`]（播放时用 data URI）。
class MediaScannerService {
  /// 打开文件选择器，选取音频文件并返回曲目列表。
  ///
  /// 支持多选（MP3/AAC/FLAC/WAV/OGG/M4A）。
  /// 用户取消时返回空列表。
  static Future<List<Music>> pickAudioFiles() async {
    final result = await FilePicker.platform.pickFiles(
      type: FileType.custom,
      allowedExtensions: ['mp3', 'aac', 'flac', 'wav', 'ogg', 'm4a'],
      allowMultiple: true,
      // Web 端需要字节内容（无文件路径）；桌面/移动端一并返回也无妨。
      withData: true,
    );
    if (result == null) return [];

    final tracks = <Music>[];
    for (var i = 0; i < result.files.length; i++) {
      final file = result.files[i];
      if (kIsWeb) {
        // Web：绝不访问 file.path（会抛异常），只用 bytes + name。
        final bytes = file.bytes;
        if (bytes != null && bytes.isNotEmpty) {
          tracks.add(
            Music.fromBytes(
              name: file.name,
              bytes: bytes,
              mimeType: _mimeFor(file.name),
              index: i,
            ),
          );
        }
      } else {
        final path = file.path;
        if (path != null && path.isNotEmpty) {
          tracks.add(Music.fromFilePath(path, index: i));
        }
      }
    }
    return tracks;
  }

  /// 根据文件扩展名推断 MIME 类型（用于 Web 端 data URI）。
  static String _mimeFor(String name) {
    final dot = name.lastIndexOf('.');
    final ext = dot >= 0 ? name.substring(dot + 1).toLowerCase() : '';
    switch (ext) {
      case 'mp3':
        return 'audio/mpeg';
      case 'aac':
        return 'audio/aac';
      case 'flac':
        return 'audio/flac';
      case 'wav':
        return 'audio/wav';
      case 'ogg':
        return 'audio/ogg';
      case 'm4a':
        return 'audio/mp4';
      default:
        return 'audio/mpeg';
    }
  }
}
