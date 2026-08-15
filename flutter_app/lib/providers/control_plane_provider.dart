import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:path_provider/path_provider.dart';

import '../services/control_api.dart';
import '../services/control_api_sse.dart';
import '../services/control_api_types.dart';
import '../services/library_sync_service.dart';
import '../services/persistence_service.dart';
import '../services/rust_bridge.dart';
import '../services/web_node_service.dart';
import 'music_player_provider.dart';

class ControlPlaneProvider extends ChangeNotifier with WidgetsBindingObserver {
  String _endpoint = 'http://127.0.0.1:8787/v1';
  String _token = '';
  bool _loading = false;
  String? _error;
  Object? _errorDetail;
  Map<String, dynamic>? _health;
  Map<String, dynamic>? _node;
  Map<String, dynamic>? _deviceNode;
  Map<String, dynamic>? _browserNode;
  Map<String, dynamic>? _nodeConfig;
  Map<String, dynamic>? _audioPath;
  Map<String, dynamic>? _audioGraph;
  Map<String, dynamic>? _audioStats;
  List<Map<String, dynamic>> _transfers = [];
  List<Map<String, dynamic>> _plugins = [];
  List<Map<String, dynamic>> _communitySources = [];
  List<Map<String, dynamic>> _moderationReports = [];
  List<Map<String, dynamic>> _follows = [];
  List<Map<String, dynamic>> _pins = [];
  Object? _lastResult;
  StreamSubscription<SseEvent>? _eventSub;
  int? _lastSequence;
  bool _sseConnected = false;
  int _reconnectAttempts = 0;
  Timer? _reconnectTimer;
  Timer? _fallbackPoller;
  Timer? _coalesceTimer;
  bool _fullRefreshPending = false;
  final Set<String> _targetedPaths = {};
  bool _disposed = false;
  final WebNodeService _webNode = WebNodeService();
  final RustBridge _rustBridge = RustBridge.instance;
  String? _nativeNodeRoot;
  bool _observingLifecycle = false;

  /// 测试注入点：创建 ControlApi 的工厂。
  @visibleForTesting
  ControlApi Function(String endpoint, String token)? debugApiFactory;

  /// 测试注入点：直接替换传输列表（播放页边下边播状态展示测试用）。
  @visibleForTesting
  void debugSetTransfers(List<Map<String, dynamic>> transfers) {
    _transfers = List.of(transfers);
    notifyListeners();
  }

  String get endpoint => _endpoint;
  String get token => _token;
  bool get loading => _loading;
  String? get error => _error;

  /// API-004：稳定机器码映射后的用户可读错误文案。
  String get userErrorText {
    final detail = _errorDetail ?? _error;
    if (detail == null) return '';
    return apiErrorText(detail).full;
  }
  Map<String, dynamic>? get health => _health;
  Map<String, dynamic>? get node => _node;
  Map<String, dynamic>? get deviceNode => _deviceNode;
  Map<String, dynamic>? get browserNode => _browserNode;
  Map<String, dynamic>? get nodeConfig => _nodeConfig;
  Map<String, dynamic>? get audioPath => _audioPath;
  Map<String, dynamic>? get audioGraph => _audioGraph;
  Map<String, dynamic>? get audioStats => _audioStats;
  List<Map<String, dynamic>> get transfers => _transfers;
  List<Map<String, dynamic>> get plugins => _plugins;
  List<Map<String, dynamic>> get communitySources => _communitySources;
  List<Map<String, dynamic>> get moderationReports => _moderationReports;
  List<Map<String, dynamic>> get follows => _follows;
  LibrarySyncReport? get librarySyncReport => _librarySyncReport;

  LibrarySyncReport? _librarySyncReport;

  /// 曲库统一同步（PLR-001/PLR-002/PLR-009/UI-002）：本地优先、双向同步
  /// 后端 LibraryService，结果保存在 [librarySyncReport]。
  Future<LibrarySyncReport?> syncLibrary(MusicPlayerProvider player) async {
    _loading = true;
    _error = null;
    notifyListeners();
    final api = _makeApi();
    try {
      final report = await LibrarySyncService().sync(api, player);
      _librarySyncReport = report;
      _loading = false;
      notifyListeners();
      return report;
    } catch (error) {
      _error = error.toString();
      _errorDetail = error;
      _loading = false;
      notifyListeners();
      return null;
    } finally {
      api.close();
    }
  }
  List<Map<String, dynamic>> get pins => _pins;
  Object? get lastResult => _lastResult;
  bool get connected => _health?['status'] == 'ok';

