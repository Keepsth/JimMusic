import 'dart:async';
import 'dart:math';
import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:just_audio/just_audio.dart';

import '../models/music.dart';
import '../models/playback_mode.dart';
import '../models/playlist.dart';
import '../services/control_api.dart';
import '../services/media_scanner_service.dart';
import '../services/persistence_service.dart';
import '../services/rust_bridge.dart';
import '../services/transfer_stream_audio_source.dart';

enum PlayerState { stopped, playing, paused, buffering }

class MusicPlayerProvider extends ChangeNotifier {
  PlayerState _playerState = PlayerState.stopped;
  Music? _currentMusic;
  List<Music> _playlist = [];
  double _currentPosition = 0.0;
  double _duration = 0.0;
  int _currentIndex = 0;
  double _volume = 1.0;
  bool _muted = false;
  String? _playbackError;
  int _crossfadeMilliseconds = 0;
  bool _crossfadeEqualPower = true;
  PlaybackMode _playbackMode = PlaybackMode.sequence;

  /// 已收藏曲目（用 id 集合表示，跨列表共享收藏状态）。
  Set<String> _favoriteIds = {};

  /// 播放列表集合：名称 -> 播放列表。
  Map<String, Playlist> _playlists = {};

  /// 当前搜索关键字（空表示不过滤）。
  String _searchQuery = '';

  /// 媒体库（全部曲目，含扫描导入的本地文件）。
  List<Music> _library = [];

  /// Rust 宿主桥（桌面/移动端经 FFI 真实播放）。
  RustBridge? _bridge;

  /// 桥事件订阅。
  StreamSubscription<BridgeEvent>? _bridgeSub;

  /// 当前是否正在经桥播放。
  bool _bridgeActive = false;

  /// 桥是否可用。
  bool get _bridgeAvailable =>
      _bridge != null && _bridge!.available && _bridge!.readyForPlayback;

  /// 可经桥播放的本地曲目（有文件路径，顺序与后端队列一致）。
  List<Music> get _playableTracks => _library
      .where(
        (m) =>
            m.availability == TrackAvailability.available &&
            m.filePath != null &&
            m.filePath!.isNotEmpty,
      )
      .toList();

  /// 真实音频播放器（懒加载；仅在播放带 filePath 的曲目时创建）。
  AudioPlayer? _audio;

  /// 音频流订阅。
  final List<StreamSubscription<dynamic>> _audioSubs = [];

  // Getters
  PlayerState get playerState => _playerState;
  Music? get currentMusic => _currentMusic;
  List<Music> get playlist => _playlist;
  double get currentPosition => _currentPosition;
  double get duration => _duration;
  int get currentIndex => _currentIndex;
  double get volume => _volume;
  bool get muted => _muted;
  String? get playbackError => _playbackError;
  int get crossfadeMilliseconds => _crossfadeMilliseconds;
  bool get crossfadeEqualPower => _crossfadeEqualPower;
  PlaybackMode get playbackMode => _playbackMode;
  bool get supportsNativeTransitions =>
      _bridge != null && _bridge!.readyForPlayback;
  Set<String> get favoriteIds => _favoriteIds;
  Map<String, Playlist> get playlists => _playlists;
  String get searchQuery => _searchQuery;
  List<Music> get library => _library;

  bool get isPlaying => _playerState == PlayerState.playing;
  bool get isPaused => _playerState == PlayerState.paused;
  bool get isBuffering => _playerState == PlayerState.buffering;

  /// just_audio 已缓冲位置（秒）；桥模式或未知时回退到播放位置。
  double _bufferedPosition = 0.0;

  /// 已缓冲位置（秒）。
  double get bufferedPosition {
    if (_bridgeActive) return _currentPosition;
    return _bufferedPosition.clamp(0.0, double.infinity).toDouble();
  }

