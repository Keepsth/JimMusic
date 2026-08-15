import 'dart:convert';

import 'package:crypto/crypto.dart';

import '../models/music.dart';
import '../providers/music_player_provider.dart';
import 'control_api.dart';

/// 与后端 `LibraryService::stable_id` 完全一致的稳定 ID：
/// `jm_` + SHA256(domain \0 value) 的前 24 个 hex 字符（PLR-001/PLR-002）。
String backendStableId(String domain, List<int> value) {
  final digest = sha256.convert([...utf8.encode(domain), 0, ...value]);
  return 'jm_${digest.toString().substring(0, 24)}';
}

/// 本地文件曲目在后端的 track_id（与 `import_local` 派生规则一致）。
String backendLocalTrackId(String path) =>
    backendStableId('local-track', utf8.encode(path));

/// 曲目在后端的 ID：拉取曲目直接用自身 ID，本地文件按路径派生。
String backendTrackIdFor(Music music) {
  if (music.filePath != null && music.filePath!.isNotEmpty) {
    return backendLocalTrackId(music.filePath!);
  }
  return music.id;
}

/// 后端 LibraryTrackV1 JSON → Flutter Music（UI-002/PLR-007 统一模型）。
Music? musicFromLibraryTrack(Map<String, dynamic> track) {
  final trackId = '${track['track_id'] ?? ''}';
  if (trackId.isEmpty) return null;
  final sources = (track['sources'] as List<dynamic>? ?? const [])
      .whereType<Map<String, dynamic>>()
      .toList();
  final primary = sources.isEmpty ? null : sources.first;

  TrackSourceType sourceType;
  String? filePath;
  switch (primary?['kind']) {
    case 'local_file':
      sourceType = TrackSourceType.localFile;
      filePath = '${primary?['uri'] ?? ''}';
    case 'ipfs':
      sourceType = TrackSourceType.ipfs;
    case 'community':
      sourceType = TrackSourceType.community;
    case 'cached_object':
      sourceType = TrackSourceType.cached;
    default:
      sourceType = TrackSourceType.ipfs;
  }

  TrackAvailability availability;
  String? reason;
  switch (primary?['availability']) {
    case 'available':
      availability = TrackAvailability.available;
    case 'missing':
      availability = TrackAvailability.missing;
      reason = '${primary?['unavailable_reason'] ?? '文件缺失'}';
    case 'offline':
      availability = TrackAvailability.remote;
      reason = '离线内容（网络不可用）';
    case 'requires_decoder':
      availability = TrackAvailability.unsupported;
      reason = '需要解码插件：${primary?['unavailable_reason'] ?? '未知格式'}';
    case 'integrity_failed':
      availability = TrackAvailability.unsupported;
      reason = '完整性校验失败';
    default:
      availability = TrackAvailability.remote;
  }

  final artists = (track['artists'] as List<dynamic>? ?? const [])
      .whereType<String>()
      .toList();
  final durationMs = (track['duration_ms'] as num?)?.toInt();
  // COM-006：后端统一标注的社区策略决策。
  final policy = track['policy'] as Map<String, dynamic>?;
  return Music(
    id: trackId,
    title: '${track['title'] ?? trackId}',
    artist: artists.join(', '),
    album: '${track['album'] ?? ''}',
    duration: _formatMs(durationMs),
    filePath: filePath != null && filePath.isNotEmpty ? filePath : null,
    sourceType: sourceType,
    availability: availability,
    unavailableReason: reason,
    manifestCid: track['manifest_cid'] as String?,
    renditionCid: primary?['content_cid'] as String?,
    codec: primary?['codec'] as String?,
    publisher: track['publisher'] as String?,
    sampleRate: (primary?['sample_rate'] as num?)?.toInt(),
    bitDepth: (primary?['bit_depth'] as num?)?.toInt(),
    channels: (primary?['channels'] as num?)?.toInt(),
    policyAction: policy?['action'] as String?,
    policyReason: policy?['reason'] as String?,
    policySourceIds: (policy?['source_ids'] as List<dynamic>? ?? const [])
        .whereType<String>()
        .toList(growable: false),
    isFavorite: track['favorite'] == true,
  );
}

