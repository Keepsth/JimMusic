// ignore_for_file: deprecated_member_use, avoid_web_libraries_in_flutter

import 'dart:convert';
import 'dart:html' as html;
import 'dart:js_interop';
import 'dart:typed_data';

import 'package:web/web.dart' as web;

import 'control_api_sse.dart';
import 'control_api_types.dart';

/// `ReadableStreamReader`（JSObject）上我们实际需要的成员。
extension type _SseReader._(JSObject _) implements JSObject {
  external JSPromise<web.ReadableStreamReadResult> read();
  external void releaseLock();
}

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

  /// 订阅控制面事件流（SSE）。使用 fetch 流式读取 body：
  /// EventSource 无法携带 Authorization 头，不能满足控制面鉴权。
  ///
  /// 流在服务端关闭或连接断开时结束；消费者应重连并以
  /// `snapshot.required` + sequence 缺口检测驱动快照重读。
  Stream<SseEvent> events({int? after}) async* {
    final query = after == null ? '' : '?after=$after';
    final headers = <String, String>{};
    if (token.trim().isNotEmpty) {
      headers['authorization'] = 'Bearer ${token.trim()}';
    }
    web.Response response;
    try {
      response = await web.window
          .fetch(
            '$endpoint/events$query'.toJS,
            web.RequestInit(
              method: 'GET',
              headers: headers.jsify()! as JSObject,
            ),
          )
          .toDart;
    } catch (error) {
      throw ControlApiException('事件流连接失败（请检查 CORS 与地址）：$error');
    }
    if (response.status < 200 || response.status >= 300) {
      String message = '事件流连接失败';
      try {
        final text = (await response.text().toDart).toDart;
        if (text.trim().isNotEmpty) message = text.trim();
      } catch (_) {}
      throw ControlApiException(message, statusCode: response.status);
    }
    final body = response.body;
    if (body == null) {
      throw const ControlApiException('事件流响应没有 body');
    }
    final reader = body.getReader() as _SseReader;
    final parser = SseParser();
    try {
      while (true) {
        final result = await reader.read().toDart;
        if (result.done) break;
        final decoded = result.value?.dartify();
        if (decoded is ByteBuffer) {
          final bytes = Uint8List.view(decoded);
          if (bytes.isNotEmpty) {
            for (final event
                in parser.add(utf8.decode(bytes, allowMalformed: true))) {
              yield event;
            }
          }
        } else if (decoded is Uint8List && decoded.isNotEmpty) {
          for (final event
              in parser.add(utf8.decode(decoded, allowMalformed: true))) {
            yield event;
          }
        }
      }
    } finally {
      reader.releaseLock();
    }
  }

  void close() {}
}
