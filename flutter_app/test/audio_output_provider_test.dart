import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:flutter_app/models/audio_output.dart';
import 'package:flutter_app/providers/audio_output_provider.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  setUp(() {
    SharedPreferences.setMockInitialValues({});
  });

  test('默认激活「自动」后端', () async {
    final provider = AudioOutputProvider();
    await provider.load();
    expect(provider.activeId, 'auto');
    expect(provider.active.id, 'auto');
  });

  test('无法加载真实插件时不伪装切换成功', () async {
    final provider = AudioOutputProvider();
    await provider.load();

    await provider.activate('null');
    expect(provider.activeId, 'auto');
    expect(provider.error, isNotNull);
  });

  test('未知后端 id 被忽略', () async {
    final provider = AudioOutputProvider();
    await provider.load();
    await provider.activate('not-a-real-backend');
    expect(provider.activeId, 'auto');
  });

  test('后端目录只声明仓库已经交付的实现', () {
    final ids = AudioOutputDevice.backends.map((d) => d.id).toList();
    for (final expected in ['auto', 'null', 'system', 'web-audio']) {
      expect(ids, contains(expected));
    }
    expect(ids, isNot(contains('pipewire')));
    expect(ids, isNot(contains('wasapi')));
  });
}