  Future<void> load() async {
    if (!_observingLifecycle) {
      WidgetsBinding.instance.addObserver(this);
      _observingLifecycle = true;
    }
    _endpoint = await PersistenceService.loadControlEndpoint();
    // 控制令牌只保留在当前进程内，不写入 SharedPreferences 明文存储。
    _token = '';
    await Future.wait([
      _refreshBrowserNode(start: true),
      _refreshDeviceNode(start: true),
    ]);
    await refresh();
    _ensureEventStream();
  }

  Future<void> configure(String endpoint, String token) async {
    _endpoint = endpoint.trim();
    _token = token.trim();
    await PersistenceService.saveControlEndpoint(_endpoint);
    _stopEventStream();
    _lastSequence = null;
    await refresh();
    _ensureEventStream();
  }

  /// 全部视图数据统一快照。
  Future<void> refresh() => _refreshPaths(const [
    '/health',
    '/node/status',
    '/node/config',
    '/pins',
    '/transfers',
    '/plugins',
    '/community-sources',
    '/community-sources/follows',
    '/moderation-reports',
    '/audio/path',
    '/audio/stats',
    '/audio/graph',
  ]);

  /// 按指定路径做轻量刷新（SSE 事件驱动的定向更新）。
  Future<void> _refreshPaths(List<String> paths) async {
    if (_loading) return;
    _loading = true;
    _error = null;
    notifyListeners();
    final api = _makeApi();
    try {
      final values = await Future.wait<dynamic>(
        paths.map((path) => api.get(path)),
      );
      for (var i = 0; i < paths.length; i++) {
        _applyPath(paths[i], values[i]);
      }
    } catch (error) {
      _error = error.toString();
      _errorDetail = error;
    } finally {
      api.close();
      await Future.wait([_refreshBrowserNode(), _refreshDeviceNode()]);
      _loading = false;
      if (_error == null) {
        _ensureEventStream();
      } else {
        // 控制面暂时不可达：交兜底轮询与退避重连，健康后自动恢复事件流。
        _onSseDisconnect();
      }
      notifyListeners();
    }
  }

  void _applyPath(String path, dynamic value) {
    switch (path) {
      case '/health':
        _health = _map(value);
      case '/node/status':
        _node = _map(value);
      case '/node/config':
        _nodeConfig = _map(value);
      case '/pins':
        _pins = _list(value);
      case '/transfers':
        _transfers = _list(value);
      case '/plugins':
        _plugins = _list(value);
      case '/community-sources':
        _communitySources = _list(value);
      case '/community-sources/follows':
        _follows = _list(value);
      case '/moderation-reports':
        _moderationReports = _list(value);
      case '/audio/path':
        _audioPath = _map(value);
      case '/audio/stats':
        _audioStats = _map(value);
      case '/audio/graph':
        _audioGraph = _map(value);
    }
  }

  ControlApi _makeApi() {
    final factory = debugApiFactory;
    if (factory != null) return factory(_endpoint, _token);
    return ControlApi(endpoint: _endpoint, token: _token);
  }

  Future<void> createTransfer({
    required String cid,
    required String kind,
    String? destination,
    int priority = 0,
  }) async {
    final requestId = 'ui-${DateTime.now().microsecondsSinceEpoch}';
    await _mutate(
      (api) => api.post(
        '/transfers',
        {
          'request_id': requestId,
          'kind': kind,
          'target_cid': cid.trim(),
          'destination': destination?.trim().isEmpty == true
              ? null
              : destination?.trim(),
          'priority': priority,
          'network_policy': {
            'wifi_only': false,
            'cellular_limit_bytes': null,
            'max_concurrency': 2,
          },
        },
        {'idempotency-key': requestId},
      ),
    );
  }

  Future<void> configureNode(Map<String, dynamic> configuration) =>
      _mutate((api) => api.put('/node/config', configuration));

