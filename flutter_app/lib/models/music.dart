class Music {
  final String id;
  final String title;
  final String artist;
  final String album;
  final String duration;
  final String? albumArt;
  final String? filePath;

  Music({
    required this.id,
    required this.title,
    required this.artist,
    required this.album,
    required this.duration,
    this.albumArt,
    this.filePath,
  });

  // 创建演示数据的工厂方法
  factory Music.demo(int index) {
    final demoTracks = [
      {
        'title': 'Shape of You',
        'artist': 'Ed Sheeran',
        'album': '÷ (Divide)',
        'duration': '3:53'
      },
      {
        'title': 'Blinding Lights', 
        'artist': 'The Weeknd',
        'album': 'After Hours',
        'duration': '3:20'
      },
      {
        'title': 'Watermelon Sugar',
        'artist': 'Harry Styles', 
        'album': 'Fine Line',
        'duration': '2:54'
      },
      {
        'title': 'Levitating',
        'artist': 'Dua Lipa',
        'album': 'Future Nostalgia',
        'duration': '3:23'
      },
      {
        'title': 'Good 4 U',
        'artist': 'Olivia Rodrigo',
        'album': 'SOUR',
        'duration': '2:58'
      },
    ];

    final track = demoTracks[index % demoTracks.length];
    return Music(
      id: 'demo_$index',
      title: track['title']!,
      artist: track['artist']!,
      album: track['album']!,
      duration: track['duration']!,
      albumArt: 'https://picsum.photos/300/300?random=$index',
    );
  }
}
