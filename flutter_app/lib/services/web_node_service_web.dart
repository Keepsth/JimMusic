import 'dart:convert';
import 'dart:js_interop';
import 'dart:js_interop_unsafe';

@JS('jimmusicHeliaStart')
external JSPromise<JSString> _startHelia();

@JS('jimmusicHeliaStop')
external JSPromise<JSString> _stopHelia();

@JS('jimmusicHeliaStatus')
external JSPromise<JSString> _heliaStatus();

@JS('jimmusicHeliaConnect')
external JSPromise<JSString> _connectHelia(JSString address);

class WebNodeService {
  bool get available => globalContext.has('jimmusicHeliaStart');

  Future<Map<String, dynamic>?> start() => _decode(_startHelia());

  Future<Map<String, dynamic>?> status() => _decode(_heliaStatus());

  Future<Map<String, dynamic>?> connect(String address) =>
      _decode(_connectHelia(address.trim().toJS));

  Future<void> stop() async {
    if (available) await _decode(_stopHelia());
  }

  Future<Map<String, dynamic>?> _decode(JSPromise<JSString> promise) async {
    final raw = (await promise.toDart).toDart;
    final envelope = jsonDecode(raw);
    if (envelope is! Map<String, dynamic>) {
      throw const FormatException('浏览器节点响应格式无效');
    }
    if (envelope['ok'] != true) {
      throw StateError('${envelope['error'] ?? '浏览器节点操作失败'}');
    }
    final value = envelope['value'];
    return value is Map<String, dynamic> ? value : null;
  }
}
