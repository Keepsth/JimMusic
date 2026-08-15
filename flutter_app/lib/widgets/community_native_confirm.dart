import 'package:flutter/material.dart';

/// PLG-007：社区原生高级授权的桌面二次确认。
///
/// 社区原生代码默认拒绝；勾选“允许社区原生制品”后必须在安装前
/// 再次确认，并展示持续警告文案。返回 true 表示用户确认继续。
Future<bool> confirmCommunityNative(
  BuildContext context, {
  required String pluginName,
}) async {
  final confirmed = await showDialog<bool>(
    context: context,
    builder: (dialogContext) => AlertDialog(
      icon: Icon(
        Icons.warning_amber_rounded,
        color: Theme.of(dialogContext).colorScheme.error,
      ),
      title: const Text('社区原生高级授权'),
      content: Text(
        '你即将以「社区原生高级授权」安装插件 $pluginName，'
        '其代码来自社区且未经官方审查。\n\n'
        '运行环境：受限目录 + 能力句柄沙箱，但仍可能消耗 CPU/内存。\n'
        '持续警告：该插件在插件列表中将永久标记“社区原生”提醒。',
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(dialogContext, false),
          child: const Text('取消'),
        ),
        FilledButton(
          style: FilledButton.styleFrom(
            backgroundColor: Theme.of(dialogContext).colorScheme.error,
          ),
          onPressed: () => Navigator.pop(dialogContext, true),
          child: const Text('我已了解，继续安装'),
        ),
      ],
    ),
  );
  return confirmed == true;
}

/// PLG-007：插件列表中对社区原生插件的持续警告条目。
class CommunityNativeWarningTile extends StatelessWidget {
  const CommunityNativeWarningTile({super.key});

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return ListTile(
      leading: Icon(Icons.warning_amber_rounded, color: scheme.error),
      title: const Text('社区原生高级授权'),
      subtitle: const Text('代码来源为社区，未经官方审查；持续警告，可在配置中随时撤销权限。'),
    );
  }
}