  /// 当前音源的稳定机器标识：本地文件 / Rust 桥 / 内存字节（Web）/
  /// IPFS 传输流 / 内容寻址 CID / 社区来源（UI-001）。
  String get sourceLabel {
    final music = _currentMusic;
    if (music == null) return '无';
    if (_bridgeActive) return 'Rust Core 输出';
    if (music.id.startsWith('transfer-')) return 'IPFS 边下边播';
    if (music.audioBytes != null && music.audioBytes!.isNotEmpty) {
      return '内存字节（Web）';
    }
    if (music.filePath != null && music.filePath!.isNotEmpty) {
      return '本地文件';
    }
    switch (music.sourceType) {
      case TrackSourceType.ipfs:
        return 'IPFS（${music.renditionCid ?? music.manifestCid ?? 'CID'}）';
      case TrackSourceType.community:
        return '社区来源';
      case TrackSourceType.cached:
        return '本地缓存';
      case TrackSourceType.localFile:
      case TrackSourceType.localMemory:
        return '本地文件';
    }
  }

  /// 边下边播伪曲目对应的传输任务 ID（普通曲目为 null）。
  String? get transferTaskId {
    final id = _currentMusic?.id ?? '';
    return id.startsWith('transfer-') ? id.substring('transfer-'.length) : null;
  }

  /// 测试注入点：直接设置当前曲目与播放状态（跳过真实音频链路）。
  @visibleForTesting
  void debugSetCurrentTrack(
    Music track, {
    PlayerState state = PlayerState.buffering,
  }) {
    _currentMusic = track;
    _playlist = [track];
    _currentIndex = 0;
    _currentPosition = 0.0;
    _bufferedPosition = 0.0;
    _bridgeActive = false;
    _playerState = state;
    notifyListeners();
  }

  /// 媒体库中过滤掉搜索后的结果。
  List<Music> get filteredLibrary {
    final q = _searchQuery.trim().toLowerCase();
    if (q.isEmpty) return _library;
    return _library.where((m) {
      return m.title.toLowerCase().contains(q) ||
          m.artist.toLowerCase().contains(q) ||
          m.album.toLowerCase().contains(q) ||
          (m.publisher?.toLowerCase().contains(q) ?? false);
    }).toList();
  }

  /// 收藏的曲目列表。
  List<Music> get favorites =>
      _library.where((m) => _favoriteIds.contains(m.id)).toList();

  MusicPlayerProvider() {
    _ready = _loadPersisted();
    _bridge = RustBridge.instance;
  }

  /// 持久化初始化完成的 Future（仅触发一次，由构造函数启动）。
  late final Future<void> _ready;

  /// 等待持久化初始化完成。
  Future<void> get ready => _ready;

  Future<void> _loadPersisted() async {
    _library = await PersistenceService.loadLibrary();
    _favoriteIds = await PersistenceService.loadFavoriteIds();
    _playlists = await PersistenceService.loadPlaylists();
    final session = await PersistenceService.loadPlaybackSession();
    _volume = ((session['volume'] as num?)?.toDouble() ?? 1.0)
        .clamp(0.0, 1.0)
        .toDouble();
    _muted = session['muted'] as bool? ?? false;
    _crossfadeMilliseconds =
        ((session['crossfadeMilliseconds'] as num?)?.toInt() ?? 0)
            .clamp(0, 30_000)
            .toInt();
    _crossfadeEqualPower = session['crossfadeEqualPower'] as bool? ?? true;
    _playbackMode = PlaybackMode.values.firstWhere(
      (mode) => mode.name == session['playbackMode'],
      orElse: () => PlaybackMode.sequence,
    );
    _currentPosition = ((session['position'] as num?)?.toDouble() ?? 0.0)
        .clamp(0.0, double.infinity)
        .toDouble();
    final queueIds = (session['queue'] as List<dynamic>? ?? const [])
        .whereType<String>()
        .toList();
    _playlist = queueIds.map(_findTrack).whereType<Music>().toList();
    final currentId = session['currentTrackId'] as String?;
    _currentMusic = currentId == null ? null : _findTrack(currentId);
    final restoredIndex = _currentMusic == null
        ? -1
        : _playlist.indexWhere((track) => track.id == _currentMusic!.id);
    _currentIndex = restoredIndex < 0 ? 0 : restoredIndex;
    // 会话恢复永不自动播放，避免启动时意外发声。
    _playerState = PlayerState.stopped;
    // 将收藏状态合并到曲目对象。
    _library = _library
        .map(
          (m) => _favoriteIds.contains(m.id) ? m.copyWith(isFavorite: true) : m,
        )
        .toList();
    notifyListeners();
  }

