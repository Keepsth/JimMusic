import 'control_api_sse.dart';
import 'control_api_types.dart';

class ControlApi {
  ControlApi({required String endpoint, required String token});

  Never _unsupported() => throw const ControlApiException('当前平台不支持控制面网络请求');
  Future<dynamic> get(String path) async => _unsupported();
  Future<dynamic> post(
    String path, [
    Object? body,
    Map<String, String>? headers,
  ]) async => _unsupported();
  Future<dynamic> put(String path, Object? body) async => _unsupported();
  Future<dynamic> patch(String path, Object? body) async => _unsupported();
  Future<dynamic> delete(String path) async => _unsupported();
  Stream<SseEvent> events({int? after}) async* {}
  void close() {}
}
