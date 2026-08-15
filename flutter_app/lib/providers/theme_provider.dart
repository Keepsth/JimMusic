import 'package:flutter/material.dart';

import '../services/persistence_service.dart';

/// 主题模式。
enum ThemeModeType { dark, light, system }

/// 主题提供者：极客风（终端/黑客）配色，强调低资源占用与低性能开销。
///
/// 性能取向：
/// - 关闭所有隐式动画（pageTransitions/inkSplash/inkRipple 均为无动画实现）；
/// - 无阴影、无模糊、无海拔，以 1px 边框替代（终端取景框观感）；
/// - 紧凑视觉密度（VisualDensity.compact）；
/// - 等宽字体 fallback（不引入字体资源，零下载）；
/// - 纯色暗底（近黑），不做渐变/纹理。
class ThemeProvider extends ChangeNotifier {
  // 默认极客风：霓虹绿。
  ThemeModeType _mode = ThemeModeType.dark;
  Color _accentColor = const Color(0xFF33FF66);

  /// 深色背景（近黑，偏一点点绿冷调）。
  static const Color geekBackground = Color(0xFF0A0E0A);
  static const Color geekSurface = Color(0xFF101410);
  static const Color geekBorder = Color(0xFF1E2A1E);
  static const Color geekText = Color(0xFFC8FFD4);
  static const Color geekTextDim = Color(0xFF5A7A5A);

  ThemeModeType get mode => _mode;
  Color get accentColor => _accentColor;

  /// 映射到 MaterialApp 的 ThemeMode。
  ThemeMode get themeMode {
    switch (_mode) {
      case ThemeModeType.dark:
        return ThemeMode.dark;
      case ThemeModeType.light:
        return ThemeMode.light;
      case ThemeModeType.system:
        return ThemeMode.system;
    }
  }

  /// 深色主题实例。
  ThemeData get darkTheme => _buildTheme(Brightness.dark);

  /// 浅色主题实例。
  ThemeData get lightTheme => _buildTheme(Brightness.light);

  /// 加载已持久化的主题设置。
  Future<void> load() async {
    final modeStr = await PersistenceService.loadThemeMode();
    _mode = _parseMode(modeStr);
    final color = await PersistenceService.loadAccentColor();
    if (color != null) _accentColor = Color(color);
    notifyListeners();
  }

  /// 设置主题模式。
  Future<void> setMode(ThemeModeType mode) async {
    _mode = mode;
    notifyListeners();
    await PersistenceService.saveThemeMode(mode.name);
  }

  /// 设置自定义强调色。
  Future<void> setAccentColor(Color color) async {
    _accentColor = color;
    notifyListeners();
    await PersistenceService.saveAccentColor(color.toARGB32());
  }

  static ThemeModeType _parseMode(String s) {
    switch (s) {
      case 'light':
        return ThemeModeType.light;
      case 'system':
        return ThemeModeType.system;
      default:
        return ThemeModeType.dark;
    }
  }

  /// 相邻文字色的扁平 ColorScheme（不依赖 fromSeed 的色调推导，避免多余计算，
  /// 也避免生成偏离极客风的中间色）。
  ColorScheme _scheme(Brightness brightness) {
    final dark = brightness == Brightness.dark;
    return ColorScheme(
      brightness: brightness,
      primary: _accentColor,
      onPrimary: Colors.black,
      secondary: _accentColor.withValues(alpha: 0.8),
      onSecondary: Colors.black,
      error: const Color(0xFFFF4466),
      onError: Colors.black,
      surface: dark ? geekBackground : Colors.white,
      onSurface: dark ? geekText : const Color(0xFF0A140A),
      onSurfaceVariant: dark ? geekTextDim : const Color(0xFF3A4A3A),
      outline: dark ? geekBorder : const Color(0xFFB0C0B0),
      outlineVariant: dark ? geekBorder : const Color(0xFFD0D8D0),
      surfaceContainerHighest: dark ? geekSurface : const Color(0xFFF0F2F0),
      shadow: Colors.transparent,
    );
  }

  /// 构建主题。
  ThemeData _buildTheme(Brightness brightness) {
    final dark = brightness == Brightness.dark;
    final scheme = _scheme(brightness);

    final base = ThemeData(
      colorScheme: scheme,
      useMaterial3: true,
      brightness: brightness,
      scaffoldBackgroundColor: dark ? geekBackground : const Color(0xFFF4F6F4),
      visualDensity: VisualDensity.compact,
      // 等宽字体，零字体资源开销。
      fontFamilyFallback: const ['monospace', 'Menlo', 'Consolas', 'Courier'],
      // 关闭 profit 的动画：页面过渡/水波纹/点击反馈全部无动画。
      pageTransitionsTheme: const PageTransitionsTheme(
        builders: {
          TargetPlatform.android: _NoAnimationPageTransitionsBuilder(),
          TargetPlatform.iOS: _NoAnimationPageTransitionsBuilder(),
          TargetPlatform.linux: _NoAnimationPageTransitionsBuilder(),
          TargetPlatform.macOS: _NoAnimationPageTransitionsBuilder(),
          TargetPlatform.windows: _NoAnimationPageTransitionsBuilder(),
          TargetPlatform.fuchsia: _NoAnimationPageTransitionsBuilder(),
        },
      ),
      splashFactory: NoSplash.splashFactory,
      appBarTheme: AppBarTheme(
        backgroundColor: dark ? geekBackground : Colors.white,
        elevation: 0,
        scrolledUnderElevation: 0,
        centerTitle: false,
        foregroundColor: dark ? geekText : const Color(0xFF0A140A),
        titleTextStyle: TextStyle(
          fontSize: 16,
          fontWeight: FontWeight.bold,
          color: scheme.primary,
          fontFamilyFallback: const ['monospace'],
          letterSpacing: 1.2,
        ),
      ),
      dividerTheme: DividerThemeData(
        color: dark ? geekBorder : const Color(0xFFC8D0C8),
        thickness: 1,
        space: 1,
      ),
    );

    return base;
  }
}

/// 无动画页面过渡：消除路由切换的 GPU 合成开销。
class _NoAnimationPageTransitionsBuilder extends PageTransitionsBuilder {
  const _NoAnimationPageTransitionsBuilder();

  @override
  Widget buildTransitions<T>(
    PageRoute<T> route,
    BuildContext context,
    Animation<double> animation,
    Animation<double> secondaryAnimation,
    Widget child,
  ) {
    return child;
  }
}