/// 本地文件曲目 → 后端 `Track` DTO（PLR-001 推送本地曲库）。
Map<String, dynamic> localTrackJsonForBackend(Music music) => {
  'path': music.filePath,
  'title': music.title,
  'artist': music.artist.isEmpty ? null : music.artist,
  'album': music.album.isEmpty ? null : music.album,
  'duration': null,
  'sample_rate': music.sampleRate,
  'channels': music.channels,
};

String _formatMs(int? milliseconds) {
  if (milliseconds == null || milliseconds <= 0) return '';
  final total = Duration(milliseconds: milliseconds);
  final minutes = total.inMinutes;
  final seconds = total.inSeconds % 60;
  return '$minutes:${seconds.toString().padLeft(2, '0')}';
}

/// 一次曲库同步的结构化报告（UI-010：可解释的结果，不吞异常）。
class LibrarySyncReport {
  int pushedLocal = 0;
  int pulledRemote = 0;
  int favoritesUp = 0;
  int favoritesDown = 0;
  int playlistsUp = 0;
  int playlistsDown = 0;
  bool sessionPushed = false;
  bool sessionPulled = false;
  final List<String> errors = <String>[];

  @override
  String toString() =>
      '推送本地 $pushedLocal · 拉取远端 $pulledRemote · '
      '收藏 ↑$favoritesUp ↓$favoritesDown · 歌单 ↑$playlistsUp ↓$playlistsDown · '
      '会话 ${sessionPushed ? '已推送' : ''}${sessionPulled ? '已恢复' : ''}'
      '${errors.isEmpty ? '' : ' · 错误 ${errors.length}'}';
}

/// 曲库统一同步（PLR-001/PLR-002/PLR-009/UI-002）：
/// 本地优先、双向同步后端 LibraryService。
///
/// 1. 推送本地文件曲目（幂等：后端按路径派生稳定 track_id）；
/// 2. 拉取后端曲库（Manifest/社区/本地）合并进 Flutter 曲库；
/// 3. 收藏双向合并（后端 favorite → 本地；本地收藏的拉取曲目 → 后端）；
/// 4. 命名歌单双向合并（本地 → 后端按名字创建/更新；后端 → 本地）；
/// 5. 会话：本地有活动会话则推送（ID 换算）；否则从后端恢复（绝不自动播放）。
class LibrarySyncService {
  Future<LibrarySyncReport> sync(
    ControlApi api,
    MusicPlayerProvider player,
  ) async {
    final report = LibrarySyncReport();

    // 1) 推送本地文件曲目。
    for (final music in player.library.where(
      (m) =>
          m.filePath != null &&
          m.filePath!.isNotEmpty &&
          m.sourceType == TrackSourceType.localFile,
    )) {
      final trackId = backendLocalTrackId(music.filePath!);
      try {
        await api.post(
          '/library/tracks/import-local',
          {
            'request_id': 'sync-local-$trackId',
            'track': localTrackJsonForBackend(music),
          },
          {'idempotency-key': 'sync-local-$trackId'},
        );
        report.pushedLocal++;
      } catch (error) {
        report.errors.add('推送 ${music.title}: $error');
      }
    }

    // 2) 拉取后端曲库并合并。
    try {
      final remote = await api.get('/library/tracks');
      final pulled = (remote is List<dynamic> ? remote : const <dynamic>[])
          .whereType<Map<String, dynamic>>()
          .map(musicFromLibraryTrack)
          .whereType<Music>()
          .toList();
      await player.mergeLibraryTracks(pulled);
      report.pulledRemote = pulled.length;

      // 3) 收藏双向合并。
      for (final music in pulled) {
        final favorite = music.isFavorite;
        if (favorite && !player.isFavorite(music)) {
          await player.toggleFavorite(music);
          report.favoritesDown++;
        } else if (!favorite && player.isFavorite(music)) {
          try {
            await api.put(
              '/library/tracks/${Uri.encodeComponent(music.id)}/favorite',
              {'request_id': 'sync-fav-${music.id}', 'favorite': true},
            );
            report.favoritesUp++;
          } catch (error) {
            report.errors.add('收藏 ${music.title}: $error');
          }
        }
      }

      // 4) 歌单双向合并。
      report.playlistsDown = await _pullPlaylists(api, player, report);
      report.playlistsUp = await _pushPlaylists(api, player, report);

      // 5) 会话：本地活动会话优先推送，否则从后端恢复（不自动播放）。
      final current = player.currentMusic;
      if (current != null) {
        await _pushSession(api, player, report);
      } else {
        await _pullSession(api, player, report);
      }
    } catch (error) {
      report.errors.add('拉取曲库: $error');
    }
    return report;
  }

