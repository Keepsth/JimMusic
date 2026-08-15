import 'dart:convert';

import 'package:flutter_app/models/music.dart';
import 'package:flutter_app/providers/music_player_provider.dart';
import 'package:flutter_app/services/control_api.dart';
import 'package:flutter_app/services/library_sync_service.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';

/// 曲库同步测试用的假控制面：GET 走 [gets]，mutation 记录到 [mutations]。
class _SyncFakeApi extends ControlApi {
  _SyncFakeApi() : super(endpoint: 'http://127.0.0.1:9/v1', token: 'test');

  final Map<String, dynamic> gets = {};
  final List<String> mutations = [];

  @override
  Future<dynamic> get(String path) async => gets[path] ?? const <dynamic>[];

  @override
  Future<dynamic> post(
    String path, [
    Object? body,
    Map<String, String>? headers,
  ]) async {
    mutations.add('POST $path ${jsonEncode(body)}');
    if (path == '/library/playlists') {
      return {
        'playlist_id': 'pl-1',
        'name': (body as Map<String, dynamic>)['name'],
        'track_ids': const <String>[],
      };
    }
    return {};
  }

  @override
  Future<dynamic> put(String path, Object? body) async {
    mutations.add('PUT $path ${jsonEncode(body)}');
    return {};
  }

  @override
  Future<dynamic> patch(String path, Object? body) async {
    mutations.add('PATCH $path ${jsonEncode(body)}');
    return {};
  }

  @override
  Future<dynamic> delete(String path) async {
    mutations.add('DELETE $path');
    return {};
  }
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  setUp(() {
    SharedPreferences.setMockInitialValues({});
  });

  group('backendStableId', () {
    test('与后端 stable_id 派生规则一致（跨语言黄金向量）', () {
      // 后端: jm_ + sha256("local-track\0/tmp/local.mp3") 前 24 hex。
      expect(
        backendStableId('local-track', utf8.encode('/tmp/local.mp3')),
        'jm_f076dbb1c7ceb56bf6172c2f',
      );
      expect(
        backendLocalTrackId('/tmp/local.mp3'),
        'jm_f076dbb1c7ceb56bf6172c2f',
      );
    });
  });

  group('musicFromLibraryTrack', () {
    test('Manifest 曲目映射来源、CID 与收藏', () {
      final music = musicFromLibraryTrack({
        'track_id': 'jm_remote1',
        'title': 'Remote Track',
        'artists': ['A', 'B'],
        'album': 'Album',
        'duration_ms': 123000,
        'favorite': true,
        'manifest_cid': 'bafym',
        'sources': [
          {
            'kind': 'ipfs',
            'uri': 'ipfs://x',
            'content_cid': 'bafyr',
            'container': 'flac',
            'codec': 'flac',
            'availability': 'offline',
          },
        ],
      })!;
      expect(music.id, 'jm_remote1');
      expect(music.artist, 'A, B');
      expect(music.duration, '2:03');
      expect(music.sourceType, TrackSourceType.ipfs);
      expect(music.renditionCid, 'bafyr');
      expect(music.availability, TrackAvailability.remote);
      expect(music.isFavorite, isTrue);
    });

    test('发布者身份 CID 随曲目映射（COM-005）', () {
      final music = musicFromLibraryTrack({
        'track_id': 'jm_pub',
        'title': 'T',
        'artists': const [],
        'album': '',
        'publisher': 'bafypublisher',
        'sources': [
          {
            'kind': 'ipfs',
            'uri': 'ipfs://x',
            'content_cid': 'bafyr',
            'container': 'flac',
            'codec': 'flac',
            'availability': 'offline',
          },
        ],
      })!;
      expect(music.publisher, 'bafypublisher');
    });

    test('社区策略决策随曲目映射（COM-006）', () {
      final music = musicFromLibraryTrack({
        'track_id': 'jm_policy',
        'title': 'T',
        'artists': const [],
        'album': '',
        'manifest_cid': 'bafymanifest',
        'policy': {
          'target': 'bafymanifest',
          'action': 'warn',
          'reason': '社区标记',
          'source_ids': ['src.example'],
          'expires_at': null,
          'locally_overridden': false,
        },
        'sources': [
          {
            'kind': 'ipfs',
            'uri': 'ipfs://x',
            'content_cid': 'bafyr',
            'container': 'flac',
            'codec': 'flac',
            'availability': 'offline',
          },
        ],
      })!;
      expect(music.policyAction, 'warn');
      expect(music.policyReason, '社区标记');
      expect(music.policySourceIds, ['src.example']);
    });

    test('本地文件源保留路径且可用', () {
      final music = musicFromLibraryTrack({
        'track_id': 'jm_local',
        'title': 'Local',
        'artists': ['L'],
        'album': '',
        'sources': [
          {
            'kind': 'local_file',
            'uri': '/tmp/a.mp3',
            'container': 'mp3',
            'codec': 'mp3',
            'availability': 'available',
          },
        ],
      })!;
      expect(music.filePath, '/tmp/a.mp3');
      expect(music.sourceType, TrackSourceType.localFile);
      expect(music.availability, TrackAvailability.available);
    });

    test('缺失文件标记不可用并带原因', () {
      final music = musicFromLibraryTrack({
        'track_id': 'jm_gone',
        'title': 'Gone',
        'artists': const [],
        'album': '',
        'sources': [
          {
            'kind': 'local_file',
            'uri': '/tmp/gone.mp3',
            'container': 'mp3',
            'codec': 'mp3',
            'availability': 'missing',
            'unavailable_reason': 'file was deleted',
          },
        ],
      })!;
      expect(music.availability, TrackAvailability.missing);
      expect(music.unavailableReason, contains('deleted'));
    });
  });