  /// 等待持久化初始化完成（测试辅助；等价于 [ready]）。
  Future<void> loadForTest() => ready;

  // ---------- 媒体库 ----------

  /// 扫描本地文件并导入曲目，返回新增数量。
  Future<int> importFiles() async {
    final tracks = await MediaScannerService.pickAudioFiles();
    final existingIds = _library.map((m) => m.id).toSet();
    final added = tracks.where((t) => !existingIds.contains(t.id)).toList();
    _library = [..._library, ...added];
    notifyListeners();
    await PersistenceService.saveLibrary(_library);
    return added.length;
  }

  /// 合并外部（本地扫描、Manifest 或社区目录）曲目；按稳定 ID 去重。
  Future<int> mergeLibraryTracks(Iterable<Music> tracks) async {
    final existingIds = _library.map((track) => track.id).toSet();
    final added = tracks.where((track) => existingIds.add(track.id)).toList();
    _library = [..._library, ...added];
    notifyListeners();
    await PersistenceService.saveLibrary(_library);
    return added.length;
  }

  /// 设置搜索关键字。
  void setSearchQuery(String query) {
    _searchQuery = query;
    notifyListeners();
  }

  // ---------- 收藏 ----------

  /// 是否为已收藏曲目。
  bool isFavorite(Music music) => _favoriteIds.contains(music.id);

  /// 切换收藏状态。
  Future<void> toggleFavorite(Music music) async {
    if (_favoriteIds.contains(music.id)) {
      _favoriteIds.remove(music.id);
    } else {
      _favoriteIds.add(music.id);
    }
    _library = _library
        .map(
          (m) => m.id == music.id
              ? m.copyWith(isFavorite: _favoriteIds.contains(m.id))
              : m,
        )
        .toList();
    notifyListeners();
    await PersistenceService.saveFavoriteIds(_favoriteIds);
    await PersistenceService.saveLibrary(_library);
  }

  // ---------- 播放列表 ----------

  /// 创建播放列表。
  Future<void> createPlaylist(String name) async {
    final trimmed = name.trim();
    if (trimmed.isEmpty || _playlists.containsKey(trimmed)) return;
    _playlists[trimmed] = Playlist(name: trimmed);
    notifyListeners();
    await _savePlaylists();
  }

  /// 删除播放列表。
  Future<void> deletePlaylist(String name) async {
    _playlists.remove(name);
    notifyListeners();
    await _savePlaylists();
  }

  /// 向播放列表添加曲目。
  Future<void> addToNamedPlaylist(String playlistName, Music music) async {
    final pl = _playlists[playlistName];
    if (pl == null) return;
    if (!pl.trackIds.contains(music.id)) {
      pl.trackIds.add(music.id);
      notifyListeners();
      await _savePlaylists();
    }
  }

  /// 从播放列表移除曲目。
  Future<void> removeFromNamedPlaylist(
    String playlistName,
    String trackId,
  ) async {
    final pl = _playlists[playlistName];
    if (pl == null) return;
    pl.trackIds.remove(trackId);
    notifyListeners();
    await _savePlaylists();
  }

  /// 同步远端命名歌单：创建或覆盖同名歌单的曲目集合（只保留曲库中
  /// 存在的 ID；曲库未含有的远端曲目在拉取阶段已合并）。
  Future<void> syncNamedPlaylist(String name, List<String> trackIds) async {
    final trimmed = name.trim();
    if (trimmed.isEmpty) return;
    final existing = _playlists[trimmed] ?? Playlist(name: trimmed);
    existing.trackIds
      ..clear()
      ..addAll(trackIds.where((id) => _library.any((m) => m.id == id)));
    _playlists[trimmed] = existing;
    notifyListeners();
    await _savePlaylists();
  }

