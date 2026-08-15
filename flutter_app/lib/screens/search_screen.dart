import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../providers/music_player_provider.dart';
import '../widgets/music_list_item.dart';

/// 搜索页：按关键字搜索标题/艺术家/专辑。
class SearchScreen extends StatelessWidget {
  const SearchScreen({super.key});

  @override
  Widget build(BuildContext context) {
    final player = context.watch<MusicPlayerProvider>();
    final results = player.filteredLibrary;

    return Column(
      children: [
        Padding(
          padding: const EdgeInsets.all(16),
          child: TextField(
            onChanged: player.setSearchQuery,
            decoration: InputDecoration(
              hintText: '搜索歌曲、艺术家或专辑',
              prefixIcon: const Icon(Icons.search),
              border: OutlineInputBorder(
                borderRadius: BorderRadius.circular(24),
              ),
              filled: true,
            ),
          ),
        ),
        Expanded(
          child: results.isEmpty
              ? const Center(child: Text('没有匹配的结果'))
              : ListView.builder(
                  itemCount: results.length,
                  itemBuilder: (context, index) {
                    final music = results[index];
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
}
