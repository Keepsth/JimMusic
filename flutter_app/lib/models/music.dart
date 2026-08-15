import 'dart:typed_data';

enum TrackSourceType { localFile, localMemory, cached, ipfs, community }

enum TrackAvailability { available, missing, remote, blocked, unsupported }

/// 音乐曲目模型。
class Music {
  final String id;
  final String title;
  final String artist;
  final String album;
  final String duration;
  final String? albumArt;
  final String? filePath;
  final TrackSourceType sourceType;
  final TrackAvailability availability;
  final String? unavailableReason;
  final String? manifestCid;
  final String? renditionCid;
  final String? codec;
  final int? sampleRate;
  final int? bitDepth;
  final int? channels;

  /// 发布者身份 CID（Manifest/社区曲目，COM-005 全文索引）。
  final String? publisher;
  final List<String> communitySourceIds;

  /// 内存中的音频数据（Web 端导入时无文件路径，直接持有字节供播放）。
  final Uint8List? audioBytes;

  /// 音频 MIME 类型（如 `audio/mpeg`），Web 端构造 data URI 时使用。
  final String? mimeType;

  /// 是否已收藏。
  final bool isFavorite;

  /// 同步歌词（LRC 文本，可选）。
  final String? lyrics;

  Music({
    required this.id,
    required this.title,
    required this.artist,
    required this.album,
    required this.duration,
    this.albumArt,
    this.filePath,
    this.sourceType = TrackSourceType.localFile,
    this.availability = TrackAvailability.available,
    this.unavailableReason,
    this.manifestCid,
    this.renditionCid,
    this.codec,
    this.sampleRate,
    this.bitDepth,
    this.channels,
    this.publisher,
    this.communitySourceIds = const [],
    this.audioBytes,
    this.mimeType,
    this.isFavorite = false,
    this.lyrics,
  });

  /// 返回带更新字段的副本（用于切换收藏等不可变更新）。
  Music copyWith({
    String? id,
    String? title,
    String? artist,
    String? album,
    String? duration,
    String? albumArt,
    String? filePath,
    TrackSourceType? sourceType,
    TrackAvailability? availability,
    String? unavailableReason,
    String? manifestCid,
    String? renditionCid,
    String? codec,
    int? sampleRate,
    int? bitDepth,
    int? channels,
    String? publisher,
    List<String>? communitySourceIds,
    Uint8List? audioBytes,
    String? mimeType,
    bool? isFavorite,
    String? lyrics,
  }) {
    return Music(
      id: id ?? this.id,
      title: title ?? this.title,
      artist: artist ?? this.artist,
      album: album ?? this.album,
      duration: duration ?? this.duration,
      albumArt: albumArt ?? this.albumArt,
      filePath: filePath ?? this.filePath,
      sourceType: sourceType ?? this.sourceType,
      availability: availability ?? this.availability,
      unavailableReason: unavailableReason ?? this.unavailableReason,
      manifestCid: manifestCid ?? this.manifestCid,
      renditionCid: renditionCid ?? this.renditionCid,
      codec: codec ?? this.codec,
      sampleRate: sampleRate ?? this.sampleRate,
      bitDepth: bitDepth ?? this.bitDepth,
      channels: channels ?? this.channels,
      publisher: publisher ?? this.publisher,
      communitySourceIds: communitySourceIds ?? this.communitySourceIds,
      audioBytes: audioBytes ?? this.audioBytes,
      mimeType: mimeType ?? this.mimeType,
      isFavorite: isFavorite ?? this.isFavorite,
      lyrics: lyrics ?? this.lyrics,
    );
  }

  /// 序列化为可持久化的 Map。
  Map<String, dynamic> toMap() => {
    'id': id,
    'title': title,
    'artist': artist,
    'album': album,
    'duration': duration,
    'albumArt': albumArt,
    'filePath': filePath,
    'sourceType': sourceType.name,
    'availability': availability.name,
    'unavailableReason': unavailableReason,
    'manifestCid': manifestCid,
    'renditionCid': renditionCid,
    'codec': codec,
    'sampleRate': sampleRate,
    'bitDepth': bitDepth,
    'channels': channels,
    'communitySourceIds': communitySourceIds,
    'isFavorite': isFavorite,
    'lyrics': lyrics,
  };

