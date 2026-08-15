import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../models/music.dart';
import '../providers/control_plane_provider.dart';
import '../providers/music_player_provider.dart';

/// 强制策略（不可本地覆盖，精确打开直接拒绝）。
const _mandatoryActions = {'block', 'revoke'};

/// 非强制策略（可在详情入口本地覆盖）。
const _overridableActions = {'warn', 'demote', 'hide'};

/// COM-006：搜索入口应用策略——hide/block/revoke 从结果移除，
/// demote 排到末尾，warn 保留并展示标记。
List<Music> applyPolicyToSearch(List<Music> results) {
  final kept = <Music>[];
  final demoted = <Music>[];
  for (final music in results) {
    final action = music.policyAction;
    if (action == 'hide' || action == 'block' || action == 'revoke') {
      continue;
    }
    if (action == 'demote') {
      demoted.add(music);
    } else {
      kept.add(music);
    }
  }
  return [...kept, ...demoted];
}

/// 策略动作对应的中文标签与图标。
(String, IconData) policyPresentation(String? action) => switch (action) {
  'warn' => ('警告', Icons.warning_amber_outlined),
  'demote' => ('降权', Icons.arrow_downward_outlined),
  'hide' => ('隐藏', Icons.visibility_off_outlined),
  'block' => ('阻止', Icons.block_outlined),
  'revoke' => ('撤销', Icons.gpp_bad_outlined),
  _ => ('', Icons.gpp_good_outlined),
};

/// COM-006：精确打开入口——block/revoke 直接拒绝并解释，
/// warn 需要用户确认，其余动作执行 [onPlay]（默认本地播放）。
Future<void> playTrackWithPolicy(
  BuildContext context,
  Music music, {
  Future<void> Function()? onPlay,
}) async {
  final player = context.read<MusicPlayerProvider>();
  final action = music.policyAction;
  if (_mandatoryActions.contains(action)) {
    await showDialog<void>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        title: const Text('社区策略阻止播放'),
        content: Text(
          '该曲目被社区策略「${policyPresentation(action).$1}」：'
          '${music.policyReason ?? '未提供原因'}。'
          '该决策不可本地覆盖。',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(dialogContext),
            child: const Text('知道了'),
          ),
        ],
      ),
    );
    return;
  }
  if (action == 'warn') {
    final proceed = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        title: const Text('社区策略警告'),
        content: Text(
          '该曲目被社区标记：${music.policyReason ?? '未提供原因'}。'
          '仍要继续播放吗？',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(dialogContext, false),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(dialogContext, true),
            child: const Text('继续播放'),
          ),
        ],
      ),
    );
    if (proceed != true) return;
  }
  await (onPlay ?? () => player.play(music))();
}

/// COM-006：详情入口——展示曲目元数据、来源与社区策略决策；
/// 非强制动作（warn/demote/hide）可本地覆盖（COM-011）。
Future<void> showTrackDetailDialog(BuildContext context, Music music) async {
  final (label, icon) = policyPresentation(music.policyAction);
  await showDialog<void>(
    context: context,
    builder: (dialogContext) => AlertDialog(
      title: Row(
        children: [
          Expanded(
            child: Text(
              music.title,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
            ),
          ),
          if (music.policyAction != null)
            Tooltip(message: '社区策略：$label', child: Icon(icon)),
        ],
      ),
      content: SizedBox(
        width: 520,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text('艺术家：${music.artist}\n专辑：${music.album}'),
              const SizedBox(height: 8),
              Text(
                'Manifest CID：${music.manifestCid ?? '-'}\n'
                '发布者：${music.publisher ?? '-'}\n'
                'Rendition：${music.renditionCid ?? '-'} · '
                '${music.codec ?? '-'} · ${music.sampleRate ?? '-'} Hz',
              ),
              const SizedBox(height: 8),
              if (music.policyAction != null) ...[
                Text(
                  '社区策略：$label'
                  '${music.policyReason == null ? '' : '（${music.policyReason}）'}',
                  style: TextStyle(color: Theme.of(context).colorScheme.error),
                ),
                Text('来源：${music.policySourceIds.join(', ')}'),
              ],
            ],
          ),
        ),
      ),
      actions: [
        if (music.policyAction != null &&
            _overridableActions.contains(music.policyAction) &&
            music.manifestCid != null)
          TextButton(
            onPressed: () async {
              final control = dialogContext.read<ControlPlaneProvider>();
              final target = music.manifestCid!;
              final confirmed = await showDialog<bool>(
                context: dialogContext,
                builder: (overrideContext) => AlertDialog(
                  title: const Text('本地覆盖策略'),
                  content: const Text(
                    '覆盖后该曲目在本地不再受此非强制策略影响。'
                    '强制策略（block/revoke）不可覆盖。',
                  ),
                  actions: [
                    TextButton(
                      onPressed: () => Navigator.pop(overrideContext, false),
                      child: const Text('取消'),
                    ),
                    FilledButton(
                      onPressed: () => Navigator.pop(overrideContext, true),
                      child: const Text('覆盖'),
                    ),
                  ],
                ),
              );
              if (confirmed != true) return;
              await control.overridePolicy(target, '详情入口本地覆盖');
              if (dialogContext.mounted) Navigator.pop(dialogContext);
            },
            child: const Text('本地覆盖'),
          ),
        TextButton(
          onPressed: () => Navigator.pop(dialogContext),
          child: const Text('关闭'),
        ),
      ],
    ),
  );
}
