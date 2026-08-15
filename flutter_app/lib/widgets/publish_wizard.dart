import 'dart:convert';

import 'package:flutter/material.dart';

/// 构造单个 rendition 条目（UI-004）。
/// 纯函数便于测试；byte_length 必须为正整数（后端校验拒绝 0）。
Map<String, dynamic> buildOriginalRendition({
  required String renditionId,
  required String contentCid,
  required String container,
  required String codec,
  required int sampleRate,
  required int bitDepth,
  required int channels,
  required int byteLength,
  int durationMs = 0,
  bool lossless = true,
  bool original = false,
}) {
  return {
    'rendition_id': renditionId,
    'content_cid': contentCid,
    'container': container,
    'codec': codec,
    'sample_rate': sampleRate,
    'bit_depth': bitDepth,
    'channels': channels,
    'channel_layout': channels == 1 ? 'mono' : 'stereo',
    'duration_ms': durationMs,
    'byte_length': byteLength,
    'lossless': lossless,
    'original': original,
    'streamable': true,
  };
}

/// 发布向导的元数据 + 多 rendition 表单 → MusicManifestV1 JSON（UI-004）。
/// 纯函数便于测试；发布者身份 CID 由签名端填充。
Map<String, dynamic> buildPublishManifest({
  required String title,
  required List<String> artists,
  required String album,
  required String licenseIdentifier,
  required List<String> contentLabels,
  required List<Map<String, dynamic>> renditions,
  int durationMs = 0,
  String language = 'zh',
}) {
  return {
    'schema_version': 1,
    'work_id': 'work-${DateTime.now().millisecondsSinceEpoch}',
    'release_id': 'release-${DateTime.now().millisecondsSinceEpoch}',
    'title': title,
    'artists': artists,
    'album': album,
    'duration_ms': durationMs,
    'language': language,
    'license': {
      'identifier': licenseIdentifier,
      'allows_redistribution': licenseIdentifier != 'all-rights-reserved',
    },
    'content_labels': contentLabels,
    'renditions': renditions,
    'publisher_identity_cid': 'filled-by-signer',
    'created_at': 1,
    'updated_at': 1,
  };
}

/// 单个 rendition 的表单草稿。
class _RenditionDraft {
  _RenditionDraft({required int index, required this.original})
    : renditionId = TextEditingController(
        text: original ? 'original' : 'rendition-${index + 1}',
      ),
      contentCid = TextEditingController(),
      container = TextEditingController(text: 'flac'),
      codec = TextEditingController(text: 'flac'),
      sampleRate = TextEditingController(text: '44100'),
      bitDepth = TextEditingController(text: '24'),
      channels = TextEditingController(text: '2'),
      byteLength = TextEditingController();

  final TextEditingController renditionId;
  final TextEditingController contentCid;
  final TextEditingController container;
  final TextEditingController codec;
  final TextEditingController sampleRate;
  final TextEditingController bitDepth;
  final TextEditingController channels;
  final TextEditingController byteLength;
  bool original;
  bool lossless = true;

  void dispose() {
    for (final controller in [
      renditionId,
      contentCid,
      container,
      codec,
      sampleRate,
      bitDepth,
      channels,
      byteLength,
    ]) {
      controller.dispose();
    }
  }
}

/// 结构化发布向导：元数据 + 多 rendition + 身份解锁，返回
/// `(manifest, displayName, passphrase, bundle)`。
class PublishWizardDialog extends StatefulWidget {
  const PublishWizardDialog({super.key});

  @override
  State<PublishWizardDialog> createState() => _PublishWizardDialogState();
}

class _PublishWizardDialogState extends State<PublishWizardDialog> {
  final title = TextEditingController();
  final artists = TextEditingController();
  final album = TextEditingController();
  final contentLabels = TextEditingController();
  final displayName = TextEditingController();
  final passphrase = TextEditingController();
  final bundle = TextEditingController();
  final renditions = <_RenditionDraft>[
    _RenditionDraft(index: 0, original: true),
  ];
  String licenseIdentifier = 'CC-BY-4.0';
  String? validationError;

  static const licenses = [
    'CC0-1.0',
    'CC-BY-4.0',
    'CC-BY-SA-4.0',
    'all-rights-reserved',
  ];