  /// 从 Map 反序列化。
  factory Music.fromMap(Map<String, dynamic> map) {
    final sourceType = _enumByName(
      TrackSourceType.values,
      map['sourceType'] as String?,
      TrackSourceType.localFile,
    );
    var availability = _enumByName(
      TrackAvailability.values,
      map['availability'] as String?,
      TrackAvailability.available,
    );
    // 浏览器内存文件不能跨重启持久化字节，恢复时诚实标记为缺失。
    if (sourceType == TrackSourceType.localMemory) {
      availability = TrackAvailability.missing;
    }
    return Music(
      id: map['id'] as String,
      title: map['title'] as String,
      artist: map['artist'] as String? ?? '',
      album: map['album'] as String? ?? '',
      duration: map['duration'] as String? ?? '0:00',
      albumArt: map['albumArt'] as String?,
      filePath: map['filePath'] as String?,
      sourceType: sourceType,
      availability: availability,
      unavailableReason: sourceType == TrackSourceType.localMemory
          ? '浏览器会话已结束，请重新导入文件'
          : map['unavailableReason'] as String?,
      manifestCid: map['manifestCid'] as String?,
      renditionCid: map['renditionCid'] as String?,
      codec: map['codec'] as String?,
      sampleRate: map['sampleRate'] as int?,
      bitDepth: map['bitDepth'] as int?,
      channels: map['channels'] as int?,
      communitySourceIds:
          (map['communitySourceIds'] as List<dynamic>? ?? const [])
              .whereType<String>()
              .toList(growable: false),
      isFavorite: map['isFavorite'] as bool? ?? false,
      lyrics: map['lyrics'] as String?,
    );
  }

  /// 从文件路径派生一个曲目（用于本地扫描导入）。
  factory Music.fromFilePath(String path, {int index = 0}) {
    final name = path.split('/').last.split('\\').last;
    final dot = name.lastIndexOf('.');
    final stem = dot > 0 ? name.substring(0, dot) : name;
    // 尝试拆分 "艺术家 - 标题" 格式。
    String title = stem;
    String artist = '未知艺术家';
    final sep = stem.indexOf(' - ');
    if (sep > 0) {
      artist = stem.substring(0, sep);
      title = stem.substring(sep + 3);
    }
    return Music(
      id: 'local_${_stableId(path)}',
      title: title,
      artist: artist,
      album: '',
      duration: '--:--',
      filePath: path,
      sourceType: TrackSourceType.localFile,
      albumArt: null,
    );
  }

  /// 从内存字节构造曲目（Web 端导入：浏览器不暴露文件路径，直接持有字节）。
  factory Music.fromBytes({
    required String name,
    required Uint8List bytes,
    String? mimeType,
    int index = 0,
  }) {
    // 去掉扩展名得 stem，再尝试拆分 "艺术家 - 标题" 格式。
    final dot = name.lastIndexOf('.');
    final stem = dot > 0 ? name.substring(0, dot) : name;
    String title = stem;
    String artist = '未知艺术家';
    final sep = stem.indexOf(' - ');
    if (sep > 0) {
      artist = stem.substring(0, sep);
      title = stem.substring(sep + 3);
    }
    return Music(
      id: 'memory_${_stableId('$name:${bytes.length}:$index')}',
      title: title,
      artist: artist,
      album: '',
      duration: '--:--',
      filePath: null,
      audioBytes: bytes,
      mimeType: mimeType,
      sourceType: TrackSourceType.localMemory,
      albumArt: null,
    );
  }
}

T _enumByName<T extends Enum>(List<T> values, String? name, T fallback) {
  for (final value in values) {
    if (value.name == name) return value;
  }
  return fallback;
}

String _stableId(String value) {
  // 32-bit DJB-style hash: all intermediate values remain exactly representable
  // by JavaScript, so native Dart and dart2js derive the same persisted ID.
  var hash = 5381;
  for (final byte in value.codeUnits) {
    hash = ((hash << 5) - hash + byte) & 0xffffffff;
  }
  return hash.toRadixString(16);
}