  /// 应用来自后端会话快照（PLR-009）：恢复队列与位置，但绝不自动播放。
  /// 返回是否实际应用（当前曲目能在曲库中解析）。
  Future<bool> applySessionSnapshot({
    required List<String> queueIds,
    required String? currentTrackId,
    required double position,
  }) async {
    if (currentTrackId == null && queueIds.isEmpty) return false;
    final resolvedQueue = queueIds.map(_findTrack).whereType<Music>().toList();
    if (resolvedQueue.isEmpty) return false;
    _playlist = resolvedQueue;
    final current = currentTrackId == null
        ? null
        : _findTrack(currentTrackId);
    _currentMusic = current ?? resolvedQueue.first;
    final restoredIndex = _playlist.indexWhere(
      (track) => track.id == _currentMusic!.id,
    );
    _currentIndex = restoredIndex < 0 ? 0 : restoredIndex;
    _currentPosition = position.clamp(0.0, double.infinity).toDouble();
    _playerState = PlayerState.stopped; // 会话恢复永不自动播放
    notifyListeners();
    await _saveSession();
    return true;
  }

  /// 根据播放列表名解析曲目列表。
  List<Music> tracksInPlaylist(String name) {
    final pl = _playlists[name];
    if (pl == null) return [];
    return _library.where((m) => pl.trackIds.contains(m.id)).toList();
  }

  Future<void> _savePlaylists() => PersistenceService.savePlaylists(_playlists);

  // ---------- 播放控制 ----------

  Future<void> togglePlayPause() async {
    if (_playerState == PlayerState.playing) {
      await pause();
    } else if (_playerState == PlayerState.paused) {
      await resume();
    } else {
      // stopped（含首次尚未选择曲目）：开始播放。
      await play();
    }
  }

  Future<void> play([Music? music]) async {
    if (music != null) {
      _currentMusic = music;
      if (_playlist.every((track) => track.id != music.id)) {
        _playlist = List<Music>.from(_library);
      }
      _currentIndex = _playlist.indexWhere((track) => track.id == music.id);
    }

    if (_currentMusic == null && _playlist.isNotEmpty) {
      _currentMusic = _playlist[0];
      _currentIndex = 0;
    }

    final target = _currentMusic;
    if (target == null) return;

    if (target.availability != TrackAvailability.available) {
      _failPlayback(
        target.unavailableReason ?? '当前音源不可用：${target.availability.name}',
      );
      return;
    }
    _playbackError = null;
    _playerState = PlayerState.buffering;
    notifyListeners();

    // 桥优先：桌面/移动端有本地文件路径时，经 Rust 桥播放（Core 队列 + 自动切歌）。
    if (target.filePath != null &&
        target.filePath!.isNotEmpty &&
        _bridgeAvailable) {
      if (_playViaBridge(target)) {
        await _saveSession();
        return;
      }
    }

    // Web 端：内存字节 → data URI 真实播放。
    if (target.audioBytes != null && target.audioBytes!.isNotEmpty) {
      if (await _playBytes(target.audioBytes!, target.mimeType)) {
        _playerState = PlayerState.playing;
        notifyListeners();
        await _saveSession();
        return;
      }
    }

    // 有本地文件路径：使用 just_audio 真实播放。
    if (target.filePath != null && target.filePath!.isNotEmpty) {
      if (await _playReal(target.filePath!)) {
        _playerState = PlayerState.playing;
        notifyListeners();
        await _saveSession();
        return;
      }
    }

    _failPlayback(_playbackError ?? '没有可用的真实音源或音频输出，未开始播放');
  }

