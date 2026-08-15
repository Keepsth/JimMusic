import 'package:flutter/material.dart';
import 'package:flutter_app/widgets/publish_wizard.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  group('buildPublishManifest（UI-004）', () {
    test('生成符合 MusicManifestV1 形状的 Manifest', () {
      final manifest = buildPublishManifest(
        title: 'My Track',
        artists: const ['A', 'B'],
        album: 'Album',
        licenseIdentifier: 'CC-BY-4.0',
        contentLabels: const ['clean', 'instrumental'],
        renditionCid: 'bafycontent',
        container: 'flac',
        codec: 'flac',
        sampleRate: 44100,
        bitDepth: 24,
        channels: 2,
      );
      expect(manifest['schema_version'], 1);
      expect(manifest['title'], 'My Track');
      expect(manifest['artists'], ['A', 'B']);
      expect(manifest['license']['identifier'], 'CC-BY-4.0');
      expect(manifest['license']['allows_redistribution'], isTrue);
      expect(manifest['content_labels'], contains('clean'));
      final rendition = (manifest['renditions'] as List<dynamic>).single as Map;
      expect(rendition['content_cid'], 'bafycontent');
      expect(rendition['original'], isTrue);
      expect(rendition['streamable'], isTrue);
      expect(rendition['channel_layout'], 'stereo');
      expect(manifest['publisher_identity_cid'], 'filled-by-signer');
    });

    test('保留权利许可禁止再分发', () {
      final manifest = buildPublishManifest(
        title: 'T',
        artists: const ['A'],
        album: '',
        licenseIdentifier: 'all-rights-reserved',
        contentLabels: const [],
        renditionCid: 'bafyx',
        container: 'wav',
        codec: 'pcm',
        sampleRate: 48000,
        bitDepth: 16,
        channels: 1,
      );
      expect(manifest['license']['allows_redistribution'], isFalse);
      expect(
        (manifest['renditions'] as List<dynamic>).single['channel_layout'],
        'mono',
      );
    });
  });

  testWidgets('发布向导表单校验并返回 Manifest 与身份', (tester) async {
    (Map<String, dynamic>, String, String, Map<String, dynamic>)? result;
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => Scaffold(
            body: Center(
              child: ElevatedButton(
                onPressed: () async {
                  result = await showDialog<(
                    Map<String, dynamic>,
                    String,
                    String,
                    Map<String, dynamic>,
                  )>(
                    context: context,
                    builder: (_) => const PublishWizardDialog(),
                  );
                },
                child: const Text('open'),
              ),
            ),
          ),
        ),
      ),
    );
    await tester.tap(find.text('open'));
    await tester.pumpAndSettle();

    // 空表单提交 → 校验错误。
    await tester.tap(find.text('签名并发布'));
    await tester.pump();
    expect(find.textContaining('不能为空'), findsOneWidget);

    // 填写必填项并提交 → 对话框弹出并携带 manifest。
    await tester.enterText(
      find.widgetWithText(TextField, '标题 *'),
      'Wizard Track',
    );
    await tester.enterText(
      find.widgetWithText(TextField, '艺术家（逗号分隔）*'),
      'Artist',
    );
    await tester.enterText(
      find.widgetWithText(TextField, 'Rendition 内容 CID *'),
      'bafywizard',
    );
    await tester.enterText(
      find.widgetWithText(TextField, '身份显示名 *'),
      'Artist',
    );
    await tester.enterText(
      find.widgetWithText(TextField, '加密身份包 JSON *'),
      '{"identity":{}}',
    );
    await tester.tap(find.text('签名并发布'));
    await tester.pumpAndSettle();
    expect(find.byType(PublishWizardDialog), findsNothing);
    expect(result?.$1['title'], 'Wizard Track');
    expect(result?.$2, 'Artist');
  });
}
