#!/usr/bin/env bash
# JimMusic 开发环境一键安装脚本。
# 安装 / 校验本三端所需的工具链：
#   - Rust（含 rustfmt、clippy）
#   - Flutter（stable）
#   - HarmonyOS/DevEco 工具链需手工安装，本脚本仅给出指引。
#
# 用法：
#   bash scripts/setup_dev_env.sh              # 安装/校验全部
#   SKIP_RUST=1 bash scripts/setup_dev_env.sh   # 跳过 Rust
#   SKIP_FLUTTER=1 bash scripts/setup_dev_env.sh # 跳过 Flutter
set -euo pipefail

log()  { printf '\033[1;32m[setup]\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m[warn]\033[0m %s\n' "$*"; }
die()  { printf '\033[1;31m[error]\033[0m %s\n' "$*" >&2; exit 1; }

command_exists() { command -v "$1" >/dev/null 2>&1; }

# ---------- Rust ----------
setup_rust() {
  if command_exists cargo && command_exists rustc; then
    log "Rust 已安装：$(rustc --version)"
  elif command_exists rustup; then
    log "rustup 已存在，安装 stable 工具链"
    rustup toolchain install stable
    rustup default stable
  else
    log "未检测到 Rust，安装 rustup（stable）"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
    # shellcheck disable=SC1091
    source "$HOME/.cargo/env"
  fi

  # 追加组件
  rustup component add rustfmt clippy 2>/dev/null || warn "rustfmt/clippy 安装失败（可忽略，仅影响 CI 校验）"

  log "Rust 最终版本：$(cargo --version); $(rustc --version)"
}

# ---------- Flutter ----------
setup_flutter() {
  if command_exists flutter; then
    log "Flutter 已安装："
    flutter --version 2>/dev/null | head -1 || true
  else
    log "未检测到 Flutter，将克隆 stable 分支到 \$HOME/flutter"
    local FLUTTER_DIR="${FLUTTER_DIR:-$HOME/flutter}"
    command_exists git || die "缺少 git，无法安装 Flutter"
    git clone --depth 1 --branch stable https://github.com/flutter/flutter.git "$FLUTTER_DIR"
    warn "请将以下路径加入 PATH：$FLUTTER_DIR/bin"
    export PATH="$FLUTTER_DIR/bin:$PATH"
  fi

  # 首次运行会下载 Dart SDK / 引擎
  flutter --version >/dev/null 2>&1 || flutter --version
  (cd "$(dirname "$0")/../flutter_app" && flutter pub get) \
    || warn "flutter pub get 未执行（可能尚未连接网络）"
}

# ---------- HarmonyOS（Flutter ohos fork 指引） ----------
setup_harmonyos_hint() {
  warn "HarmonyOS 目标使用 OpenHarmony 生态的 Flutter 适配仓库 flutter_flutter（ohos 分支），而非上游 Flutter。"
  warn "如需本地构建 HAP，请克隆并配置该 SDK："
  warn "  git clone --depth 1 --branch ohos https://gitcode.com/oh-flutter/flutter_flutter.git \$HOME/flutter-ohos"
  warn "  export PATH=\"\$HOME/flutter-ohos/bin:\$PATH\""
  warn "  cd flutter_app && flutter create --platforms ohos --org com.jimmusic . && flutter build hap --release"
  warn "另需 OpenHarmony SDK 与签名配置；或在打了 [self-hosted, harmonyos] 标签的 runner 交由 CI 构建。"
}

main() {
  log "== JimMusic 开发环境初始化 =="
  [ "${SKIP_RUST:-0}" = "1" ] || setup_rust
  [ "${SKIP_FLUTTER:-0}" = "1" ] || setup_flutter
  setup_harmonyos_hint
  log "完成。构建后端：cd backend && cargo build；运行前端：cd flutter_app && flutter run"
}

main "$@"