  @override
  void dispose() {
    for (final controller in [
      title,
      artists,
      album,
      contentLabels,
      displayName,
      passphrase,
      bundle,
    ]) {
      controller.dispose();
    }
    for (final draft in renditions) {
      draft.dispose();
    }
    super.dispose();
  }

  void _addRendition() {
    setState(() {
      renditions.add(
        _RenditionDraft(index: renditions.length, original: false),
      );
    });
  }

  void _removeRendition(int index) {
    if (renditions.length == 1) return;
    setState(() {
      renditions.removeAt(index).dispose();
    });
  }

  (int, int, int, int)? _parse(_RenditionDraft draft) {
    final rate = int.tryParse(draft.sampleRate.text);
    final depth = int.tryParse(draft.bitDepth.text);
    final channels = int.tryParse(draft.channels.text);
    final length = int.tryParse(draft.byteLength.text);
    if (rate == null ||
        rate <= 0 ||
        depth == null ||
        depth <= 0 ||
        channels == null ||
        channels <= 0 ||
        length == null ||
        length <= 0) {
      return null;
    }
    return (rate, depth, channels, length);
  }

  void _submit() {
    if (title.text.trim().isEmpty ||
        artists.text.trim().isEmpty ||
        displayName.text.trim().isEmpty ||
        bundle.text.trim().isEmpty) {
      setState(() => validationError = '标题、艺术家、身份名称与身份包均不能为空');
      return;
    }
    try {
      jsonDecodeBundle();
    } catch (_) {
      setState(() => validationError = '身份包不是有效 JSON');
      return;
    }
    final ids = <String>{};
    var originals = 0;
    final renditionMaps = <Map<String, dynamic>>[];
    for (final draft in renditions) {
      final id = draft.renditionId.text.trim();
      if (id.isEmpty) {
        setState(() => validationError = '每个 rendition 都需要 rendition ID');
        return;
      }
      if (!ids.add(id)) {
        setState(() => validationError = 'rendition ID 重复：$id');
        return;
      }
      if (draft.contentCid.text.trim().isEmpty) {
        setState(() => validationError = 'rendition $id 的内容 CID 不能为空');
        return;
      }
      final parsed = _parse(draft);
      if (parsed == null) {
        setState(() => validationError = 'rendition $id：采样率/位深/声道/字节长度必须是正整数');
        return;
      }
      final (rate, depth, channels, length) = parsed;
      if (draft.original) originals += 1;
      renditionMaps.add(
        buildOriginalRendition(
          renditionId: id,
          contentCid: draft.contentCid.text.trim(),
          container: draft.container.text.trim(),
          codec: draft.codec.text.trim(),
          sampleRate: rate,
          bitDepth: depth,
          channels: channels,
          byteLength: length,
          lossless: draft.lossless,
          original: draft.original,
        ),
      );
    }
    if (originals != 1) {
      setState(() => validationError = '必须且只能标记一个 rendition 为 original');
      return;
    }
    final manifest = buildPublishManifest(
      title: title.text.trim(),
      artists: artists.text
          .split(',')
          .map((v) => v.trim())
          .where((v) => v.isNotEmpty)
          .toList(),
      album: album.text.trim(),
      licenseIdentifier: licenseIdentifier,
      contentLabels: contentLabels.text
          .split(',')
          .map((v) => v.trim())
          .where((v) => v.isNotEmpty)
          .toList(),
      renditions: renditionMaps,
    );
    Navigator.pop(context, (
      manifest,
      displayName.text.trim(),
      passphrase.text,
      jsonDecodeBundle(),
    ));
  }

  Map<String, dynamic> jsonDecodeBundle() =>
      (bundle.text.isEmpty ? {} : jsonDecode(bundle.text))
          as Map<String, dynamic>;

