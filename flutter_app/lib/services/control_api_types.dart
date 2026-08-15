class ControlApiException implements Exception {
  final int? statusCode;
  final String message;
  final Object? body;

  const ControlApiException(this.message, {this.statusCode, this.body});

  @override
  String toString() =>
      statusCode == null ? message : 'HTTP $statusCode: $message';
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
