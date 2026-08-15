import 'dart:convert';
import 'dart:io';

import 'control_api_types.dart';

class ControlApi {
  final String endpoint;
  final String token;
  final HttpClient _client = HttpClient()
    ..connectionTimeout = const Duration(seconds: 5);

  ControlApi({required String endpoint, required this.token})
    : endpoint = normalizeControlEndpoint(endpoint);

  Future<dynamic> get(String path) => _request('GET', path);
  Future<dynamic> post(
    String path, [
    Object? body,
    Map<String, String>? headers,
  ]) => _request('POST', path, body: body, headers: headers);
  Future<dynamic> put(String path, Object? body) =>
      _request('PUT', path, body: body);
  Future<dynamic> patch(String path, Object? body) =>
      _request('PATCH', path, body: body);
  Future<dynamic> delete(String path) => _request('DELETE', path);

  Future<dynamic> _request(
    String method,
    String path, {
    Object? body,
    Map<String, String>? headers,
  }) async {
    final request = await _client
        .openUrl(method, Uri.parse('$endpoint$path'))
        .timeout(const Duration(seconds: 10));
    request.followRedirects = false;
    request.headers.contentType = ContentType.json;
    if (token.trim().isNotEmpty) {
      request.headers.set(
        HttpHeaders.authorizationHeader,
        'Bearer ${token.trim()}',
      );
    }
    final effectiveHeaders = <String, String>{...?headers};
    if (method != 'GET') {
      effectiveHeaders.putIfAbsent(
        'idempotency-key',
        () => 'flutter-${DateTime.now().microsecondsSinceEpoch}',
      );
    }
    effectiveHeaders.forEach(request.headers.set);
    if (body != null) request.write(jsonEncode(body));
    final response = await request.close().timeout(const Duration(seconds: 20));
    final bytes = <int>[];
    await for (final chunk in response) {
      bytes.addAll(chunk);
      if (bytes.length > 4 * 1024 * 1024) {
        throw const ControlApiException('控制面响应超过 4 MiB 限制');
      }
    }
    final text = utf8.decode(bytes, allowMalformed: true);
    Object? decoded;
    if (text.isNotEmpty) {
      try {
        decoded = jsonDecode(text);
      } catch (_) {
        decoded = text;
      }
    }
    if (response.statusCode < 200 || response.statusCode >= 300) {
      final message = decoded is Map<String, dynamic>
          ? (decoded['message'] ?? decoded['error'] ?? text).toString()
          : text;
      throw ControlApiException(
        message.isEmpty ? '控制面请求失败' : message,
        statusCode: response.statusCode,
        body: decoded,
      );
    }
    return decoded;
  }

  void close() => _client.close(force: true);
}
