import 'package:flutter_app/services/control_api_types.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  group('apiErrorText（API-004 本地化）', () {
    test('unsupported 携带结构化原因', () {
      final text = apiErrorText(
        const ControlApiException(
          'x',
          statusCode: 400,
          body: {
            'code': 'unsupported',
            'subsystem': 'node',
            'unsupported_reason': '内嵌 Bitswap 无带宽节流',
            'retryable': false,
          },
        ),
      );
      expect(text.message, contains('不受支持'));
      expect(text.message, contains('node'));
      expect(text.message, contains('Bitswap'));
      expect(text.suggestion, isNull);
    });

    test('401 映射为令牌提示', () {
      final text = apiErrorText(
        const ControlApiException('unauthorized', statusCode: 401),
      );
      expect(text.message, contains('令牌'));
      expect(text.suggestion, contains('重新设置'));
    });

    test('网络策略暂停码映射恢复提示', () {
      for (final entry in {
        'paused_wifi_only': 'Wi-Fi',
        'paused_metered_network': '计量',
        'paused_cellular_quota': '蜂窝额度',
      }.entries) {
        final text = apiErrorText(
          ControlApiException(
            'x',
            body: {'code': entry.key, 'retryable': false},
          ),
        );
        expect(text.message, contains('暂停'), reason: entry.key);
        expect(networkPauseHint(entry.key), contains(entry.value));
      }
    });

    test('retryable 提供重试建议', () {
      final text = apiErrorText(
        ControlApiException(
          'x',
          body: {
            'code': 'invalid_request',
            'subsystem': 'transfer',
            'retryable': true,
          },
        ),
      );
      expect(text.message, contains('transfer'));
      expect(text.suggestion, contains('稍后重试'));
    });

    test('未知错误回退原文', () {
      final text = apiErrorText(const ControlApiException('连接被拒绝'));
      expect(text.message, '连接被拒绝');
    });
  });
}
