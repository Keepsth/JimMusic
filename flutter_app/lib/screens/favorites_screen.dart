import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../providers/music_player_provider.dart';
import '../widgets/music_list_item.dart';

/// 收藏页：展示已收藏曲目。
class FavoritesScreen extends StatelessWidget {
  const FavoritesScreen({super.key});

  @override
  Widget build(BuildContext context) {
    final player = context.watch<MusicPlayerProvider>();
    final favorites = player.favorites;

    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const Padding(
          padding: EdgeInsets.fromLTRB(16, 16, 16, 8),
          child: Text(
            '我的收藏',
            style: TextStyle(fontSize: 20, fontWeight: FontWeight.bold),
          ),
        ),
        Expanded(
          child: favorites.isEmpty
              ? const Center(child: Text('还没有收藏任何歌曲，点击列表中的心形图标收藏吧'))
              : ListView.builder(
                  itemCount: favorites.length,
                  itemBuilder: (context, index) {
                    final music = favorites[index];
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
