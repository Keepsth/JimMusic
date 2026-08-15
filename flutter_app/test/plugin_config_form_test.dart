import 'package:flutter/material.dart';
import 'package:flutter_app/widgets/plugin_config_form.dart';
import 'package:flutter_test/flutter_test.dart';

const _schema = <String, dynamic>{
  'type': 'object',
  'properties': {
    'enabled': {
      'type': 'boolean',
      'default': true,
      'description': '启用处理器',
    },
    'mode': {'type': 'string', 'enum': ['fast', 'quality'], 'default': 'fast'},
    'gain': {'type': 'number', 'minimum': -12, 'maximum': 12, 'default': 0},
    'label': {'type': 'string', 'default': 'preset'},
  },
};

void main() {
  group('schemaDefaults（PLG-014）', () {
    test('按类型与默认值计算默认配置', () {
      final defaults = schemaDefaults(_schema);
      expect(defaults['enabled'], isTrue);
      expect(defaults['mode'], 'fast');
      expect(defaults['gain'], 0);
      expect(defaults['label'], 'preset');
    });

    test('enum 无默认时取首项，整数取最小值', () {
      final defaults = schemaDefaults({
        'properties': {
          'mode': {'type': 'string', 'enum': ['a', 'b']},
          'level': {'type': 'integer', 'minimum': 3},
        },
      });
      expect(defaults['mode'], 'a');
      expect(defaults['level'], 3);
    });
  });

  testWidgets('敏感字段遮罩显示（PLG-011）', (tester) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: PluginConfigForm(
            schema: {
              'properties': {
                'api_token': {
                  'type': 'string',
                  'default': 'secret-value',
                  'sensitive': true,
                },
              },
            },
            initial: const {},
            onChanged: (_) {},
          ),
        ),
      ),
    );
    final field = tester.widget<TextField>(
      find.descendant(
        of: find.byType(TextFormField),
        matching: find.byType(TextField),
      ),
    );
    expect(field.obscureText, isTrue);
    expect(find.textContaining('敏感'), findsOneWidget);
  });

  testWidgets('按 Schema 渲染开关/枚举/滑杆/文本框并回传更新', (tester) async {
    Map<String, dynamic>? latest;
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SingleChildScrollView(
            child: PluginConfigForm(
              schema: _schema,
              initial: const {},
              onChanged: (values) => latest = values,
            ),
          ),
        ),
      ),
    );

    expect(find.byType(SwitchListTile), findsOneWidget);
    expect(find.byType(DropdownButtonFormField<Object>), findsOneWidget);
    expect(find.byType(Slider), findsOneWidget);
    expect(find.byType(TextFormField), findsOneWidget);
    expect(find.textContaining('启用处理器'), findsOneWidget);

    // 切换开关 → 回传更新值。
    await tester.tap(find.byType(SwitchListTile));
    await tester.pump();
    expect(latest?['enabled'], isFalse);

    // 拖拽滑杆 → 数值更新。
    await tester.drag(find.byType(Slider), const Offset(80, 0));
    await tester.pump();
    expect(latest?['gain'], isNot(0));
  });
}
