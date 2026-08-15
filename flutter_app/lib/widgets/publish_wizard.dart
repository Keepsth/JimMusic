import 'dart:convert';

import 'package:flutter/material.dart';

/// 发布向导的元数据/rendition 表单 → MusicManifestV1 JSON（UI-004）。
/// 纯函数便于测试；发布者身份 CID 由签名端填充。
Map<String, dynamic> buildPublishManifest({
  required String title,
  required List<String> artists,
  required String album,
  required String licenseIdentifier,
  required List<String> contentLabels,
  required String renditionCid,
  required String container,
  required String codec,
  required int sampleRate,
  required int bitDepth,
  required int channels,
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
    'renditions': [
      {
        'rendition_id': 'original',
        'content_cid': renditionCid,
        'container': container,
        'codec': codec,
        'sample_rate': sampleRate,
        'bit_depth': bitDepth,
        'channels': channels,
        'channel_layout': channels == 1 ? 'mono' : 'stereo',
        'duration_ms': durationMs,
        'byte_length': 0,
        'lossless': true,
        'original': true,
        'streamable': true,
      },
    ],
    'publisher_identity_cid': 'filled-by-signer',
    'created_at': 1,
    'updated_at': 1,
  };
}

/// 结构化发布向导：元数据 + rendition + 身份解锁，返回
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
  final renditionCid = TextEditingController();
  final container = TextEditingController(text: 'flac');
  final codec = TextEditingController(text: 'flac');
  final sampleRate = TextEditingController(text: '44100');
  final bitDepth = TextEditingController(text: '24');
  final channels = TextEditingController(text: '2');
  final displayName = TextEditingController();
  final passphrase = TextEditingController();
  final bundle = TextEditingController();
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
      renditionCid,
      container,
      codec,
      sampleRate,
      bitDepth,
      channels,
      displayName,
      passphrase,
      bundle,
    ]) {
      controller.dispose();
    }
    super.dispose();
  }

  void _submit() {
    if (title.text.trim().isEmpty ||
        artists.text.trim().isEmpty ||
        renditionCid.text.trim().isEmpty ||
        displayName.text.trim().isEmpty ||
        bundle.text.trim().isEmpty) {
      setState(
        () => validationError = '标题、艺术家、内容 CID、身份名称与身份包均不能为空',
      );
      return;
    }
    final parsedRate = int.tryParse(sampleRate.text);
    final parsedDepth = int.tryParse(bitDepth.text);
    final parsedChannels = int.tryParse(channels.text);
    if (parsedRate == null ||
        parsedRate <= 0 ||
        parsedDepth == null ||
        parsedDepth <= 0 ||
        parsedChannels == null ||
        parsedChannels <= 0) {
      setState(() => validationError = '采样率/位深/声道必须是正整数');
      return;
    }
    try {
      jsonDecodeBundle();
    } catch (_) {
      setState(() => validationError = '身份包不是有效 JSON');
      return;
    }
    final manifest = buildPublishManifest(
      title: title.text.trim(),
      artists: artists.text.split(',').map((v) => v.trim()).where((v) => v.isNotEmpty).toList(),
      album: album.text.trim(),
      licenseIdentifier: licenseIdentifier,
      contentLabels: contentLabels.text.split(',').map((v) => v.trim()).where((v) => v.isNotEmpty).toList(),
      renditionCid: renditionCid.text.trim(),
      container: container.text.trim(),
      codec: codec.text.trim(),
      sampleRate: parsedRate,
      bitDepth: parsedDepth,
      channels: parsedChannels,
    );
    Navigator.pop(context, (
      manifest,
      displayName.text.trim(),
      passphrase.text,
      jsonDecodeBundle(),
    ));
  }

  Map<String, dynamic> jsonDecodeBundle() =>
      (bundle.text.isEmpty ? {} : jsonDecode(bundle.text)) as Map<String, dynamic>;

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
              TextField(
                controller: renditionCid,
                decoration: const InputDecoration(labelText: 'Rendition 内容 CID *'),
              ),
              Row(
                children: [
                  Expanded(
                    child: TextField(
                      controller: container,
                      decoration: const InputDecoration(labelText: '容器'),
                    ),
                  ),
                  const SizedBox(width: 8),
                  Expanded(
                    child: TextField(
                      controller: codec,
                      decoration: const InputDecoration(labelText: '编解码器'),
                    ),
                  ),
                ],
              ),
              Row(
                children: [
                  Expanded(
                    child: TextField(
                      controller: sampleRate,
                      keyboardType: TextInputType.number,
                      decoration: const InputDecoration(labelText: '采样率 Hz'),
                    ),
                  ),
                  const SizedBox(width: 8),
                  Expanded(
                    child: TextField(
                      controller: bitDepth,
                      keyboardType: TextInputType.number,
                      decoration: const InputDecoration(labelText: '位深'),
                    ),
                  ),
                  const SizedBox(width: 8),
                  Expanded(
                    child: TextField(
                      controller: channels,
                      keyboardType: TextInputType.number,
                      decoration: const InputDecoration(labelText: '声道数'),
                    ),
                  ),
                ],
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
