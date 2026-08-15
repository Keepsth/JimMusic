// ignore_for_file: deprecated_member_use, avoid_web_libraries_in_flutter

import 'dart:convert';
import 'dart:html' as html;

import 'control_api_types.dart';

class ControlApi {
  final String endpoint;
  final String token;

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
    final requestHeaders = <String, String>{
      'content-type': 'application/json',
      ...?headers,
    };
    if (method != 'GET') {
      requestHeaders.putIfAbsent(
        'idempotency-key',
        () => 'flutter-${DateTime.now().microsecondsSinceEpoch}',
      );
    }
    if (token.trim().isNotEmpty) {
      requestHeaders['authorization'] = 'Bearer ${token.trim()}';
    }
    html.HttpRequest response;
    try {
      response = await html.HttpRequest.request(
        '$endpoint$path',
        method: method,
        requestHeaders: requestHeaders,
        sendData: body == null ? null : jsonEncode(body),
      ).timeout(const Duration(seconds: 20));
    } catch (error) {
      throw ControlApiException('控制面连接失败（请检查 CORS 与地址）：$error');
    }
    final text = response.responseText ?? '';
    Object? decoded;
    if (text.isNotEmpty) {
      try {
        decoded = jsonDecode(text);
      } catch (_) {
        decoded = text;
      }
    }
    if (response.status! < 200 || response.status! >= 300) {
      final message = decoded is Map<String, dynamic>
          ? (decoded['message'] ?? decoded['error'] ?? text).toString()
          : text;
      throw ControlApiException(
        message,
        statusCode: response.status,
        body: decoded,
      );
    }
    return decoded;
  }

  void close() {}
}