  /// 边下边播（DST-007）：经控制面 `/v1/transfers/{id}/stream` 播放正在
  /// 下载的音频。服务端跟随 part 文件增长推送字节，播放器按需用 Range
  /// 请求 Seek；任务终结后流正常结束。会话快照不保存该伪曲目（重启后
  /// 由传输列表重新发起）。
  Future<void> playTransferStream({
    required String taskId,
    required String endpoint,
    String token = '',
    String? mimeType,
    String? title,
  }) async {
    await stop();
    _bridgeActive = false;
    _playbackError = null;
    _playerState = PlayerState.buffering;
    final shortId = taskId.length > 8 ? taskId.substring(0, 8) : taskId;
    _currentMusic = Music(
      id: 'transfer-$taskId',
      title: title ?? '网络串流 $shortId…',
      artist: 'IPFS 边下边播',
      album: '',
      duration: '',
      sourceType: TrackSourceType.ipfs,
      availability: TrackAvailability.available,
      mimeType: mimeType,
    );
    _playlist = [_currentMusic!];
    _currentIndex = 0;
    _currentPosition = 0.0;
    _bufferedPosition = 0.0;
    notifyListeners();

    final source = transferStreamAudioSource(
      endpoint: endpoint,
      token: token,
      taskId: taskId,
    );
    try {
      final audio = await _ensureAudio();
      await audio.setAudioSource(source);
      unawaited(audio.play());
    } catch (error) {
      _failPlayback('边下边播失败：$error');
    }
  }

  /// 网络曲目接入同一播放入口（PLR-007/DST-003）：按内容 CID 建立幂等
  /// fetch 传输任务，随后经传输流端点边下边播。已在本地文件/缓存的曲目
  /// 应继续走 [play]。
  Future<void> playNetworkTrack(
    Music music, {
    required String endpoint,
    String token = '',
  }) async {
    final cid = music.renditionCid ?? music.manifestCid;
    if (cid == null || cid.isEmpty) {
      _failPlayback('网络曲目缺少内容 CID');
      return;
    }
    final requestId = 'play-${music.id}';
    final api = ControlApi(endpoint: endpoint, token: token);
    try {
      final created = await api.post(
        '/transfers',
        {
          'request_id': requestId,
          'kind': 'fetch',
          'target_cid': cid,
          'network_policy': {'wifi_only': false, 'max_concurrency': 2},
        },
        {'idempotency-key': requestId},
      );
      final taskId = created is Map<String, dynamic>
          ? created['task_id'] as String?
          : null;
      if (taskId == null || taskId.isEmpty) {
        _failPlayback('无法建立网络传输任务');
        return;
      }
      await playTransferStream(
        taskId: taskId,
        endpoint: endpoint,
        token: token,
        mimeType: _mimeForCodec(music.codec),
        title: music.title,
      );
    } catch (error) {
      _failPlayback('无法开始网络播放：$error');
    } finally {
      api.close();
    }
  }

  /// 编解码器名 → MIME（传输流端点的内容类型提示）。
  String? _mimeForCodec(String? codec) {
    switch ((codec ?? '').toLowerCase()) {
      case 'mp3':
        return 'audio/mpeg';
      case 'aac':
        return 'audio/aac';
      case 'm4a':
      case 'alac':
        return 'audio/mp4';
      case 'flac':
        return 'audio/flac';
      case 'wav':
        return 'audio/wav';
      case 'opus':
      case 'vorbis':
      case 'ogg':
        return 'audio/ogg';
      default:
        return null;
    }
  }

  Future<void> pause() async {
    if (_playerState == PlayerState.paused) return;
    if (_bridgeActive) {
      final code = _bridge?.pause();
      if (code != 0) _failPlayback(_bridge?.lastError() ?? '暂停失败（错误码 $code）');
      return;
    }
    if (_audio == null) return;
    try {
      await _audio?.pause();
      _playerState = PlayerState.paused;
      notifyListeners();
      await _saveSession();
    } catch (error) {
      _failPlayback('暂停失败：$error');
    }
  }

