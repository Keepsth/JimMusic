import 'dart:convert';

import 'package:shared_preferences/shared_preferences.dart';

import '../models/playlist.dart';
import '../models/music.dart';

/// 本地持久化服务：保存/恢复收藏、播放列表与主题设置。
class PersistenceService {
  static const _kFavorites = 'favorites';
  static const _kPlaylists = 'playlists';
  static const _kThemeMode = 'theme_mode';
  static const _kAccentColor = 'accent_color';
  static const _kActiveOutput = 'active_output';
  static const _kLibrary = 'library_v2';
  static const _kPlaybackSession = 'playback_session_v2';
  static const _kControlEndpoint = 'control_endpoint';

  /// 加载收藏的曲目 ID 集合。
  static Future<Set<String>> loadFavoriteIds() async {
    final prefs = await SharedPreferences.getInstance();
    final list = prefs.getStringList(_kFavorites) ?? [];
    return list.toSet();
  }

  /// 保存收藏的曲目 ID 集合。
  static Future<void> saveFavoriteIds(Set<String> ids) async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setStringList(_kFavorites, ids.toList());
  }

  /// 加载播放列表（Map: 名称 -> id 列表）。
  static Future<Map<String, Playlist>> loadPlaylists() async {
    final prefs = await SharedPreferences.getInstance();
    final raw = prefs.getString(_kPlaylists);
    if (raw == null || raw.isEmpty) return {};
    try {
      final decoded = jsonDecode(raw) as Map<String, dynamic>;
      return decoded.map(
        (k, v) => MapEntry(k, Playlist.fromMap(v as Map<String, dynamic>)),
      );
    } catch (_) {
      return {};
    }
  }

  /// 保存播放列表。
  static Future<void> savePlaylists(Map<String, Playlist> playlists) async {
    final prefs = await SharedPreferences.getInstance();
    final encoded = jsonEncode(playlists.map((k, v) => MapEntry(k, v.toMap())));
    await prefs.setString(_kPlaylists, encoded);
  }

  /// 加载主题模式字符串（'dark' | 'light' | 'system'）。
  static Future<String> loadThemeMode() async {
    final prefs = await SharedPreferences.getInstance();
    return prefs.getString(_kThemeMode) ?? 'dark';
  }

  /// 保存主题模式。
  static Future<void> saveThemeMode(String mode) async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_kThemeMode, mode);
  }

  /// 加载自定义强调色（int 值）。
  static Future<int?> loadAccentColor() async {
    final prefs = await SharedPreferences.getInstance();
    return prefs.getInt(_kAccentColor);
  }

  /// 保存自定义强调色。
  static Future<void> saveAccentColor(int color) async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setInt(_kAccentColor, color);
  }

  /// 加载激活的音频输出后端 id（缺省 'auto'）。
  static Future<String> loadActiveOutput() async {
    final prefs = await SharedPreferences.getInstance();
    return prefs.getString(_kActiveOutput) ?? 'auto';
  }

  /// 保存激活的音频输出后端 id。
  static Future<void> saveActiveOutput(String id) async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_kActiveOutput, id);
  }

  static Future<List<Music>> loadLibrary() async {
    final prefs = await SharedPreferences.getInstance();
    final raw = prefs.getString(_kLibrary);
    if (raw == null || raw.isEmpty) return [];
    try {
      final values = jsonDecode(raw) as List<dynamic>;
      return values
          .whereType<Map<String, dynamic>>()
          .map(Music.fromMap)
          .toList(growable: false);
    } catch (_) {
      return [];
    }
  }

  static Future<void> saveLibrary(List<Music> tracks) async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(
      _kLibrary,
      jsonEncode(tracks.map((track) => track.toMap()).toList()),
    );
  }

  static Future<Map<String, dynamic>> loadPlaybackSession() async {
    final prefs = await SharedPreferences.getInstance();
    final raw = prefs.getString(_kPlaybackSession);
    if (raw == null || raw.isEmpty) return {};
    try {
      return jsonDecode(raw) as Map<String, dynamic>;
    } catch (_) {
      return {};
    }
  }

  static Future<void> savePlaybackSession(Map<String, dynamic> session) async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_kPlaybackSession, jsonEncode(session));
  }

  static Future<String> loadControlEndpoint() async {
    final prefs = await SharedPreferences.getInstance();
    return prefs.getString(_kControlEndpoint) ?? 'http://127.0.0.1:8787/v1';
  }

  static Future<void> saveControlEndpoint(String endpoint) async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_kControlEndpoint, endpoint);
  }
}
