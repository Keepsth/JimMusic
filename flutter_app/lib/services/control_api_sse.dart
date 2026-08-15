import 'dart:convert';

/// 控制面 `/v1/events` SSE 事件帧。
///
/// 服务端约定：事件类型在 `event:` 行，单调 sequence 在 `id:` 行，
/// JSON 载荷在 `data:` 行；keep-alive 为 `:` 开头的注释行。
class SseEvent {
  const SseEvent({this.eventType, this.sequence, required this.data});

  /// `event:` 字段（如 `transfer.state_changed`）。
  final String? eventType;

  /// `id:` 字段解析出的单调 sequence；服务器也可能把 sequence 放在载荷里。
  final int? sequence;

  /// 合并后的 `data:` 字段原始文本。
  final String data;

  /// 解析为 JSON 对象；失败返回 null。
  Map<String, dynamic>? get json {
    try {
      final decoded = jsonDecode(data);
      return decoded is Map<String, dynamic> ? decoded : null;
    } catch (_) {
      return null;
    }
  }

  @override
  String toString() => 'SseEvent($eventType, seq=$sequence, data=$data)';
}

/// 增量 SSE 解析器：按块喂入字节串，产出完整事件帧。
///
/// - 支持 `\r\n`、`\n`、`\r` 三种行结束符，行可跨块边界；
/// - `:` 开头行按规范视为注释（含 keep-alive），忽略；
/// - 空行分派一个事件；`event:`/`id:`/`data:` 之外的行忽略；
/// - 单个事件数据超过 [maxEventBytes] 时抛 [FormatException]，防止恶意流耗尽内存。
class SseParser {
  SseParser({this.maxEventBytes = 1024 * 1024});

  /// 单个事件 data 缓冲上限（默认 1 MiB）。
  final int maxEventBytes;

  String _buffer = '';
  String? _eventType;
  String? _id;
  final List<String> _dataLines = [];
  int _dataBytes = 0;

  /// 喂入新块，返回本次完整产出的事件帧。
  List<SseEvent> add(String chunk) {
    _buffer += chunk;
    final events = <SseEvent>[];
    while (true) {
      final ending = _findLineEnding(_buffer);
      if (ending < 0) break;
      final line = _buffer.substring(0, ending);
      final advance = ending + (_takeLineEnding(_buffer, ending) ? 2 : 1);
      _buffer = _buffer.substring(advance);
      final event = _consumeLine(line);
      if (event != null) events.add(event);
    }
    return events;
  }

  /// 找到第一个行结束符位置；没有则返回 -1。
  static int _findLineEnding(String text) {
    for (var i = 0; i < text.length; i++) {
      final code = text.codeUnitAt(i);
      if (code == 0x0a || code == 0x0d) return i;
    }
    return -1;
  }

  /// 行结束符是否为 `\r\n` 两字节形式。
  static bool _takeLineEnding(String text, int index) =>
      text.codeUnitAt(index) == 0x0d &&
      index + 1 < text.length &&
      text.codeUnitAt(index + 1) == 0x0a;

  SseEvent? _consumeLine(String line) {
    if (line.isEmpty) return _dispatch();
    if (line.startsWith(':')) return null; // 注释 / keep-alive

    if (line.startsWith('event:')) {
      _eventType = _fieldValue(line, 'event:');
    } else if (line.startsWith('id:')) {
      _id = _fieldValue(line, 'id:');
    } else if (line.startsWith('data:')) {
      final value = _fieldValue(line, 'data:');
      _dataBytes += value.length;
      if (_dataBytes > maxEventBytes) {
        _resetFields();
        throw const FormatException('SSE 事件数据超过大小上限');
      }
      _dataLines.add(value);
    }
    // 其它字段（retry 等）忽略。
    return null;
  }

  /// 去掉 `field:` 前缀并按规范去掉冒号后单个前导空格。
  static String _fieldValue(String line, String field) {
    var value = line.substring(field.length);
    if (value.startsWith(' ')) value = value.substring(1);
    return value;
  }

  SseEvent? _dispatch() {
    final hasFields = _eventType != null || _id != null || _dataLines.isNotEmpty;
    if (!hasFields) return null;
    final event = SseEvent(
      eventType: _eventType,
      sequence: _id == null ? null : int.tryParse(_id!),
      data: _dataLines.join('\n'),
    );
    _resetFields();
    return event;
  }

  void _resetFields() {
    _eventType = null;
    _id = null;
    _dataLines.clear();
    _dataBytes = 0;
  }
}