  /// 从暂停处继续播放：若已有已加载的音频源，仅调用 `play()` 恢复，
  /// **不重新加载源**（重新 `setAudioSource`/`setFilePath` 会把进度重置到 0）。
  Future<void> resume() async {
    if (_playerState == PlayerState.playing) return;

    if (_bridgeActive) {
      final code = _bridge?.resume();
      if (code != 0) _failPlayback(_bridge?.lastError() ?? '继续播放失败（错误码 $code）');
      return;
    }

    final audio = _audio;
    if (audio != null) {
      try {
        unawaited(audio.play());
        _playerState = PlayerState.playing;
        notifyListeners();
      } catch (error) {
        _failPlayback('继续播放失败：$error');
      }
    } else {
      await play();
    }
  }

  Future<void> stop() async {
    if (_bridgeActive) {
      _bridge?.stop();
      _bridgeActive = false;
    } else {
      try {
        await _audio?.stop();
      } catch (_) {}
    }
    _playerState = PlayerState.stopped;
    _currentPosition = 0.0;
    _bufferedPosition = 0.0;
    notifyListeners();
    await _saveSession();
  }

  Future<void> next() async {
    // 桥模式：委托后端 Player 切歌并自动播放。
    if (_bridgeActive) {
      _bridge?.next();
      return;
    }
    if (_playlist.isEmpty) return;
    _currentIndex = (_currentIndex + 1) % _playlist.length;
    _currentMusic = _playlist[_currentIndex];
    _currentPosition = 0.0;

    if (_playerState == PlayerState.playing) {
      await play();
    } else {
      notifyListeners();
    }
  }

  Future<void> previous() async {
    // 桥模式：委托后端 Player 切歌并自动播放。
    if (_bridgeActive) {
      _bridge?.previous();
      return;
    }
    if (_playlist.isEmpty) return;
    _currentIndex = (_currentIndex - 1 + _playlist.length) % _playlist.length;
    _currentMusic = _playlist[_currentIndex];
    _currentPosition = 0.0;

    if (_playerState == PlayerState.playing) {
      await play();
    } else {
      notifyListeners();
    }
  }

  Future<void> seekTo(double position) async {
    _currentPosition = position.clamp(0.0, _duration).toDouble();
    if (_bridgeActive) {
      _bridge?.seek(position);
      notifyListeners();
      return;
    }
    try {
      await _audio?.seek(Duration(seconds: position.toInt()));
    } catch (_) {}
    notifyListeners();
    await _saveSession();
  }

  Future<void> setVolume(double volume) async {
    _volume = volume.clamp(0.0, 1.0).toDouble();
    try {
      await _audio?.setVolume(_muted ? 0.0 : _volume);
    } catch (_) {}
    notifyListeners();
    await _saveSession();
  }

  Future<void> setMuted(bool muted) async {
    _muted = muted;
    try {
      await _audio?.setVolume(_muted ? 0.0 : _volume);
    } catch (_) {}
    notifyListeners();
    await _saveSession();
  }

  /// Zero selects sample-contiguous gapless playback. Positive values are
  /// applied by the Rust double-timeline mixer when native output is active.
  Future<void> setCrossfade(Duration duration, {bool? equalPower}) async {
    _crossfadeMilliseconds = duration.inMilliseconds.clamp(0, 30_000).toInt();
    if (equalPower != null) _crossfadeEqualPower = equalPower;
    if (_bridgeAvailable) {
      final code = _bridge!.setCrossfade(
        Duration(milliseconds: _crossfadeMilliseconds),
        equalPower: _crossfadeEqualPower,
      );
      if (code != 0) {
        _playbackError = _bridge!.lastError() ?? '无法更新切歌过渡（错误码 $code）';
      }
    }
    notifyListeners();
    await _saveSession();
  }

  /// 循环切换播放模式（PLR-102）：顺序 → 列表循环 → 单曲循环 → 随机。
  Future<void> cyclePlaybackMode() async {
    _playbackMode = switch (_playbackMode) {
      PlaybackMode.sequence => PlaybackMode.repeatAll,
      PlaybackMode.repeatAll => PlaybackMode.repeatOne,
      PlaybackMode.repeatOne => PlaybackMode.shuffle,
      PlaybackMode.shuffle => PlaybackMode.sequence,
    };
    await _applyPlaybackModeToAudio();
    notifyListeners();
    await _saveSession();
  }