  Future<void> connectNativePeer(String address) async {
    if (_rustBridge.available && _deviceNode != null) {
      final result = _rustBridge.connectNode(address.trim());
      await _refreshDeviceNode();
      if (result != 0) {
        throw StateError(
          '${_deviceNode?['last_error'] ?? '应用内原生节点连接失败（$result）'}',
        );
      }
      notifyListeners();
      return;
    }
    await _mutate(
      (api) => api.post('/node/peers', {'address': address.trim()}),
    );
  }

  Future<void> connectBrowserPeer(String address) async {
    _browserNode = await _webNode.connect(address);
    notifyListeners();
  }

  Future<void> setBitPerfectMode(bool enabled) async {
    final graph = _audioGraph;
    if (graph == null) throw StateError('音频图尚未加载');
    final updated = Map<String, dynamic>.from(graph)
      ..['mode'] = enabled ? 'bit_perfect' : 'normal'
      ..['allow_format_conversion'] = !enabled;
    await _mutate((api) => api.put('/audio/graph', updated));
  }

  Future<void> pin(String cid) =>
      _mutate((api) => api.post('/pins/${Uri.encodeComponent(cid.trim())}'));

  Future<void> unpin(String cid) =>
      _mutate((api) => api.delete('/pins/${Uri.encodeComponent(cid.trim())}'));

  Future<Map<String, dynamic>> diagnostics() async {
    final api = ControlApi(endpoint: _endpoint, token: _token);
    try {
      final report = _map(await api.get('/diagnostics'));
      if (report == null) throw const FormatException('诊断响应不是 JSON 对象');
      return report;
    } finally {
      api.close();
    }
  }

  Future<void> registerIdentity(Map<String, dynamic> identity) =>
      _mutate((api) => api.post('/identities', identity));

  Future<void> generateIdentity(String displayName, String passphrase) =>
      _mutate(
        (api) => api.post('/identities/generate', {
          'display_name': displayName.trim(),
          'passphrase': passphrase,
        }),
      );

  Future<void> importIdentity(
    String displayName,
    String passphrase,
    Map<String, dynamic> bundle,
  ) => _mutate(
    (api) => api.post('/identities/import', {
      'display_name': displayName.trim(),
      'passphrase': passphrase,
      'bundle': bundle,
    }),
  );

  Future<void> rotateIdentity(
    String displayName,
    String passphrase,
    Map<String, dynamic> bundle,
  ) => _mutate(
    (api) => api.post('/identities/rotate', {
      'display_name': displayName.trim(),
      'passphrase': passphrase,
      'bundle': bundle,
    }),
  );

  Future<void> revokeIdentity(
    String displayName,
    String passphrase,
    Map<String, dynamic> bundle,
  ) => _mutate(
    (api) => api.post('/identities/revoke', {
      'display_name': displayName.trim(),
      'passphrase': passphrase,
      'bundle': bundle,
    }),
  );

  Future<void> publish(
    Map<String, dynamic> manifest,
    Map<String, dynamic> event,
  ) => _mutate(
    (api) => api.post('/publications', {'manifest': manifest, 'event': event}),
  );

  Future<void> signPublication({
    required String displayName,
    required String passphrase,
    required Map<String, dynamic> bundle,
    required String operation,
    Map<String, dynamic>? manifest,
    String? targetCid,
    String? reason,
  }) => _mutate(
    (api) => api.post('/publications/sign', {
      'display_name': displayName.trim(),
      'passphrase': passphrase,
      'bundle': bundle,
      'operation': operation,
      'manifest': manifest,
      'target_cid': targetCid?.trim().isEmpty == true
          ? null
          : targetCid?.trim(),
      'reason': reason?.trim().isEmpty == true ? null : reason?.trim(),
    }),
  );

  Future<void> addCommunitySource(
    Map<String, dynamic> manifest,
    String publicKey, {
    int trustOrder = 0,
  }) => _mutate(
    (api) => api.post('/community-sources', {
      'manifest': manifest,
      'maintainer_public_key': publicKey.trim(),
      'trust_order': trustOrder,
    }),
  );

  Future<void> importCommunitySource(
    String locator,
    String publicKey, {
    int trustOrder = 0,
  }) => _mutate(
    (api) => api.post('/community-sources/import', {
      'locator': locator.trim(),
      'maintainer_public_key': publicKey.trim(),
      'trust_order': trustOrder,
    }),
  );

