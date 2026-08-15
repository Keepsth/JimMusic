import 'package:flutter/material.dart';
import 'package:flutter_app/widgets/publish_wizard.dart';
import 'package:flutter_test/flutter_test.dart';

Map<String, dynamic> rendition({
  String id = 'original',
  String cid = 'bafycontent',
  int byteLength = 1234,
  bool original = true,
}) => buildOriginalRendition(
  renditionId: id,
  contentCid: cid,
  container: 'flac',
  codec: 'flac',
  sampleRate: 44100,
  bitDepth: 24,
  channels: 2,
  byteLength: byteLength,
  original: original,
);

void main() {
  group('buildPublishManifest（UI-004）', () {
    test('生成符合 MusicManifestV1 形状的 Manifest', () {
      final manifest = buildPublishManifest(
        title: 'My Track',
        artists: const ['A', 'B'],
        album: 'Album',
        licenseIdentifier: 'CC-BY-4.0',
        contentLabels: const ['clean', 'instrumental'],
        renditions: [rendition()],
      );
      expect(manifest['schema_version'], 1);
      expect(manifest['title'], 'My Track');
      expect(manifest['artists'], ['A', 'B']);
      expect(manifest['license']['identifier'], 'CC-BY-4.0');
      expect(manifest['license']['allows_redistribution'], isTrue);
      expect(manifest['content_labels'], contains('clean'));
      final entries = manifest['renditions'] as List<dynamic>;
      expect(entries, hasLength(1));
      final first = entries.single as Map;
      expect(first['content_cid'], 'bafycontent');
      expect(first['byte_length'], 1234);
      expect(first['original'], isTrue);
      expect(first['streamable'], isTrue);
      expect(first['channel_layout'], 'stereo');
      expect(manifest['publisher_identity_cid'], 'filled-by-signer');
    });

    test('保留权利许可禁止再分发', () {
      final manifest = buildPublishManifest(
        title: 'T',
        artists: const ['A'],
        album: '',
        licenseIdentifier: 'all-rights-reserved',
        contentLabels: const [],
        renditions: [
          buildOriginalRendition(
            renditionId: 'original',
            contentCid: 'bafyx',
            container: 'wav',
            codec: 'pcm',
            sampleRate: 48000,
            bitDepth: 16,
            channels: 1,
            byteLength: 42,
            original: true,
          ),
        ],
      );
      expect(manifest['license']['allows_redistribution'], isFalse);
      expect(
        (manifest['renditions'] as List<dynamic>).single['channel_layout'],
        'mono',
      );
    });

    test('多 rendition 编辑：条目完整保留且仅一个 original', () {
      final manifest = buildPublishManifest(
        title: 'Multi',
        artists: const ['A'],
        album: '',
        licenseIdentifier: 'CC0-1.0',
        contentLabels: const [],
        renditions: [
          rendition(
            id: 'original',
            cid: 'bafy-lossless',
            byteLength: 9000,
            original: true,
          ),
          rendition(
            id: 'aac-256',
            cid: 'bafy-lossy',
            byteLength: 3000,
            original: false,
          ),
        ],
      );
      final entries = (manifest['renditions'] as List<dynamic>).cast<Map>();
      expect(entries, hasLength(2));
      expect(entries.map((entry) => entry['rendition_id']), [
        'original',
        'aac-256',
      ]);
      expect(entries.where((entry) => entry['original'] == true), hasLength(1));
      expect(entries[1]['byte_length'], 3000);
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
                  result =
                      await showDialog<
                        (
                          Map<String, dynamic>,
                          String,
                          String,
                          Map<String, dynamic>,
                        )
                      >(
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
      find.widgetWithText(TextField, '内容 CID *'),
      'bafywizard',
    );
    await tester.enterText(find.widgetWithText(TextField, '字节长度 *'), '2048');
    await tester.enterText(find.widgetWithText(TextField, '身份显示名 *'), 'Artist');
    await tester.enterText(
      find.widgetWithText(TextField, '加密身份包 JSON *'),
      '{"identity":{}}',
    );
    await tester.tap(find.text('签名并发布'));
    await tester.pumpAndSettle();
    expect(find.byType(PublishWizardDialog), findsNothing);
    expect(result?.$1['title'], 'Wizard Track');
    expect(result?.$2, 'Artist');
    final entries = (result?.$1['renditions'] as List<dynamic>);
    expect(entries.single['byte_length'], 2048);
  });

  testWidgets('向导支持多 rendition 编辑与删除', (tester) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => Scaffold(
            body: Center(
              child: ElevatedButton(
                onPressed: () => showDialog<void>(
                  context: context,
                  builder: (_) => const PublishWizardDialog(),
                ),
                child: const Text('open'),
              ),
            ),
          ),
        ),
      ),
    );
    await tester.tap(find.text('open'));
    await tester.pumpAndSettle();

    // 添加第二个 rendition。
    await tester.ensureVisible(find.text('添加 Rendition'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('添加 Rendition'));
    await tester.pumpAndSettle();
    expect(find.text('Rendition #2'), findsOneWidget);
    expect(find.text('rendition-2'), findsOneWidget);

    // 删除它。
    final deleteButton = find.byTooltip('删除该 rendition');
    await tester.ensureVisible(deleteButton.last);
    await tester.pumpAndSettle();
    await tester.tap(deleteButton.last);
    await tester.pumpAndSettle();
    expect(find.text('Rendition #2'), findsNothing);
  });
}
