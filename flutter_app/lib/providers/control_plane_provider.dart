import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:path_provider/path_provider.dart';

import '../services/control_api.dart';
import '../services/persistence_service.dart';
import '../services/rust_bridge.dart';
import '../services/web_node_service.dart';

class ControlPlaneProvider extends ChangeNotifier with WidgetsBindingObserver {
  String _endpoint = 'http://127.0.0.1:8787/v1';
  String _token = '';
  bool _loading = false;
  String? _error;
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
  List<Map<String, dynamic>> _pins = [];
  Object? _lastResult;
  Timer? _poller;
  final WebNodeService _webNode = WebNodeService();
  final RustBridge _rustBridge = RustBridge.instance;
  String? _nativeNodeRoot;
  bool _observingLifecycle = false;

  String get endpoint => _endpoint;
  String get token => _token;
  bool get loading => _loading;
  String? get error => _error;
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
    _ensurePolling();
  }

  Future<void> configure(String endpoint, String token) async {
    _endpoint = endpoint.trim();
    _token = token.trim();
    await PersistenceService.saveControlEndpoint(_endpoint);
    await refresh();
    _ensurePolling();
  }

  Future<void> refresh() async {
    if (_loading) return;
    _loading = true;
    _error = null;
    notifyListeners();
    final api = ControlApi(endpoint: _endpoint, token: _token);
    try {
      final values = await Future.wait<dynamic>([
        api.get('/health'),
        api.get('/node/status'),
        api.get('/node/config'),
        api.get('/pins'),
        api.get('/transfers'),
        api.get('/plugins'),
        api.get('/community-sources'),
        api.get('/moderation-reports'),
        api.get('/audio/path'),
        api.get('/audio/stats'),
        api.get('/audio/graph'),
      ]);
      _health = _map(values[0]);
      _node = _map(values[1]);
      _nodeConfig = _map(values[2]);
      _pins = _list(values[3]);
      _transfers = _list(values[4]);
      _plugins = _list(values[5]);
      _communitySources = _list(values[6]);
      _moderationReports = _list(values[7]);
      _audioPath = _map(values[8]);
      _audioStats = _map(values[9]);
      _audioGraph = _map(values[10]);
    } catch (error) {
      _error = error.toString();
    } finally {
      api.close();
      await Future.wait([_refreshBrowserNode(), _refreshDeviceNode()]);
      _loading = false;
      notifyListeners();
    }
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
    final api = ControlApi(endpoint: _endpoint, token: _token);
    try {
      _lastResult = await operation(api);
    } catch (error) {
      _error = error.toString();
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

  void _ensurePolling() {
    if (!connected || _poller != null) return;
    _poller = Timer.periodic(
      const Duration(seconds: 5),
      (_) => unawaited(refresh()),
    );
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
    _poller?.cancel();
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