  Future<void> installPlugin(
    Map<String, dynamic> manifest,
    String publicKey, {
    String? artifactLocation,
    List<String> grantedPermissions = const [],
    bool allowCommunityNative = false,
  }) async {
    final requestId = 'plugin-ui-${DateTime.now().microsecondsSinceEpoch}';
    await _mutate(
      (api) => api.post(
        '/plugins/install',
        {
          'request_id': requestId,
          'manifest': manifest,
          'public_key': publicKey.trim(),
          'artifact_location': artifactLocation?.trim().isEmpty == true
              ? null
              : artifactLocation?.trim(),
          'granted_permissions': grantedPermissions,
          'allow_community_native': allowCommunityNative,
        },
        {'idempotency-key': requestId},
      ),
    );
  }

  Future<void> transferAction(String id, String action) => _mutate(
    (api) => api.post('/transfers/${Uri.encodeComponent(id)}/$action'),
  );

  Future<void> setTransferPriority(String id, int priority) => _mutate(
    (api) => api.patch('/transfers/${Uri.encodeComponent(id)}/priority', {
      'priority': priority,
    }),
  );

  Future<void> pluginAction(String id, String action) =>
      _mutate((api) => api.post('/plugins/${Uri.encodeComponent(id)}/$action'));

  Future<void> uninstallPlugin(String id) =>
      _mutate((api) => api.delete('/plugins/${Uri.encodeComponent(id)}'));

  Future<void> configurePlugin(String id, Map<String, dynamic> configuration) =>
      _mutate(
        (api) => api.put(
          '/plugins/${Uri.encodeComponent(id)}/config',
          configuration,
        ),
      );

  Future<void> followPublisher(
    String identityCid,
    String publisherId,
    String displayName,
  ) async {
    final requestId = 'follow-${DateTime.now().microsecondsSinceEpoch}';
    await _mutate(
      (api) => api.post(
        '/community-sources/follows',
        {
          'identity_cid': identityCid.trim(),
          'publisher_id': publisherId.trim(),
          'display_name': displayName.trim(),
        },
        {'idempotency-key': requestId},
      ),
    );
  }

  /// COM-011：查询目标的社区策略决策。
  Future<Map<String, dynamic>?> policyDecision(String target) async {
    final api = _makeApi();
    try {
      return _map(await api.get('/policy/${Uri.encodeComponent(target)}'));
    } catch (error) {
      _error = error.toString();
      _errorDetail = error;
      notifyListeners();
      return null;
    } finally {
      api.close();
    }
  }

  /// COM-011：本地覆盖非强制策略（warn/demote/hide）。
  Future<void> overridePolicy(String target, String reason) async {
    final requestId = 'override-${DateTime.now().microsecondsSinceEpoch}';
    await _mutate(
      (api) => api.post(
        '/policy/${Uri.encodeComponent(target)}/override',
        {'request_id': requestId, 'reason': reason.trim()},
      ),
    );
  }

  /// COM-011：取消本地覆盖，恢复社区决策。
  Future<void> clearPolicyOverride(String target) => _mutate(
    (api) => api.delete('/policy/${Uri.encodeComponent(target)}/override'),
  );

  Future<void> unfollowPublisher(String identityCid) => _mutate(
    (api) =>
        api.delete('/community-sources/follows/${Uri.encodeComponent(identityCid)}'),
  );

  Future<void> setCommunitySwitches(
    String id, {
    required bool catalogEnabled,
    required bool policyEnabled,
  }) => _mutate(
    (api) => api.patch('/community-sources/${Uri.encodeComponent(id)}', {
      'catalog_enabled': catalogEnabled,
      'policy_enabled': policyEnabled,
    }),
  );

  Future<void> refreshCommunity(String id) => _mutate(
    (api) => api.post('/community-sources/${Uri.encodeComponent(id)}/refresh'),
  );

  Future<void> removeCommunity(String id) => _mutate(
    (api) => api.delete('/community-sources/${Uri.encodeComponent(id)}'),
  );

  Future<void> applyMaintainerKeyEvent(
    String sourceId,
    Map<String, dynamic> event,
  ) => _mutate(
    (api) => api.post(
      '/community-sources/${Uri.encodeComponent(sourceId)}/maintainer-key-events',
      event,
    ),
  );

  Future<void> queueModerationReport(
    Map<String, dynamic> report, {
    bool submitNow = true,
    bool encryptForRecipient = false,
  }) => _mutate(
    (api) => api.post('/moderation-reports', {
      'report': report,
      'submit_now': submitNow,
      'encrypt_for_recipient': encryptForRecipient,
    }),
  );

