import 'dart:async';

import 'package:flutter_app/providers/control_plane_provider.dart';
import 'package:flutter_app/services/control_api.dart';
import 'package:flutter_app/services/control_api_sse.dart';
import 'package:flutter_app/services/control_api_types.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';

/// 挂起的假控制面：post 挂起直到 close()，用于验证取消语义（UI-010）。
class _HangingApi extends ControlApi {
  _HangingApi() : super(endpoint: 'http://127.0.0.1:9/v1', token: 'test');

  final Completer<dynamic> pending = Completer<dynamic>();
  bool closed = false;

  @override
  Future<dynamic> post(
    String path, [
    Object? body,
    Map<String, String>? headers,
  ]) => pending.future;

  @override
  Future<dynamic> put(String path, Object? body) => pending.future;

  @override
  Future<dynamic> get(String path) async => const <dynamic>[];

  @override
  Stream<SseEvent> events({int? after}) async* {}

  @override
  void close() {
    closed = true;
    if (!pending.isCompleted) {
      pending.completeError(
        ControlApiException("连接已关闭", statusCode: 0),
      );
    }
  }
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  setUp(() {
    SharedPreferences.setMockInitialValues({});
  });

  test('取消当前操作：挂起请求以“操作已取消”结束', () async {
    final api = _HangingApi();
    final provider = ControlPlaneProvider()
      ..debugApiFactory = (endpoint, token) => api;
    addTearDown(provider.dispose);

    // 发起一个挂起的 mutation。
    final pending = provider.configureNode({'max_concurrent_transfers': 2});
    expect(provider.loading, isTrue);

    // 取消 → close() → 请求以取消语义结束。
    await provider.cancelCurrentOperation();
    expect(api.closed, isTrue);
    await pending;
    expect(provider.loading, isFalse);
    expect(provider.error, '操作已取消');
    expect(provider.userErrorText, contains('操作已取消'));
    expect(provider.userErrorText, contains('重新发起'));
  });
}
