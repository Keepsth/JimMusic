/// DST-002：客户端按能力、质量和网络策略选择 rendition。
///
/// 输入为后端 `TrackSourceV1` JSON 列表（`/v1/library/tracks` 的 `sources`），
/// 输出播放用内容 CID；无可用候选时返回 null（调用方回退默认 CID 或报错）。
library;

/// 浏览器保守支持清单（容器或编解码命中其一即可）。
const webSupportedContainers = {
  'mp3',
  'm4a',
  'aac',
  'ogg',
  'opus',
  'wav',
  'flac',
  'webm',
};

const webSupportedCodecs = {'mp3', 'aac', 'opus', 'vorbis', 'pcm', 'flac'};

/// 该 source 在当前平台是否“可解码”。
bool _platformSupported(Map<String, dynamic> source, bool isWeb) {
  if (!isWeb) return true; // 原生端 just_audio 覆盖主流容器/编解码。
  final container = '${source['container'] ?? ''}'.toLowerCase();
  final codec = '${source['codec'] ?? ''}'.toLowerCase();
  return webSupportedContainers.contains(container) ||
      webSupportedCodecs.contains(codec);
}

/// 选择最优 rendition 的内容 CID：
/// - 平台能力：Web 端剔除浏览器无法解码的容器/编解码，原生端全量；
/// - 质量：非计量网络优先 lossless 与 original；
/// - 网络策略：计量网络（preferCompact）优先有损流式小体积。
///
/// 候选先取 `availability == "available"`，全部不可用时回退到任一
/// 带 CID 的 source（传输任务仍会在下载后校验 CID）。
String? selectBestRenditionCid(
  List<Map<String, dynamic>> sources, {
  required bool isWeb,
  bool preferCompact = false,
}) {
  final withCid = sources
      .where(
        (source) =>
            source['content_cid'] is String &&
            '${source['content_cid']}'.isNotEmpty,
      )
      .toList();
  if (withCid.isEmpty) return null;
  var pool = withCid
      .where((source) => source['availability'] == 'available')
      .toList();
  if (pool.isEmpty) pool = withCid;
  pool.sort(
    (a, b) => _rank(
      b,
      isWeb: isWeb,
      preferCompact: preferCompact,
    ).compareTo(_rank(a, isWeb: isWeb, preferCompact: preferCompact)),
  );
  return pool.first['content_cid'] as String?;
}

int _rank(
  Map<String, dynamic> source, {
  required bool isWeb,
  required bool preferCompact,
}) {
  var score = 0;
  if (_platformSupported(source, isWeb)) score += 1000;
  if (preferCompact) {
    // 计量网络：有损且可流式的优先，按体积（每 MiB）降权。
    if (source['lossless'] == false && source['streamable'] == true) {
      score += 400;
    }
    final bytes = (source['byte_length'] as num?)?.toInt() ?? 0;
    if (bytes > 0) score -= bytes ~/ (1024 * 1024);
  } else {
    if (source['lossless'] == true) score += 400;
    if (source['original'] == true) score += 200;
  }
  return score;
}
