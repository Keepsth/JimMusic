import 'package:flutter/material.dart';
import '../models/music.dart';

enum PlayerState { stopped, playing, paused, buffering }

class MusicPlayerProvider extends ChangeNotifier {
  // 当前播放状态
  PlayerState _playerState = PlayerState.stopped;
  
  // 当前播放的音乐
  Music? _currentMusic;
  
  // 播放列表
  List<Music> _playlist = [];
  
  // 当前播放位置（秒）
  double _currentPosition = 0.0;
  
  // 音乐总时长（秒）
  double _duration = 0.0;
  
  // 当前播放索引
  int _currentIndex = 0;
  
  // 音量 (0.0 - 1.0)
  double _volume = 1.0;

  // Getters
  PlayerState get playerState => _playerState;
  Music? get currentMusic => _currentMusic;
  List<Music> get playlist => _playlist;
  double get currentPosition => _currentPosition;
  double get duration => _duration;
  int get currentIndex => _currentIndex;
  double get volume => _volume;
  
  bool get isPlaying => _playerState == PlayerState.playing;
  bool get isPaused => _playerState == PlayerState.paused;
  bool get isBuffering => _playerState == PlayerState.buffering;

  // 初始化演示播放列表
  MusicPlayerProvider() {
    _initializeDemoPlaylist();
  }

  void _initializeDemoPlaylist() {
    _playlist = List.generate(3, (index) => Music.demo(index));
    notifyListeners();
  }

  // 播放/暂停
  void togglePlayPause() {
    if (_playerState == PlayerState.playing) {
      pause();
    } else {
      play();
    }
  }

  // 播放
  void play([Music? music]) {
    if (music != null) {
      _currentMusic = music;
      _currentIndex = _playlist.indexOf(music);
    }
    
    if (_currentMusic == null && _playlist.isNotEmpty) {
      _currentMusic = _playlist[0];
      _currentIndex = 0;
    }
    
    _playerState = PlayerState.playing;
    
    // 模拟播放进度更新
    _simulatePlayback();
    
    notifyListeners();
  }

  // 暂停
  void pause() {
    _playerState = PlayerState.paused;
    notifyListeners();
  }

  // 停止
  void stop() {
    _playerState = PlayerState.stopped;
    _currentPosition = 0.0;
    notifyListeners();
  }

  // 下一首
  void next() {
    if (_playlist.isEmpty) return;
    
    _currentIndex = (_currentIndex + 1) % _playlist.length;
    
    _currentMusic = _playlist[_currentIndex];
    _currentPosition = 0.0;
    
    if (_playerState == PlayerState.playing) {
      play();
    } else {
      notifyListeners();
    }
  }

  // 上一首
  void previous() {
    if (_playlist.isEmpty) return;
    
    _currentIndex = (_currentIndex - 1 + _playlist.length) % _playlist.length;
    _currentMusic = _playlist[_currentIndex];
    _currentPosition = 0.0;
    
    if (_playerState == PlayerState.playing) {
      play();
    } else {
      notifyListeners();
    }
  }

  // 跳转到指定位置
  void seekTo(double position) {
    _currentPosition = position.clamp(0.0, _duration);
    notifyListeners();
  }

  // 设置音量
  void setVolume(double volume) {
    _volume = volume.clamp(0.0, 1.0);
    notifyListeners();
  }

  // 添加到播放列表
  void addToPlaylist(Music music) {
    _playlist.add(music);
    notifyListeners();
  }

  // 从播放列表移除
  void removeFromPlaylist(int index) {
    if (index >= 0 && index < _playlist.length) {
      _playlist.removeAt(index);
      if (_currentIndex >= _playlist.length && _playlist.isNotEmpty) {
        _currentIndex = _playlist.length - 1;
        _currentMusic = _playlist[_currentIndex];
      }
      notifyListeners();
    }
  }

  // 模拟播放进度（实际应用中会使用真正的音频播放器）
  void _simulatePlayback() {
    if (_currentMusic == null) return;
    
    // 解析时长字符串为秒数
    final parts = _currentMusic!.duration.split(':');
    _duration = double.parse(parts[0]) * 60 + double.parse(parts[1]);
    
    // 这里只是演示，实际应用中需要与音频播放器集成
    // 可以使用 just_audio 等插件来实现真正的音频播放
  }
}