  Future<int> _pullPlaylists(
    ControlApi api,
    MusicPlayerProvider player,
    LibrarySyncReport report,
  ) async {
    var count = 0;
    final remote = await api.get('/library/playlists');
    for (final entry in remote is List<dynamic> ? remote : const <dynamic>[]) {
      if (entry is! Map<String, dynamic>) continue;
      final name = '${entry['name'] ?? ''}'.trim();
      if (name.isEmpty) continue;
      final trackIds = (entry['track_ids'] as List<dynamic>? ?? const [])
          .whereType<String>()
          .toList();
      // 曲库中不存在的 ID 会被解析阶段自然丢弃。
      await player.syncNamedPlaylist(name, trackIds);
      count++;
    }
    return count;
  }

  Future<int> _pushPlaylists(
    ControlApi api,
    MusicPlayerProvider player,
    LibrarySyncReport report,
  ) async {
    var count = 0;
    for (final entry in player.playlists.entries) {
      final name = entry.key.trim();
      if (name.isEmpty) continue;
      try {
        final created = await api.post(
          '/library/playlists',
          {'request_id': 'sync-pl-$name', 'name': name},
          {'idempotency-key': 'sync-pl-$name'},
        );
        final playlistId = created is Map<String, dynamic>
            ? '${created['playlist_id'] ?? ''}'
            : '';
        if (playlistId.isEmpty) continue;
        final trackIds = player
            .tracksInPlaylist(name)
            .map(backendTrackIdFor)
            .toList();
        await api.patch(
          '/library/playlists/${Uri.encodeComponent(playlistId)}',
          {
            'request_id': 'sync-pl-update-$name',
            'name': name,
            'track_ids': trackIds,
          },
        );
        count++;
      } catch (error) {
        report.errors.add('歌单 $name: $error');
      }
    }
    return count;
  }

  Future<void> _pushSession(
    ControlApi api,
    MusicPlayerProvider player,
    LibrarySyncReport report,
  ) async {
    try {
      await api.put('/library/session', {
        'current_track_id': player.currentMusic == null
            ? null
            : backendTrackIdFor(player.currentMusic!),
        'queue': player.playlist.map(backendTrackIdFor).toList(),
        'position_seconds': player.currentPosition,
        'selected_audio_path': null,
        'volume': player.volume,
        'muted': player.muted,
        'auto_play': false,
      });
      report.sessionPushed = true;
    } catch (error) {
      report.errors.add('推送会话: $error');
    }
  }

  Future<void> _pullSession(
    ControlApi api,
    MusicPlayerProvider player,
    LibrarySyncReport report,
  ) async {
    try {
      final session = await api.get('/library/session');
      if (session is! Map<String, dynamic>) return;
      final currentId = session['current_track_id'] as String?;
      final queue = (session['queue'] as List<dynamic>? ?? const [])
          .whereType<String>()
          .toList();
      if (currentId == null && queue.isEmpty) return;
      final applied = await player.applySessionSnapshot(
        queueIds: queue,
        currentTrackId: currentId,
        position: ((session['position_seconds'] as num?) ?? 0).toDouble(),
      );
      report.sessionPulled = applied;
    } catch (error) {
      report.errors.add('恢复会话: $error');
    }
  }
}
