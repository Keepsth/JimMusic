import 'package:just_audio/just_audio.dart';

import 'control_api_types.dart';

/// 边下边播（DST-007）的流式 URI 构造。
///
/// `/v1/transfers/{id}/stream` 服务端跟随 part 文件增长推送字节、支持单范围
/// `Range` 请求，任务终结后把已落盘字节服务完并结束。控制面要求 Bearer
/// 鉴权：just_audio 在 headers 非空时走其本地代理转发（不会把令牌交给平台
/// 播放器），并把播放器的 Range 请求一并透传，因此 Seek 只取已下载前缀内
/// 的窗口，不会向服务端索取未下载内容。
///
/// Web 端 just_audio 无法为媒体元素携带请求头，边下边播在 Web 上会得到
/// 结构化 401 失败；Web 走已有的整段字节路径（浏览器限制，无真正增量播放）。
Uri transferStreamUri(String endpoint, String taskId) {
  final normalized = normalizeControlEndpoint(endpoint);
  return Uri.parse(
    '$normalized/transfers/${Uri.encodeComponent(taskId)}/stream',
  );
}

/// 构造边下边播音源；token 为空时不带鉴权头（默认控制面始终要求 token）。
AudioSource transferStreamAudioSource({
  required String endpoint,
  required String token,
  required String taskId,
}) {
  final headers = token.trim().isEmpty
      ? const <String, String>{}
      : <String, String>{'authorization': 'Bearer ${token.trim()}'};
  return AudioSource.uri(transferStreamUri(endpoint, taskId), headers: headers);
}
