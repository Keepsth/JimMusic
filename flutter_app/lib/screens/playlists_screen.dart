import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../providers/music_player_provider.dart';
import '../widgets/music_list_item.dart';

/// 播放列表页：管理多个播放列表。
class PlaylistsScreen extends StatelessWidget {
  const PlaylistsScreen({super.key});

  @override
  Widget build(BuildContext context) {
    final player = context.watch<MusicPlayerProvider>();
    final names = player.playlists.keys.toList()..sort();

    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 16, 16, 8),
          child: Row(
            mainAxisAlignment: MainAxisAlignment.spaceBetween,
            children: [
              const Text(
                '播放列表',
                style: TextStyle(fontSize: 20, fontWeight: FontWeight.bold),
              ),
              TextButton.icon(
                onPressed: () => _createPlaylist(context),
                icon: const Icon(Icons.playlist_add),
                label: const Text('新建'),
              ),
            ],
          ),
        ),
        Expanded(
          child: names.isEmpty
              ? const Center(child: Text('还没有播放列表，点击「新建」创建'))
              : ListView.builder(
                  itemCount: names.length,
                  itemBuilder: (context, index) {
                    final name = names[index];
                    final tracks = player.tracksInPlaylist(name);
                    return ListTile(
                      leading: const Icon(Icons.queue_music),
                      title: Text(name),
                      subtitle: Text('${tracks.length} 首歌曲'),
                      trailing: IconButton(
                        icon: const Icon(Icons.delete_outline),
                        onPressed: () => player.deletePlaylist(name),
                      ),
                      onTap: () => _openPlaylist(context, name),
                    );
                  },
                ),
        ),
      ],
    );
  }

  Future<void> _createPlaylist(BuildContext context) async {
    final player = context.read<MusicPlayerProvider>();
    final controller = TextEditingController();
    final name = await showDialog<String>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('新建播放列表'),
        content: TextField(
          controller: controller,
          autofocus: true,
          decoration: const InputDecoration(hintText: '播放列表名称'),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx),
            child: const Text('取消'),
          ),
          TextButton(
            onPressed: () => Navigator.pop(ctx, controller.text),
            child: const Text('创建'),
          ),
        ],
      ),
    );
    if (name != null && name.trim().isNotEmpty) {
      await player.createPlaylist(name);
    }
  }

  void _openPlaylist(BuildContext context, String name) {
    Navigator.push(
      context,
      MaterialPageRoute(
        builder: (_) => PlaylistDetailScreen(playlistName: name),
      ),
    );
  }
}

/// 单个播放列表详情页。
class PlaylistDetailScreen extends StatelessWidget {
  final String playlistName;

  const PlaylistDetailScreen({super.key, required this.playlistName});

  @override
  Widget build(BuildContext context) {
    final player = context.watch<MusicPlayerProvider>();
    final tracks = player.tracksInPlaylist(playlistName);

    return Scaffold(
      appBar: AppBar(title: Text(playlistName)),
      body: tracks.isEmpty
          ? const Center(child: Text('播放列表为空'))
          : ListView.builder(
              itemCount: tracks.length,
              itemBuilder: (context, index) {
                final music = tracks[index];
                return MusicListItem(
                  music: music,
                  isPlaying:
                      player.currentMusic?.id == music.id && player.isPlaying,
                  onTap: () => player.play(music),
                  trailingExtra: IconButton(
                    icon: const Icon(Icons.remove_circle_outline),
                    onPressed: () =>
                        player.removeFromNamedPlaylist(playlistName, music.id),
                  ),
                );
              },
            ),
    );
  }
}
