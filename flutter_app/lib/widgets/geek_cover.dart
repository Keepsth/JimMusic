import 'package:flutter/material.dart';

/// 极客风专辑封面：确定性配色 + 字形，零网络请求、零图片解码。
///
/// 用 [seed] 的哈希派生前景/背景色，展示曲目标题首字符或音符，
/// 完全取代 `Image.network`，从根源上消除逐项网络拉取、图片解码与缓存开销。
class GeekCover extends StatelessWidget {
  final String seed;
  final String? label;
  final double size;
  final BorderRadius? borderRadius;

  const GeekCover({
    super.key,
    required this.seed,
    this.label,
    this.size = 56,
    this.borderRadius,
  });

  /// 极客风调色板（低饱和暗底 + 高亮前景），仅 8 组，保证低成本。
  static const List<Color> _bg = [
    Color(0xFF0F1A0F),
    Color(0xFF0A1420),
    Color(0xFF1A0F1A),
    Color(0xFF1A140A),
    Color(0xFF0F1A1A),
    Color(0xFF1A0F0F),
    Color(0xFF10101A),
    Color(0xFF141A0F),
  ];

  static const List<Color> _fg = [
    Color(0xFF33FF66),
    Color(0xFF33CCFF),
    Color(0xFFFF66CC),
    Color(0xFFFFCC33),
    Color(0xFF33FFCC),
    Color(0xFFFF6633),
    Color(0xFF9966FF),
    Color(0xFF99FF33),
  ];

  @override
  Widget build(BuildContext context) {
    // 稳定哈希：String.hashCode 在平台间可能不一致，但同一次运行内稳定，
    // 足以满足「确定性、零成本」的目标（不做跨端持久化要求）。
    final h = seed.hashCode;
    final idx = h.abs() % _bg.length;

    return Container(
      width: size,
      height: size,
      decoration: BoxDecoration(
        color: _bg[idx],
        borderRadius: borderRadius ?? BorderRadius.zero,
        border: Border.all(color: _fg[idx].withValues(alpha: 0.35), width: 1),
      ),
      alignment: Alignment.center,
      child: Text(
        _glyph(),
        style: TextStyle(
          color: _fg[idx],
          fontSize: size * 0.32,
          fontWeight: FontWeight.bold,
          fontFamilyFallback: const ['monospace'],
        ),
      ),
    );
  }

  String _glyph() {
    final t = label;
    if (t != null && t.isNotEmpty) {
      final c = t.trim()[0];
      // 仅对可打印 ASCII 展示首字符，其余回退为音符符号。
      if (c.codeUnitAt(0) >= 0x20 && c.codeUnitAt(0) < 0x7F) {
        return String.fromCharCode(c.codeUnitAt(0));
      }
    }
    return '♪';
  }
}