  group('LibrarySyncService.sync', () {
    test('推送本地、拉取远端、收藏与会话恢复（绝不自动播放）', () async {
      final api = _SyncFakeApi();
      api.gets['/library/tracks'] = [
        {
          'track_id': 'jm_remote1',
          'title': 'Remote Track',
          'artists': ['Remote Artist'],
          'album': 'RA',
          'duration_ms': 123000,
          'favorite': true,
          'manifest_cid': 'bafym',
          'sources': [
            {
              'kind': 'ipfs',
              'uri': 'ipfs://x',
              'content_cid': 'bafyr',
              'container': 'flac',
              'codec': 'flac',
              'availability': 'offline',
            },
          ],
        },
      ];
      api.gets['/library/playlists'] = [
        {
          'playlist_id': 'pl-r',
          'name': '远端歌单',
          'track_ids': ['jm_remote1'],
        },
      ];
      api.gets['/library/session'] = {
        'current_track_id': 'jm_remote1',
        'queue': ['jm_remote1'],
        'position_seconds': 42.0,
        'volume': 1.0,
        'muted': false,
        'auto_play': false,
      };

      final player = MusicPlayerProvider();
      addTearDown(player.dispose);
      await player.ready;
      await player.mergeLibraryTracks([
        Music(
          id: 'local-1',
          title: 'Local',
          artist: 'L',
          album: '',
          duration: '',
          filePath: '/tmp/local.mp3',
          sourceType: TrackSourceType.localFile,
          availability: TrackAvailability.available,
        ),
      ]);

      final report = await LibrarySyncService().sync(api, player);
      expect(report.pushedLocal, 1);
      expect(report.pulledRemote, 1);
      expect(report.favoritesDown, 1);
      expect(report.playlistsDown, 1);
      expect(report.sessionPulled, isTrue);

      expect(player.library.any((m) => m.id == 'jm_remote1'), isTrue);
      expect(player.currentMusic?.id, 'jm_remote1');
      expect(player.currentPosition, 42.0);
      expect(player.playerState, PlayerState.stopped);
      expect(player.playlists.containsKey('远端歌单'), isTrue);
      expect(
        player.isFavorite(
          player.library.firstWhere((m) => m.id == 'jm_remote1'),
        ),
        isTrue,
      );

      final mutations = api.mutations.join('\n');
      expect(mutations, contains('POST /library/tracks/import-local'));
      expect(mutations, contains('/tmp/local.mp3'));
      expect(mutations, contains('jm_f076dbb1c7ceb56bf6172c2f'));
      expect(report.errors, isEmpty);
    });

    test('本地歌单推送到后端并使用后端稳定 ID', () async {
      final api = _SyncFakeApi();
      final player = MusicPlayerProvider();
      addTearDown(player.dispose);
      await player.ready;
      await player.mergeLibraryTracks([
        Music(
          id: 'local-1',
          title: 'Local',
          artist: 'L',
          album: '',
          duration: '',
          filePath: '/tmp/local.mp3',
          sourceType: TrackSourceType.localFile,
          availability: TrackAvailability.available,
        ),
      ]);
      await player.createPlaylist('我的歌单');
      await player.addToNamedPlaylist('我的歌单', player.library.first);

      final report = await LibrarySyncService().sync(api, player);
      expect(report.playlistsUp, 1);
      final mutations = api.mutations.join('\n');
      expect(mutations, contains('POST /library/playlists'));
      expect(mutations, contains('PATCH /library/playlists/pl-1'));
      expect(mutations, contains('jm_f076dbb1c7ceb56bf6172c2f'));
    });
  });
}
