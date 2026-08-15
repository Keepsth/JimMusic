class ControlApiException implements Exception {
  final int? statusCode;
  final String message;
  final Object? body;

  const ControlApiException(this.message, {this.statusCode, this.body});

  @override
  String toString() =>
      statusCode == null ? message : 'HTTP $statusCode: $message';
}

/// API-004：稳定机器码 → 用户可读消息的统一映射结果。
class ApiErrorText {
  const ApiErrorText(this.message, {this.suggestion});

  final String message;
  final String? suggestion;

  String get full => suggestion == null ? message : '$message（$suggestion）';
}

/// 把控制面错误（[ControlApiException] 携带的 ErrorEnvelopeV1 或普通文本）
/// 映射为一致的本地化文案。所有 UI 入口应使用同一函数，保证七端语义一致。
ApiErrorText apiErrorText(Object error) {
  if ('$error' == '操作已取消' || error is String && error == '操作已取消') {
    return const ApiErrorText('操作已取消', suggestion: '可以重新发起操作');
  }
  if (error is ControlApiException) {
    if (error.statusCode == 401) {
      return const ApiErrorText(
        '未授权：控制面令牌缺失或不正确',
        suggestion: '请重新设置控制面连接令牌',
      );
    }
    final body = error.body;
    if (body is Map<String, dynamic>) {
      final code = '${body['code'] ?? ''}';
      final subsystem = '${body['subsystem'] ?? ''}';
      final retryable = body['retryable'] == true;
      final unsupported = body['unsupported_reason'] as String?;
      final message = _localizeCode(code, subsystem, unsupported);
      return ApiErrorText(
        message,
        suggestion: retryable ? '可以稍后重试' : null,
      );
    }
    return ApiErrorText(error.message);
  }
  return ApiErrorText('$error');
}

String _localizeCode(String code, String subsystem, String? unsupported) {
  final where = subsystem.isEmpty ? '' : '（$subsystem）';
  switch (code) {
    case 'unsupported':
      return '当前能力不受支持$where：${unsupported ?? '无更多信息'}';
    case 'not_found':
      return '资源不存在$where';
    case 'conflict':
      return '操作与当前状态冲突$where';
    case 'payload_too_large':
      return '数据超过大小上限$where';
    case 'unavailable':
      return '网络暂时不可用$where：操作已加入离线队列，恢复后自动重试';
    case 'idempotency_key_required':
    case 'idempotency_conflict':
      return '请求幂等键缺失或重复$where';
    case 'paused_wifi_only':
      return '任务已暂停：当前网络不是 Wi-Fi';
    case 'paused_metered_network':
      return '任务已暂停：当前是蜂窝网络且未允许计量网络';
    case 'paused_cellular_quota':
      return '任务已暂停：蜂窝额度已用完';
    case 'invalid_request':
      return '请求无效$where';
    default:
      return '操作失败$where（$code）';
  }
}

/// 网络策略暂停码 → 恢复提示（DST-010/NOD-006 的 UI 文案一致性）。
String? networkPauseHint(String code) {
  switch (code) {
    case 'paused_wifi_only':
      return '连接 Wi-Fi 或修改任务的网络策略后会自动恢复';
    case 'paused_metered_network':
      return '在节点设置中允许计量网络，或切换网络后自动恢复';
    case 'paused_cellular_quota':
      return '提高任务的蜂窝额度或切换网络后会自动恢复';
    default:
      return null;
  }
}

String normalizeControlEndpoint(String endpoint) {
  var value = endpoint.trim();
  while (value.endsWith('/')) {
    value = value.substring(0, value.length - 1);
  }
  if (!value.endsWith('/v1')) value = '$value/v1';
  final uri = Uri.tryParse(value);
  if (uri == null || !uri.hasScheme || uri.host.isEmpty) {
    throw const ControlApiException('控制面地址无效');
  }
  if (uri.scheme != 'https' && uri.scheme != 'http') {
    throw const ControlApiException('控制面仅支持 HTTPS 或回环 HTTP');
  }
  final loopback =
      uri.host == '127.0.0.1' || uri.host == '::1' || uri.host == 'localhost';
  if (uri.scheme == 'http' && !loopback) {
    throw const ControlApiException('非回环控制面必须使用 HTTPS');
  }
  return value;
}
