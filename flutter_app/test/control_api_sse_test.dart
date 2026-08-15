import 'dart:async';
import 'dart:io';
import 'dart:math' as math;

import 'package:flutter_app/providers/control_plane_provider.dart';
import 'package:flutter_app/services/control_api.dart';
import 'package:flutter_app/services/control_api_sse.dart';
import 'package:flutter_app/services/control_api_types.dart';
import 'package:flutter_test/flutter_test.dart';

/// 注入 Provider 的假控制面：网络路径全部走内存。
class _FakeControlApi extends ControlApi {
  _FakeControlApi(this.eventsController)
    : super(endpoint: 'http://127.0.0.1:9/v1', token: 'test');

  final StreamController<SseEvent> eventsController;
  final List<String> requests = [];
  final List<int?> afterValues = [];
  final List<String> mutations = [];
  final Map<String, dynamic> responses = {'/health': {'status': 'ok'}};

  @override
  Future<dynamic> get(String path) async {
    requests.add(path);
    return responses[path] ?? const <dynamic>[];
  }

  @override
  Future<dynamic> post(
    String path, [
    Object? body,
    Map<String, String>? headers,
  ]) async {
    mutations.add('POST $path $body');
    return {};
  }

  @override
  Future<dynamic> delete(String path) async {
    mutations.add('DELETE $path');
    return {};
  }

  @override
  Stream<SseEvent> events({int? after}) {
    afterValues.add(after);
    return eventsController.stream;
  }

  @override
  void close() {}
}

