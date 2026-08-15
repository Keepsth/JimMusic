#!/usr/bin/env bash
# JimMusic 本地 CI 校验脚本（镜像 GitHub Actions 的关键步骤）。
# 后端：fmt → clippy → test
# 前端：flutter pub get → analyze → test
#
# 用法：
#   bash scripts/ci_build.sh               # 全部
#   SKIP_BACKEND=1 bash scripts/ci_build.sh # 仅前端
#   SKIP_FLUTTER=1 bash scripts/ci_build.sh # 仅后端
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
log()  { printf '\033[1;34m[ci]\033[0m %s\n' "$*"; }
die()  { printf '\033[1;31m[ci][error]\033[0m %s\n' "$*" >&2; exit 1; }

command_exists() { command -v "$1" >/dev/null 2>&1; }

ci_backend() {
  log "== 后端校验（Rust）=="
  command_exists cargo || die "未找到 cargo，请先运行 scripts/setup_dev_env.sh"
  cd "$ROOT/backend"
  log "cargo fmt --all --check"
  cargo fmt --all --check
  log "cargo clippy --locked --all-targets --all-features -- -D warnings"
  cargo clippy --locked --all-targets --all-features -- -D warnings
  log "cargo build --locked --workspace --all-features（生成 FFI 动态库供 ABI 测试）"
  cargo build --locked --workspace --all-features
  log "cargo test --locked --all --all-features"
  cargo test --locked --all --all-features
  log "后端检查通过 ✔"
}

ci_flutter() {
  log "== 前端校验（Flutter）=="
  command_exists flutter || die "未找到 flutter，请先运行 scripts/setup_dev_env.sh"
  command_exists npm || die "未找到 npm，无法构建浏览器 Helia 节点"
  cd "$ROOT/flutter_app/web_node"
  log "npm ci / audit / Helia bundle / native interop"
  npm ci
  npm audit --omit=dev --audit-level=high
  npm run build
  npm run test:interop
  cd "$ROOT/flutter_app"
  log "flutter pub get"
  flutter pub get
  log "flutter analyze"
  flutter analyze
  log "flutter test"
  flutter test
  log "前端检查通过 ✔"
}

main() {
  log "== 发布验收报告校验器测试 =="
  command_exists node || die "未找到 Node.js，无法测试发布验收门禁"
  (cd "$ROOT" && node --test scripts/tests/validate_acceptance_report.test.mjs)
  [ "${SKIP_BACKEND:-0}" = "1" ] || ci_backend
  [ "${SKIP_FLUTTER:-0}" = "1" ] || ci_flutter
  log "全部 CI 校验通过 ✔"
}

main "$@"
