/// 播放列表模型。
class Playlist {
  final String name;
  final List<String> trackIds;

  Playlist({required this.name, List<String>? trackIds})
    : trackIds = trackIds ?? [];

  Playlist copyWith({String? name, List<String>? trackIds}) =>
      Playlist(name: name ?? this.name, trackIds: trackIds ?? this.trackIds);

  Map<String, dynamic> toMap() => {'name': name, 'trackIds': trackIds};

  factory Playlist.fromMap(Map<String, dynamic> map) => Playlist(
    name: map['name'] as String,
    trackIds: (map['trackIds'] as List<dynamic>? ?? [])
        .map((e) => e as String)
        .toList(),
  );
}
