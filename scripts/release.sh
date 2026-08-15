#!/usr/bin/env bash
# JimMusic 本地发布打包脚本。
# 收集后端 release 产物与插件、Flutter Android APK/AAB、Web，打包到 dist/ 目录，
# 生成 SHA-256 校验和清单。HarmonyOS HAP 需 flutter_flutter(ohos) SDK，见 harmonyos 流程。
#
# 用法：
#   bash scripts/release.sh [VERSION]    # VERSION 默认取 git describe 或 0.1.0
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERSION="${1:-$(git -C "$ROOT" describe --tags --always 2>/dev/null || echo '0.1.0')}"
VERSION="${VERSION#v}"
DIST="$ROOT/dist"

log()  { printf '\033[1;35m[release]\033[0m %s\n' "$*"; }
die()  { printf '\033[1;31m[release][error]\033[0m %s\n' "$*" >&2; exit 1; }
command_exists() { command -v "$1" >/dev/null 2>&1; }

# ---------- 后端 ----------
release_backend() {
  command_exists cargo || die "未找到 cargo"
  log "构建后端（release）"
  (cd "$ROOT/backend" && cargo build --locked --release --workspace)

  local TB="$ROOT/backend/target/release"
  local OUT="$DIST/jimmusic-backend-$VERSION"
  mkdir -p "$OUT"
  cp "$TB/plugin-manager" "$OUT/" 2>/dev/null || true
  cp "$TB"/*.so "$OUT/" 2>/dev/null || true
  cp "$TB"/*.dylib "$OUT/" 2>/dev/null || true
  cp "$TB"/*.dll "$OUT/" 2>/dev/null || true
  tar -czf "$OUT.tar.gz" -C "$DIST" "jimmusic-backend-$VERSION"
  log "后端产物：$OUT.tar.gz"
}

# ---------- Flutter ----------
release_flutter() {
  command_exists flutter || die "未找到 flutter"
  command_exists npm || die "未找到 npm，无法构建浏览器 Helia 节点"
  command_exists cargo-ndk || die "未找到 cargo-ndk 4.1.2，无法把原生 Rust 节点打入 Android"
  cargo ndk --version | grep -Fq '4.1.2' || die "cargo-ndk 必须固定为 4.1.2"
  local FA="$ROOT/flutter_app"
  (cd "$FA/web_node" && npm ci && npm audit --omit=dev --audit-level=high && npm run build && npm run test:interop)
  (cd "$FA" && flutter pub get)

  log "构建 Android 应用内 Rust 节点"
  rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
  (cd "$ROOT/backend" && cargo ndk -p 21 \
    -t arm64-v8a -t armeabi-v7a -t x86_64 \
    -o "$FA/android/app/src/main/jniLibs" \
    build --locked --release -p app-core)

  log "构建 Android APK"
  (cd "$FA" && flutter build apk --release)
  cp "$FA/build/app/outputs/flutter-apk/app-release.apk" \
     "$DIST/jimmusic-$VERSION.apk"

  log "构建 Android AppBundle (AAB)"
  (cd "$FA" && flutter build appbundle --release)
  cp "$FA/build/app/outputs/bundle/release/app-release.aab" \
     "$DIST/jimmusic-$VERSION.aab"

  log "构建 Web"
  (cd "$FA" && flutter build web --release)
  tar -czf "$DIST/jimmusic-web-$VERSION.tar.gz" -C "$FA/build" web
}

# ---------- 校验和 ----------
checksums() {
  log "生成 SHA-256 校验和"
  (cd "$DIST" && find . -maxdepth 1 -type f -print0 | sort -z | \
     xargs -0 sha256sum > SHA256SUMS)
  cat "$DIST/SHA256SUMS"
}

main() {
  rm -rf "$DIST"
  mkdir -p "$DIST"
  log "版本：$VERSION"
  release_backend
  release_flutter
  checksums
  log "发布产物已生成于 $DIST"
}

main "$@"
