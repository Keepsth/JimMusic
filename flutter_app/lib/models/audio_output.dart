/// 音频输出后端（对应需求 3.3「音频输出插件」）。
///
/// 每个后端即一个输出插件（`PluginKind::AudioOutput`），由 Core 按能力元数据校验、
/// 经插件管理器 `/outputs` 目录列举并可运行时切换。前端据此渲染「输出设备」选择界面。
class AudioOutputDevice {
  /// 后端标识（与插件管理器 `kind=output` 记录名一致）。
  final String id;

  /// 展示名。
  final String name;

  /// 一句话能力描述。
  final String description;

  const AudioOutputDevice({
    required this.id,
    required this.name,
    required this.description,
  });

  /// 仓库内已定义的后端。平台 provider 只暴露已接入当前播放链路的项目。
  static const List<AudioOutputDevice> backends = [
    AudioOutputDevice(
      id: 'auto',
      name: '自动（系统默认）',
      description: '由宿主系统选择默认输出设备',
    ),
    AudioOutputDevice(
      id: 'null',
      name: 'null（参考后端）',
      description: '无硬件输出，用于测试/演示完整 ABI',
    ),
    AudioOutputDevice(
      id: 'system',
      name: '系统原生输出',
      description: 'CPAL：ALSA / WASAPI / CoreAudio / AAudio / AudioUnit',
    ),
    AudioOutputDevice(
      id: 'web-audio',
      name: 'Web Audio（开发中）',
      description: '参考 ABI 已有，Flutter 播放链路尚未接入',
    ),
  ];

  /// 按 id 查找后端；未知 id 返回 `null`。
  static AudioOutputDevice? byId(String id) {
    for (final d in backends) {
      if (d.id == id) return d;
    }
    return null;
  }
}