  Future<void> retryModerationReport(String reportId) => _mutate(
    (api) =>
        api.post('/moderation-reports/${Uri.encodeComponent(reportId)}/retry'),
  );

  Future<void> _mutate(
    Future<dynamic> Function(ControlApi api) operation,
  ) async {
    _loading = true;
    _error = null;
    notifyListeners();
    final api = _makeApi();
    try {
      _lastResult = await operation(api);
    } catch (error) {
      _error = error.toString();
      _errorDetail = error;
    } finally {
      api.close();
      _loading = false;
      notifyListeners();
    }
    if (_error == null) await refresh();
  }

  void clearError() {
    _error = null;
    notifyListeners();
  }

  // ---------- 事件流（SSE）----------
  //
  // 控制面 `/v1/events` 提供带单调 sequence 的 SSE。此处以事件流为主，
  // 断开时指数退避重连并以 30s 慢轮询兜底；收到 `snapshot.required`、
  // `stream.ready` 或检测到 sequence 缺口时整体重读快照，其余事件做
  // 300ms 合并的定向刷新。这满足 API-005：消费事件而非依赖定时轮询。

  /// 健康时启动事件流（幂等）。
  void _ensureEventStream() {
    if (_disposed || !connected || _eventSub != null) return;
    _connectEventStream();
  }

  void _connectEventStream() {
    if (_disposed) return;
    _reconnectTimer?.cancel();
    _reconnectTimer = null;
    final api = _makeApi();
    _eventSub = api.events(after: _lastSequence).listen(
      _onSseEvent,
      onError: (Object error) => _onSseDisconnect(),
      onDone: _onSseDisconnect,
    );
  }

  void _stopEventStream() {
    _eventSub?.cancel();
    _eventSub = null;
    _sseConnected = false;
  }

  void _onSseDisconnect() {
    _stopEventStream();
    if (_disposed) return;
    _startFallbackPoller();
    final shift = math.min(_reconnectAttempts, 5);
    _reconnectAttempts++;
    final delay = Duration(seconds: 1 << shift);
    _reconnectTimer?.cancel();
    _reconnectTimer = Timer(delay, () {
      if (_disposed) return;
      if (_eventSub == null && connected) _connectEventStream();
    });
  }

  void _onSseEvent(SseEvent event) {
    _sseConnected = true;
    _reconnectAttempts = 0;
    _stopFallbackPoller();

    final sequence = event.sequence;
    if (sequence != null) {
      // 检测 sequence 缺口：期间必有事件丢失，整体重读快照。
      if (_lastSequence != null && sequence > _lastSequence! + 1) {
        _scheduleFullRefresh();
      }
      if (_lastSequence == null || sequence > _lastSequence!) {
        _lastSequence = sequence;
      }
    }

    switch (event.eventType) {
      case 'snapshot.required':
      case 'stream.ready':
        // 服务器在载荷中给出最新 sequence，采纳后整体重读快照。
        _adoptPayloadSequence(event.json);
        _scheduleFullRefresh();
      case 'transfer.state_changed':
      case 'transfer.progress':
        _scheduleTargeted(['/transfers']);
      case 'node.status_changed':
        _scheduleTargeted(['/node/status', '/node/config']);
      case 'plugin.state_changed':
      case 'plugin.revoked':
        _scheduleTargeted(['/plugins']);
      case 'community_source.updated':
      case 'policy.decision_changed':
        _scheduleTargeted(['/community-sources', '/moderation-reports']);
      case 'audio.graph_changed':
        _scheduleTargeted(['/audio/path', '/audio/stats', '/audio/graph']);
      case 'audio.xrun':
      case 'audio.device_changed':
        _scheduleTargeted(['/audio/stats', '/audio/path']);
      case 'playback.state_changed':
      case 'playback.position':
      case 'playback.completed':
      case 'playback.transitioned':
      case 'playback.error':
        // 播放状态由播放页的桥事件直接承载，控制中心无需响应。
        break;
      case 'publication.changed':
        break;
      default:
        // 未知事件类型：保守整体重读，保持对服务器新事件的前向兼容。
        _scheduleFullRefresh();
    }
  }

