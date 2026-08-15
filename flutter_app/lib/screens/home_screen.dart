import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../providers/music_player_provider.dart';
import '../widgets/music_list_item.dart';
import '../widgets/now_playing_bar.dart';
import 'favorites_screen.dart';
import 'player_screen.dart';
import 'playlists_screen.dart';
import 'search_screen.dart';
import 'settings_screen.dart';
import 'control_center_screen.dart';

class HomeScreen extends StatelessWidget {
  const HomeScreen({super.key});

  @override
  Widget build(BuildContext context) {
    return const _HomeScreenScaffold();
  }
}

class _HomeScreenScaffold extends StatefulWidget {
  const _HomeScreenScaffold();

  @override
  State<_HomeScreenScaffold> createState() => _HomeScreenScaffoldState();
}

class _HomeScreenScaffoldState extends State<_HomeScreenScaffold> {
  int _currentTab = 0;

  static const _tabs = <Widget>[
    LibraryTab(),
    SearchScreen(),
    FavoritesScreen(),
    PlaylistsScreen(),
  ];

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        title: const Text(
          'JimMusic',
          style: TextStyle(fontWeight: FontWeight.bold, fontSize: 24),
        ),
        actions: [
          IconButton(
            icon: const Icon(Icons.hub_outlined),
            tooltip: '节点与分发控制台',
            onPressed: () {
              Navigator.push(
                context,
                MaterialPageRoute(builder: (_) => const ControlCenterScreen()),
              );
            },
          ),
          IconButton(
            icon: const Icon(Icons.settings_outlined),
            tooltip: '设置',
            onPressed: () {
              Navigator.push(
                context,
                MaterialPageRoute(builder: (_) => const SettingsScreen()),
              );
            },
          ),
        ],
      ),
      body: Column(
        children: [
          Consumer<MusicPlayerProvider>(
            builder: (context, player, _) {
              final error = player.playbackError;
              if (error == null) return const SizedBox.shrink();
              return MaterialBanner(
                content: Text(error),
                leading: const Icon(Icons.error_outline),
                actions: [
                  TextButton(
                    onPressed: player.clearPlaybackError,
                    child: const Text('关闭'),
                  ),
                ],
              );
            },
          ),
          Expanded(
            child: IndexedStack(index: _currentTab, children: _tabs),
          ),
          Consumer<MusicPlayerProvider>(
            builder: (context, player, child) {
              if (player.currentMusic != null) {
                return NowPlayingBar(
                  onTap: () {
                    Navigator.push(
                      context,
                      MaterialPageRoute(builder: (_) => const PlayerScreen()),
                    );
                  },
                );
              }
              return const SizedBox.shrink();
            },
          ),
        ],
      ),
      bottomNavigationBar: NavigationBar(
        selectedIndex: _currentTab,
        onDestinationSelected: (i) => setState(() => _currentTab = i),
        destinations: const [
          NavigationDestination(
            icon: Icon(Icons.library_music_outlined),
            label: '媒体库',
          ),
          NavigationDestination(icon: Icon(Icons.search), label: '搜索'),
          NavigationDestination(
            icon: Icon(Icons.favorite_outline),
            label: '收藏',
          ),
          NavigationDestination(icon: Icon(Icons.playlist_add), label: '播放列表'),
        ],
      ),
    );
  }
}

/// 媒体库标签页：展示本地音乐，支持导入与基本操作。
class LibraryTab extends StatelessWidget {
  const LibraryTab({super.key});

  @override
  Widget build(BuildContext context) {
    final player = context.watch<MusicPlayerProvider>();
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 16, 16, 8),
          child: Row(
            mainAxisAlignment: MainAxisAlignment.spaceBetween,
            children: [
              const Text(
                '本地音乐',
                style: TextStyle(fontSize: 20, fontWeight: FontWeight.bold),
              ),
              TextButton.icon(
                onPressed: () => _import(context),
                icon: const Icon(Icons.add),
                label: const Text('导入'),
              ),
            ],
          ),
        ),
        Expanded(
          child: player.library.isEmpty
              ? const _EmptyLibrary()
              : ListView.builder(
                  itemCount: player.library.length,
                  itemBuilder: (context, index) {
                    final music = player.library[index];
                    return MusicListItem(
                      music: music,
                      isPlaying:
                          player.currentMusic?.id == music.id &&
                          player.isPlaying,
                      onTap: () => player.play(music),
                    );
                  },
                ),
        ),
      ],
    );
  }

  Future<void> _import(BuildContext context) async {
    final player = context.read<MusicPlayerProvider>();
    final messenger = ScaffoldMessenger.of(context);
    try {
      final added = await player.importFiles();
      if (added > 0) {
        messenger.showSnackBar(SnackBar(content: Text('已导入 $added 首曲目')));
      } else {
        messenger.showSnackBar(const SnackBar(content: Text('未发现新的音频文件')));
      }
    } catch (e) {
      messenger.showSnackBar(SnackBar(content: Text('导入失败：$e')));
    }
  }
}

class _EmptyLibrary extends StatelessWidget {
  const _EmptyLibrary();

  @override
  Widget build(BuildContext context) {
    return Center(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(Icons.library_music, size: 64, color: Colors.grey.shade600),
          const SizedBox(height: 12),
          const Text('媒体库为空'),
          const SizedBox(height: 6),
          const Text('点击「导入」添加真实音频文件；应用不会填充示例曲目'),
        ],
      ),
    );
  }
}
