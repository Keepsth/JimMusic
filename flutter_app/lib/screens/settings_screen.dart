import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../providers/audio_output_provider.dart';
import '../providers/music_player_provider.dart';
import '../providers/theme_provider.dart';

/// 设置页：主题切换、自定义强调色与音频输出后端选择。
class SettingsScreen extends StatelessWidget {
  const SettingsScreen({super.key});

  /// 极客风预设强调色（霓虹色系）。
  static const _presetColors = <Color>[
    Color(0xFF33FF66), // 霓虹绿
    Color(0xFF33CCFF), // 霓虹青
    Color(0xFFFFCC33), // 琥珀
    Color(0xFFFF66CC), // 洋红
    Color(0xFFFF6633), // 红橙
    Color(0xFF9966FF), // 紫
  ];

  @override
  Widget build(BuildContext context) {
    final theme = context.watch<ThemeProvider>();

    return Scaffold(
      appBar: AppBar(title: const Text('设置')),
      body: ListView(
        padding: const EdgeInsets.all(16),
        children: [
          const _SectionTitle('主题模式'),
          RadioGroup<ThemeModeType>(
            groupValue: theme.mode,
            onChanged: (v) {
              if (v != null) theme.setMode(v);
            },
            child: Column(
              children: [
                RadioListTile<ThemeModeType>(
                  title: const Text('深色'),
                  value: ThemeModeType.dark,
                ),
                RadioListTile<ThemeModeType>(
                  title: const Text('浅色'),
                  value: ThemeModeType.light,
                ),
                RadioListTile<ThemeModeType>(
                  title: const Text('跟随系统'),
                  value: ThemeModeType.system,
                ),
              ],
            ),
          ),
          const Divider(height: 32),
          const _SectionTitle('自定义强调色'),
          const SizedBox(height: 8),
          Wrap(
            spacing: 12,
            children: _presetColors.map((color) {
              final selected = theme.accentColor == color;
              return GestureDetector(
                onTap: () => theme.setAccentColor(color),
                child: CircleAvatar(
                  radius: 20,
                  backgroundColor: color,
                  child: selected
                      ? const Icon(Icons.check, color: Colors.white)
                      : null,
                ),
              );
            }).toList(),
          ),
          const Divider(height: 32),
          const _SectionTitle('音频输出'),
          const SizedBox(height: 4),
          // 仅列出当前平台已接入真实播放链路的输出后端。
          Consumer<AudioOutputProvider>(
            builder: (context, output, _) {
              return Column(
                children: [
                  if (output.error != null)
                    ListTile(
                      leading: Icon(
                        Icons.error_outline,
                        color: Theme.of(context).colorScheme.error,
                      ),
                      title: Text(output.error!),
                    ),
                  RadioGroup<String>(
                    groupValue: output.activeId,
                    onChanged: (v) {
                      if (v != null) output.activate(v);
                    },
                    child: Column(
                      children: output.devices.map((device) {
                        return RadioListTile<String>(
                          title: Text(device.name),
                          subtitle: Text(
                            device.description,
                            style: const TextStyle(fontSize: 12),
                          ),
                          value: device.id,
                        );
                      }).toList(),
                    ),
                  ),
                  if (output.session case final session?)
                    _OutputSessionCard(session: session),
                ],
              );
            },
          ),
          const Divider(height: 32),
          const _SectionTitle('切歌过渡'),
          Consumer2<MusicPlayerProvider, AudioOutputProvider>(
            builder: (context, player, output, _) {
              final seconds = player.crossfadeMilliseconds / 1000.0;
              final nativeReady =
                  output.error == null && player.supportsNativeTransitions;
              return Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  ListTile(
                    contentPadding: EdgeInsets.zero,
                    leading: Icon(
                      seconds == 0 ? Icons.skip_next : Icons.multitrack_audio,
                    ),
                    title: Text(
                      seconds == 0
                          ? '无缝切歌'
                          : '交叉淡化 ${seconds.toStringAsFixed(1)} 秒',
                    ),
                    subtitle: Text(
                      nativeReady
                          ? '由 Rust Core 双时间线混音器执行'
                          : '配置已保存；原生音频输出可用后生效',
                    ),
                  ),
                  Slider(
                    value: seconds,
                    min: 0,
                    max: 12,
                    divisions: 24,
                    label: seconds == 0
                        ? '无缝'
                        : '${seconds.toStringAsFixed(1)} 秒',
                    onChanged: (value) => player.setCrossfade(
                      Duration(milliseconds: (value * 1000).round()),
                    ),
                  ),
                  SwitchListTile(
                    contentPadding: EdgeInsets.zero,
                    title: const Text('等功率曲线'),
                    subtitle: const Text('交叉淡化时使主观响度更平稳'),
                    value: player.crossfadeEqualPower,
                    onChanged: seconds == 0
                        ? null
                        : (value) => player.setCrossfade(
                            Duration(
                              milliseconds: player.crossfadeMilliseconds,
                            ),
                            equalPower: value,
                          ),
                  ),
                  if (!nativeReady)
                    const Padding(
                      padding: EdgeInsets.only(top: 4),
                      child: Text(
                        'Web 与 just_audio 回退路径不执行交叉淡化。',
                        style: TextStyle(fontSize: 12),
                      ),
                    ),
                ],
              );
            },
          ),
        ],
      ),
    );
  }
}

class _SectionTitle extends StatelessWidget {
  final String text;

  const _SectionTitle(this.text);

  @override
  Widget build(BuildContext context) {
    return Text(
      text,
      style: const TextStyle(fontSize: 16, fontWeight: FontWeight.bold),
    );
  }
}

class _OutputSessionCard extends StatelessWidget {
  final Map<String, dynamic> session;

  const _OutputSessionCard({required this.session});

  @override
  Widget build(BuildContext context) {
    final format = session['negotiated_format'] as Map<String, dynamic>? ?? {};
    final deviceBuffer = session['device_buffer_frames'];
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const Text(
              '已打开会话证据',
              style: TextStyle(fontWeight: FontWeight.bold),
            ),
            const SizedBox(height: 8),
            Text('设备：${session['device_name'] ?? session['device_id']}'),
            Text('驱动：${session['driver']} · ${session['share_mode']}'),
            Text(
              '协商格式：${format['sample_rate']} Hz / '
              '${format['channels']} ch / ${format['bit_depth']} bit',
            ),
            Text(
              '缓冲：软件 ${session['software_buffer_frames']} 帧 / '
              '设备 ${deviceBuffer ?? '未由驱动暴露'}',
            ),
            Text('时钟：${session['clock_source']}'),
            Text('来源：${session['capability_source']}'),
          ],
        ),
      ),
    );
  }
}