  Future<void> _applyPlaybackModeToAudio() async {
    final audio = _audio;
    if (audio == null) return;
    try {
      await audio.setLoopMode(switch (_playbackMode) {
        PlaybackMode.sequence => LoopMode.off,
        PlaybackMode.repeatOne => LoopMode.one,
        PlaybackMode.repeatAll || PlaybackMode.shuffle => LoopMode.all,
      });
      await audio.setShuffleModeEnabled(_playbackMode == PlaybackMode.shuffle);
    } catch (_) {
      // 平台不支持时静默保留当前行为。
    }
  }

  void addToPlaylist(Music music) {
    _playlist.add(music);
    notifyListeners();
  }

  void removeFromPlaylist(int index) {
    if (index >= 0 && index < _playlist.length) {
      _playlist.removeAt(index);
      if (_currentIndex >= _playlist.length && _playlist.isNotEmpty) {
        _currentIndex = _playlist.length - 1;
        _currentMusic = _playlist[_currentIndex];
      }
      notifyListeners();
    }
  }

  // ---------- 内部 ----------

  /// 经 Rust 桥播放：把本地曲目队列交给后端 Player，由后端负责自动切歌。成功返回 true。
  bool _playViaBridge(Music target) {
    try {
      var tracks = _playableTracks;
      // 随机模式：先洗牌再交给桥（桥仍按列表推进）。
      if (_playbackMode == PlaybackMode.shuffle) {
        tracks = shuffledPlaylist(tracks, Random());
      }
      final index = tracks.indexWhere((m) => m.id == target.id);
      if (index < 0) return false;

      final paths = tracks.map((m) => m.filePath!).toList();
      final transitionCode = _bridge!.setCrossfade(
        Duration(milliseconds: _crossfadeMilliseconds),
        equalPower: _crossfadeEqualPower,
      );
      if (transitionCode != 0) {
        _playbackError =
            _bridge!.lastError() ?? '设置切歌过渡失败（错误码 $transitionCode）';
        return false;
      }
      final qCode = _bridge!.setQueue(paths);
      if (qCode != 0) {
        _playbackError = _bridge!.lastError() ?? '设置播放队列失败（错误码 $qCode）';
        return false;
      }
      final pCode = _bridge!.playTrack(index);
      if (pCode != 0) {
        _playbackError = _bridge!.lastError() ?? '播放请求失败（错误码 $pCode）';
        return false;
      }

      _bridgeActive = true;
      // 桥在 set_queue 内同步读取各曲目元数据时长，此处可获取当前曲目时长。
      _duration = _bridge!.duration();
      _bridgeSub ??= _bridge!.events.listen(_onBridgeEvent);
      return true;
    } catch (_) {
      return false;
    }
  }

  /// 处理来自 Rust 桥的播放事件（状态/进度/自动切歌）。
  void _onBridgeEvent(BridgeEvent event) {
    if (!_bridgeActive) return;
    if (event.eventType == PlaybackEventType.playing) {
      _playerState = PlayerState.playing;
      // 后端自动切歌后，同步当前曲目并应用队列边界模式（PLR-102）。
      final tracks = _playableTracks;
      final idx = _bridge?.currentIndex() ?? 0;
      if (tracks.isNotEmpty && idx >= 0 && idx < tracks.length) {
        final now = tracks[idx];
        final isAdvance = now.id != _currentMusic?.id;
        final decision = evaluateAdvance(
          isAdvance: isAdvance,
          currentIndex: _currentIndex,
          advancedIndex: idx,
          mode: _playbackMode,
        );
        switch (decision) {
          case PlaybackDecision.replayCurrent:
            // 单曲循环：桥已切走，把当前曲目重新拉起。
            _bridge?.playTrack(_currentIndex);
            return;
          case PlaybackDecision.stop:
            unawaited(stop());
            return;
          case PlaybackDecision.accept:
            break;
        }
        if (isAdvance) {
          _currentMusic = now;
          _currentPosition = 0.0;
          final inPlaylist = _playlist.indexOf(now);
          _currentIndex = inPlaylist >= 0 ? inPlaylist : idx;
        }
      }
    } else if (event.eventType == PlaybackEventType.paused) {
      _playerState = PlayerState.paused;
    } else if (event.eventType == PlaybackEventType.stopped) {
      _playerState = PlayerState.stopped;
      _currentPosition = 0.0;
    } else if (event.eventType == PlaybackEventType.progress) {
      _currentPosition = _bridge?.position() ?? _currentPosition;
    } else if (event.eventType == PlaybackEventType.error) {
      _bridgeActive = false;
      _failPlayback(_bridge?.lastError() ?? 'Rust Core 报告播放失败');
      return;
    }
    notifyListeners();
    unawaited(_saveSession());
  }