  Widget _renditionCard(BuildContext context, int index) {
    final draft = renditions[index];
    final removable = renditions.length > 1;
    return Card(
      margin: const EdgeInsets.only(bottom: 8),
      child: Padding(
        padding: const EdgeInsets.all(8),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Row(
              children: [
                Expanded(
                  child: Text(
                    'Rendition #${index + 1}',
                    style: Theme.of(context).textTheme.titleSmall,
                  ),
                ),
                if (removable)
                  IconButton(
                    icon: const Icon(Icons.remove_circle_outline),
                    tooltip: '删除该 rendition',
                    onPressed: () => _removeRendition(index),
                  ),
              ],
            ),
            TextField(
              controller: draft.renditionId,
              decoration: const InputDecoration(labelText: 'Rendition ID'),
            ),
            TextField(
              controller: draft.contentCid,
              decoration: const InputDecoration(labelText: '内容 CID *'),
            ),
            Row(
              children: [
                Expanded(
                  child: TextField(
                    controller: draft.container,
                    decoration: const InputDecoration(labelText: '容器'),
                  ),
                ),
                const SizedBox(width: 8),
                Expanded(
                  child: TextField(
                    controller: draft.codec,
                    decoration: const InputDecoration(labelText: '编解码器'),
                  ),
                ),
              ],
            ),
            Row(
              children: [
                Expanded(
                  child: TextField(
                    controller: draft.sampleRate,
                    keyboardType: TextInputType.number,
                    decoration: const InputDecoration(labelText: '采样率 Hz'),
                  ),
                ),
                const SizedBox(width: 8),
                Expanded(
                  child: TextField(
                    controller: draft.bitDepth,
                    keyboardType: TextInputType.number,
                    decoration: const InputDecoration(labelText: '位深'),
                  ),
                ),
                const SizedBox(width: 8),
                Expanded(
                  child: TextField(
                    controller: draft.channels,
                    keyboardType: TextInputType.number,
                    decoration: const InputDecoration(labelText: '声道数'),
                  ),
                ),
                const SizedBox(width: 8),
                Expanded(
                  child: TextField(
                    controller: draft.byteLength,
                    keyboardType: TextInputType.number,
                    decoration: const InputDecoration(labelText: '字节长度 *'),
                  ),
                ),
              ],
            ),
            Row(
              children: [
                Checkbox(
                  value: draft.lossless,
                  onChanged: (value) =>
                      setState(() => draft.lossless = value ?? false),
                ),
                const Text('lossless'),
                const SizedBox(width: 16),
                Checkbox(
                  value: draft.original,
                  onChanged: (value) {
                    if (value ?? false) {
                      setState(() {
                        for (final other in renditions) {
                          other.original = identical(other, draft);
                        }
                      });
                    }
                  },
                ),
                const Text('original（仅一个）'),
              ],
            ),
          ],
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: const Text('发布向导（元数据 / rendition / 身份）'),
      content: SizedBox(
        width: 640,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              TextField(
                controller: title,
                decoration: const InputDecoration(labelText: '标题 *'),
              ),
              TextField(
                controller: artists,
                decoration: const InputDecoration(labelText: '艺术家（逗号分隔）*'),
              ),
              TextField(
                controller: album,
                decoration: const InputDecoration(labelText: '专辑'),
              ),
              TextField(
                controller: contentLabels,
                decoration: const InputDecoration(
                  labelText: '内容标签（逗号分隔，如 clean）',
                ),
              ),
              DropdownButtonFormField<String>(
                initialValue: licenseIdentifier,
                decoration: const InputDecoration(labelText: '许可证'),
                items: [
                  for (final license in licenses)
                    DropdownMenuItem(value: license, child: Text(license)),
                ],
                onChanged: (value) =>
                    setState(() => licenseIdentifier = value ?? 'CC-BY-4.0'),
              ),
              const Divider(height: 24),
              for (var index = 0; index < renditions.length; index++)
                _renditionCard(context, index),
              Align(
                alignment: Alignment.centerLeft,
                child: OutlinedButton.icon(
                  onPressed: _addRendition,
                  icon: const Icon(Icons.add),
                  label: const Text('添加 Rendition'),
                ),
              ),
              const Divider(height: 24),
              TextField(
                controller: displayName,
                decoration: const InputDecoration(labelText: '身份显示名 *'),
              ),
              TextField(
                controller: passphrase,
                obscureText: true,
                decoration: const InputDecoration(labelText: '身份包口令'),
              ),
              TextField(
                controller: bundle,
                minLines: 4,
                maxLines: 10,
                decoration: const InputDecoration(
                  labelText: '加密身份包 JSON *',
                  border: OutlineInputBorder(),
                ),
              ),
              if (validationError != null)
                Text(
                  validationError!,
                  style: TextStyle(color: Theme.of(context).colorScheme.error),
                ),
            ],
          ),
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('取消'),
        ),
        FilledButton(onPressed: _submit, child: const Text('签名并发布')),
      ],
    );
  }
}
