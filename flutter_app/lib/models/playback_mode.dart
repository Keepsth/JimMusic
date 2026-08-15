import 'dart:math';

/// 播放模式（PLR-102）：顺序 / 单曲循环 / 列表循环 / 随机。
enum PlaybackMode { sequence, repeatOne, repeatAll, shuffle }

/// 桥自动切歌事件的处理决策。
enum PlaybackDecision { accept, replayCurrent, stop }

/// 队列边界逻辑（纯函数，可测）：
/// - 桥只做 +1 循环推进，[isAdvance] 表示事件确实切到了新曲目；
/// - 单曲循环：自动切歌时回到当前曲目；
/// - 顺序模式：从最后一首绕回队首视为列表结束 → 停止；
/// - 列表循环与随机：接受推进。
PlaybackDecision evaluateAdvance({
  required bool isAdvance,
  required int currentIndex,
  required int advancedIndex,
  required PlaybackMode mode,
}) {
  if (!isAdvance) return PlaybackDecision.accept;
  switch (mode) {
    case PlaybackMode.repeatOne:
      return PlaybackDecision.replayCurrent;
    case PlaybackMode.sequence:
      return advancedIndex <= currentIndex
          ? PlaybackDecision.stop
          : PlaybackDecision.accept;
    case PlaybackMode.repeatAll:
    case PlaybackMode.shuffle:
      return PlaybackDecision.accept;
  }
}

/// 确定性洗牌（[random] 可注入固定种子供测试）。
List<T> shuffledPlaylist<T>(List<T> items, Random random) {
  final copy = List<T>.of(items);
  copy.shuffle(random);
  return copy;
}