  /// 尝试用 just_audio 播放本地文件。失败时返回 false，调用方会显示真实错误。
  Future<bool> _playReal(String path) async {
    try {
      final audio = await _ensureAudio();
      await audio.setFilePath(path);
      unawaited(audio.play());
      return true;
    } catch (error) {
      _playbackError = '无法播放本地文件：$error';
      return false;
    }
  }

  /// 尝试用 just_audio 播放内存字节（Web 端）。经 data URI 供浏览器 `<audio>` 元素加载。
  Future<bool> _playBytes(Uint8List bytes, String? mimeType) async {
    try {
      final audio = await _ensureAudio();
      final uri = Uri.dataFromBytes(bytes, mimeType: mimeType ?? 'audio/mpeg');
      await audio.setAudioSource(AudioSource.uri(uri));
      unawaited(audio.play());
      return true;
    } catch (error) {
      _playbackError = '浏览器无法解码该音频：$error';
      return false;
    }
  }

  /// 懒创建 AudioPlayer 并订阅 position/duration/completed 流。
  Future<AudioPlayer> _ensureAudio() async {
    final existing = _audio;
    if (existing != null) return existing;

    final audio = AudioPlayer();
    _audio = audio;

    _audioSubs.add(
      audio.positionStream.listen((pos) {
        _currentPosition = pos.inSeconds.toDouble();
        notifyListeners();
      }),
    );
    _audioSubs.add(
      audio.durationStream.listen((dur) {
        if (dur != null) _duration = dur.inSeconds.toDouble();
      }),
    );
    _audioSubs.add(
      audio.bufferedPositionStream.listen((buffered) {
        _bufferedPosition = buffered.inSeconds.toDouble();
        notifyListeners();
      }),
    );
    _audioSubs.add(
      audio.playerStateStream.listen((state) {
        if (state.processingState == ProcessingState.completed) {
          // 播放完毕自动切下一首。
          next();
        }
      }),
    );
    _audioSubs.add(
      audio.playbackEventStream.listen(
        (_) {},
        onError: (Object error, StackTrace stackTrace) {
          _failPlayback('音频输出错误：$error');
        },
      ),
    );
    await audio.setVolume(_muted ? 0.0 : _volume);
    await _applyPlaybackModeToAudio();
    return audio;
  }

  void clearPlaybackError() {
    _playbackError = null;
    notifyListeners();
  }

  void _failPlayback(String message) {
    _playerState = PlayerState.stopped;
    _playbackError = message;
    notifyListeners();
    unawaited(_saveSession());
  }

  Future<void> _saveSession() => PersistenceService.savePlaybackSession({
    'queue': _playlist.map((track) => track.id).toList(),
    'currentTrackId': _currentMusic?.id,
    'position': _currentPosition,
    'volume': _volume,
    'muted': _muted,
    'crossfadeMilliseconds': _crossfadeMilliseconds,
    'crossfadeEqualPower': _crossfadeEqualPower,
    'playbackMode': _playbackMode.name,
    'autoPlay': false,
  });

  Music? _findTrack(String id) {
    for (final track in _library) {
      if (track.id == id) return track;
    }
    return null;
  }

  @override
  void dispose() {
    _bridgeSub?.cancel();
    for (final s in _audioSubs) {
      s.cancel();
    }
    _audio?.dispose();
    super.dispose();
  }
}
