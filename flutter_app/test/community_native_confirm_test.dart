import 'package:flutter/material.dart';
import 'package:flutter_app/widgets/community_native_confirm.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  Future<bool? Function()> pumpAndOpen(WidgetTester tester) async {
    bool? result;
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => Scaffold(
            body: Center(
              child: ElevatedButton(
                onPressed: () async {
                  result = await confirmCommunityNative(
                    context,
                    pluginName: 'DemoPlugin',
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
    return () => result;
  }

  testWidgets('社区原生高级授权二次确认展示持续警告文案（PLG-007）', (tester) async {
    final result = await pumpAndOpen(tester);
    expect(find.text('社区原生高级授权'), findsOneWidget);
    expect(find.textContaining('DemoPlugin'), findsOneWidget);
    expect(find.textContaining('未经官方审查'), findsOneWidget);
    expect(find.textContaining('持续警告'), findsOneWidget);
    expect(result(), isNull); // 对话框仍打开。
  });

  testWidgets('确认后返回 true', (tester) async {
    final result = await pumpAndOpen(tester);
    await tester.tap(find.text('我已了解，继续安装'));
    await tester.pumpAndSettle();
    expect(result(), isTrue);
    expect(find.byType(AlertDialog), findsNothing);
  });

  testWidgets('取消返回 false', (tester) async {
    final result = await pumpAndOpen(tester);
    await tester.tap(find.text('取消'));
    await tester.pumpAndSettle();
    expect(result(), isFalse);
    expect(find.byType(AlertDialog), findsNothing);
  });

  testWidgets('持续警告条目在插件列表中渲染', (tester) async {
    await tester.pumpWidget(
      const MaterialApp(home: Scaffold(body: CommunityNativeWarningTile())),
    );
    expect(find.text('社区原生高级授权'), findsOneWidget);
    expect(find.textContaining('未经官方审查'), findsOneWidget);
  });
}