Future<void> _pumpEventLoop([int milliseconds = 450]) =>
    Future<void>.delayed(Duration(milliseconds: milliseconds));

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  // flutter_test 默认把所有 HTTP 请求替换为 400；本文件需要真实回环
  // HttpServer 验证 SSE 流式通道，因此恢复真实 HttpClient。
  HttpOverrides.global = null;

  group('SseParser', () {
    test('解析 event/id/data 字段', () {
      final parser = SseParser();
      final events = parser.add(
        'event: transfer.state_changed\nid: 7\ndata: {"a":1}\n\n',
      );
      expect(events, hasLength(1));
      expect(events.single.eventType, 'transfer.state_changed');
      expect(events.single.sequence, 7);
      expect(events.single.json, {'a': 1});
    });

    test('合并多行 data、CRLF 与 keep-alive 注释', () {
      final parser = SseParser();
      final events = parser.add(
        ':keep-alive\r\n'
        'event: node.status_changed\r\n'
        'id: 8\r\n'
        'data: {"line":1}\r\n'
        'data: {"line":2}\r\n'
        '\r\n',
      );
      expect(events, hasLength(1));
      expect(events.single.data, '{"line":1}\n{"line":2}');
      expect(events.single.sequence, 8);
    });

    test('事件跨任意块边界仍完整解析', () {
      final parser = SseParser();
      const raw = 'event: audio.xrun\nid: 12\ndata: {"x":true}\n\n';
      final all = <SseEvent>[];
      for (var i = 0; i < raw.length; i += 3) {
        all.addAll(
          parser.add(raw.substring(i, math.min(i + 3, raw.length))),
        );
      }
      expect(all, hasLength(1));
      expect(all.single.eventType, 'audio.xrun');
      expect(all.single.sequence, 12);
      expect(all.single.json, {'x': true});
    });

    test('单独 CR 换行与载荷携带 sequence', () {
      final parser = SseParser();
      final events = parser.add(
        'event: stream.ready\rdata: {"sequence":3}\r\r',
      );
      expect(events, hasLength(1));
      expect(events.single.sequence, isNull);
      expect(events.single.json?['sequence'], 3);
    });

    test('字段名与值之间空格可有可无', () {
      final parser = SseParser();
      final events = parser.add('event:plugin.state_changed\nid:1\ndata:x\n\n');
      expect(events.single.eventType, 'plugin.state_changed');
      expect(events.single.sequence, 1);
      expect(events.single.data, 'x');
    });

    test('数据超过上限抛 FormatException', () {
      final parser = SseParser(maxEventBytes: 4);
      expect(() => parser.add('data: 12345\n\n'), throwsFormatException);
    });

    test('无字段空行不产出事件', () {
      final parser = SseParser();
      expect(parser.add('\n\n'), isEmpty);
      expect(parser.add('data: hello\n\n').single.data, 'hello');
    });
  });

  group('ControlApi.events（真实 HTTP SSE）', () {
    test('按序收到事件，服务端关闭后流结束', () async {
      final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
      server.listen((request) {
        if (request.uri.path == '/v1/events') {
          request.response.statusCode = 200;
          request.response.headers.contentType = ContentType(
            'text',
            'event-stream',
            charset: 'utf-8',
          );
          request.response.write(
            'event: transfer.state_changed\nid: 7\ndata: {"kind":"transfer"}\n\n',
          );
          request.response.write(
            'event: node.status_changed\nid: 8\ndata: {"kind":"node"}\n\n',
          );
          request.response.close();
        } else {
          request.response.statusCode = 404;
          request.response.close();
        }
      });
      addTearDown(() => server.close(force: true));

      final api = ControlApi(
        endpoint: 'http://127.0.0.1:${server.port}/v1',
        token: 'secret',
      );
      final events = await api.events().toList();
      api.close();
      expect(events.map((e) => e.sequence), [7, 8]);
      expect(events.map((e) => e.eventType), [
        'transfer.state_changed',
        'node.status_changed',
      ]);
    });

    test('非 2xx 响应抛 ControlApiException', () async {
      final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
      server.listen((request) {
        request.response.statusCode = 401;
        request.response.write('{"message":"unauthorized"}');
        request.response.close();
      });
      addTearDown(() => server.close(force: true));

      final api = ControlApi(
        endpoint: 'http://127.0.0.1:${server.port}/v1',
        token: 'wrong',
      );
      await expectLater(
        api.events().toList(),
        throwsA(isA<ControlApiException>()),
      );
      api.close();
    });
  });

  group('ControlPlaneProvider SSE 驱动刷新', () {
    late _FakeControlApi fake;
    late ControlPlaneProvider provider;

    setUp(() {
      fake = _FakeControlApi(StreamController<SseEvent>.broadcast());
      provider = ControlPlaneProvider()
        ..debugApiFactory = (endpoint, token) => fake;
    });

    tearDown(() {
      provider.dispose();
      fake.eventsController.close();
    });

    test('健康后订阅事件流；sequence 缺口触发整体重读', () async {
      await provider.refresh();
      expect(fake.afterValues, [null], reason: '首次订阅不带 after');
      fake.requests.clear();

      // 正常事件：定向刷新。
      fake.eventsController.add(
        const SseEvent(eventType: 'transfer.state_changed', sequence: 3, data: '{}'),
      );
      await _pumpEventLoop();
      expect(fake.requests, ['/transfers']);
      fake.requests.clear();

      // sequence 3 → 5 缺口：必须整体重读。
      fake.eventsController.add(
        const SseEvent(eventType: 'transfer.progress', sequence: 5, data: '{}'),
      );
      await _pumpEventLoop();
      expect(fake.requests, contains('/health'));
      expect(fake.requests, contains('/audio/path'));
      expect(fake.requests, contains('/transfers'));
    });

    test('snapshot.required 采纳载荷 sequence 并整体重读', () async {
      await provider.refresh();
      fake.requests.clear();

      fake.eventsController.add(
        const SseEvent(
          eventType: 'snapshot.required',
          sequence: null,
          data: '{"sequence": 41, "event_type": "snapshot.required"}',
        ),
      );
      await _pumpEventLoop();
      expect(fake.requests, contains('/health'));
      fake.requests.clear();

      // 载荷 sequence=41 之后收到 42：无缺口，只做定向刷新。
      fake.eventsController.add(
        const SseEvent(eventType: 'node.status_changed', sequence: 42, data: '{}'),
      );
      await _pumpEventLoop();
      expect(fake.requests, ['/node/status', '/node/config']);
    });

    test('未知事件类型保守整体重读', () async {
      await provider.refresh();
      fake.requests.clear();
      fake.eventsController.add(
        const SseEvent(eventType: 'brand.new.event', sequence: 9, data: '{}'),
      );
      await _pumpEventLoop();
      expect(fake.requests, contains('/health'));
    });

    test('策略查询与本地覆盖走对应端点', () async {
      fake.responses['/policy/bafy-target'] = {
        'target': 'bafy-target',
        'action': 'warn',
        'reason': 'community note',
        'source_ids': ['s1'],
        'expires_at': null,
        'locally_overridden': false,
      };
      await provider.refresh();
      final decision = await provider.policyDecision('bafy-target');
      expect(decision?['action'], 'warn');
      await provider.overridePolicy('bafy-target', '我复核过该内容');
      expect(
        fake.mutations.join('\n'),
        contains('POST /policy/bafy-target/override'),
      );
      await provider.clearPolicyOverride('bafy-target');
      expect(
        fake.mutations.join('\n'),
        contains('DELETE /policy/bafy-target/override'),
      );
    });

    test('关注与取消关注发布者走对应 mutation 端点', () async {
      await provider.refresh();
      await provider.followPublisher('bafy-id', 'jm:publisher', 'Name');
      expect(
        fake.mutations,
        contains(
          allOf(
            contains('POST /community-sources/follows'),
            contains('bafy-id'),
          ),
        ),
      );
      await provider.unfollowPublisher('bafy-id');
      expect(
        fake.mutations,
        contains('DELETE /community-sources/follows/bafy-id'),
      );
    });

    test('流断开后带 after 重连', () async {
      await provider.refresh();
      expect(fake.afterValues, [null]);

      fake.eventsController.add(
        const SseEvent(eventType: 'plugin.state_changed', sequence: 2, data: '{}'),
      );
      await _pumpEventLoop();
      await fake.eventsController.close();
      // 退避 1s 后重连，且带最后 sequence。
      await _pumpEventLoop(1400);
      expect(fake.afterValues, [null, 2]);
    });
  });
}