  void _adoptPayloadSequence(Map<String, dynamic>? payload) {
    final sequence = payload?['sequence'];
    if (sequence is int && (_lastSequence == null || sequence > _lastSequence!)) {
      _lastSequence = sequence;
    }
  }

  void _scheduleTargeted(List<String> paths) {
    _targetedPaths.addAll(paths);
    _coalesceTimer ??= Timer(
      const Duration(milliseconds: 300),
      _drainScheduledRefresh,
    );
  }

  void _scheduleFullRefresh() {
    _fullRefreshPending = true;
    _coalesceTimer ??= Timer(
      const Duration(milliseconds: 300),
      _drainScheduledRefresh,
    );
  }

  Future<void> _drainScheduledRefresh() async {
    _coalesceTimer = null;
    if (_loading) {
      // 有刷新正在执行：稍后重排，避免丢更新。
      _coalesceTimer = Timer(
        const Duration(milliseconds: 300),
        _drainScheduledRefresh,
      );
      return;
    }
    final full = _fullRefreshPending;
    final paths = _targetedPaths.toList();
    _fullRefreshPending = false;
    _targetedPaths.clear();
    if (full) {
      await refresh();
    } else if (paths.isNotEmpty) {
      await _refreshPaths(paths);
    }
  }

  void _startFallbackPoller() {
    if (_fallbackPoller != null) return;
    _fallbackPoller = Timer.periodic(const Duration(seconds: 30), (_) {
      if (_sseConnected) return;
      unawaited(refresh());
    });
  }

  void _stopFallbackPoller() {
    _fallbackPoller?.cancel();
    _fallbackPoller = null;
  }

  Future<void> _refreshBrowserNode({bool start = false}) async {
    if (!_webNode.available) return;
    try {
      _browserNode = start ? await _webNode.start() : await _webNode.status();
    } catch (error) {
      _browserNode = {
        'implementation': 'helia',
        'lifecycle_state': 'failed',
        'last_error': error.toString(),
        'limitations': const <String>[],
      };
    }
  }

  Future<void> _refreshDeviceNode({bool start = false}) async {
    if (kIsWeb || !_rustBridge.available) return;
    try {
      if (start) {
        final support = await getApplicationSupportDirectory();
        _nativeNodeRoot = '${support.path}/embedded-ipfs';
        final result = _rustBridge.startNode(_nativeNodeRoot!);
        if (result != 0) {
          throw StateError('应用内原生节点启动失败（错误码 $result）');
        }
      }
      _deviceNode = _rustBridge.nodeStatus();
    } catch (error) {
      _deviceNode = {
        'implementation': 'rust-ipfs',
        'lifecycle_state': 'failed',
        'last_error': error.toString(),
        'persists_after_app_close': false,
        'limitations': const ['后台联网遵循系统限制，可能被暂停', '应用进程关闭后节点不会继续提供内容'],
      };
    }
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    unawaited(_applyNativeNodeLifecycle(state));
  }

  Future<void> _applyNativeNodeLifecycle(AppLifecycleState state) async {
    if (kIsWeb || !_rustBridge.available) return;
    if (state == AppLifecycleState.resumed) {
      final current = _rustBridge.nodeStatus();
      if (current?['lifecycle_state'] == 'stopped' && _nativeNodeRoot != null) {
        _rustBridge.startNode(_nativeNodeRoot!);
      } else {
        _rustBridge.setNodeForeground(true);
      }
    } else if (state == AppLifecycleState.detached) {
      _rustBridge.stopNode();
    } else {
      _rustBridge.setNodeForeground(false);
    }
    await _refreshDeviceNode();
    notifyListeners();
  }

  @override
  void dispose() {
    _disposed = true;
    _stopEventStream();
    _reconnectTimer?.cancel();
    _fallbackPoller?.cancel();
    _coalesceTimer?.cancel();
    if (_observingLifecycle) {
      WidgetsBinding.instance.removeObserver(this);
      _observingLifecycle = false;
    }
    _rustBridge.stopNode();
    unawaited(_webNode.stop());
    super.dispose();
  }

  static Map<String, dynamic>? _map(dynamic value) =>
      value is Map<String, dynamic> ? value : null;

  static List<Map<String, dynamic>> _list(dynamic value) =>
      value is List<dynamic>
      ? value.whereType<Map<String, dynamic>>().toList(growable: false)
      : [];
}
