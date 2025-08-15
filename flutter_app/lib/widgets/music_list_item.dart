import 'package:flutter/material.dart';
import '../models/music.dart';

class MusicListItem extends StatelessWidget {
  final Music music;
  final bool isPlaying;
  final VoidCallback onTap;

  const MusicListItem({
    super.key,
    required this.music,
    required this.isPlaying,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    return ListTile(
      contentPadding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
      leading: Hero(
        tag: 'album_art_${music.id}',
        child: ClipRRect(
          borderRadius: BorderRadius.circular(8),
          child: SizedBox(
            width: 56,
            height: 56,
            child: Container(
              color: Colors.grey[800],
              child: const Icon(
                Icons.music_note,
                color: Colors.grey,
                size: 24,
              ),
            ),
          ),
        ),
      ),
      title: Text(
        music.title,
        style: TextStyle(
          color: isPlaying ? const Color(0xFF1DB954) : Colors.white,
          fontWeight: isPlaying ? FontWeight.bold : FontWeight.normal,
        ),
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
      ),
      subtitle: Text(
        '${music.artist} • ${music.album}',
        style: const TextStyle(
          color: Colors.grey,
          fontSize: 12,
        ),
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
      ),
      trailing: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Text(
            music.duration,
            style: const TextStyle(
              color: Colors.grey,
              fontSize: 12,
            ),
          ),
          const SizedBox(width: 8),
          if (isPlaying)
            const Icon(
              Icons.play_arrow,
              color: Color(0xFF1DB954),
              size: 20,
            )
          else
            const Icon(
              Icons.more_vert,
              color: Colors.grey,
              size: 20,
            ),
        ],
      ),
      onTap: onTap,
    );
  }
}
