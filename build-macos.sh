#!/bin/bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT_DIR"

if [ -f "$HOME/.cargo/env" ]; then
  # shellcheck disable=SC1090
  source "$HOME/.cargo/env"
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "未找到 cargo，请先安装 Rust toolchain"
  exit 1
fi

if ! cargo tauri --version >/dev/null 2>&1; then
  echo "未找到 cargo-tauri，请先执行: cargo install tauri-cli --locked"
  exit 1
fi

if ! command -v trunk >/dev/null 2>&1; then
  echo "未找到 trunk，请先执行: cargo install trunk --locked"
  exit 1
fi

export NO_COLOR=false

echo "开始构建 macOS 应用包..."
echo "项目目录: $ROOT_DIR"

APP_NAME="music-tauri.app"
APP_PATH="$ROOT_DIR/target-current/release/bundle/macos/$APP_NAME"
DMG_PATH="$ROOT_DIR/target-current/release/bundle/macos/music-tauri-macos-arm64.dmg"
ZIP_PATH="$ROOT_DIR/target-current/release/bundle/macos/music-tauri-macos-arm64.zip"

rm -f "$DMG_PATH" "$ZIP_PATH"
rm -f "$ROOT_DIR"/target-current/release/bundle/macos/rw.*.dmg

cargo tauri build --bundles app

if [ ! -d "$APP_PATH" ]; then
  echo "未找到构建后的 .app: $APP_PATH"
  exit 1
fi

if command -v codesign >/dev/null 2>&1; then
  echo "为应用添加 ad-hoc 签名..."
  codesign --force --deep --sign - "$APP_PATH"
fi

echo "创建 ZIP 包..."
ditto -c -k --keepParent "$APP_PATH" "$ZIP_PATH"

echo "创建 DMG 包..."
hdiutil create -volname "Miku Tunes" -srcfolder "$APP_PATH" -ov -format UDZO "$DMG_PATH"

echo ""
echo "构建完成。常见输出目录："
echo "  $ROOT_DIR/target-current/release/bundle/macos/"
echo ""
echo "说明："
echo "  1. 当前 .app 内不再明文附带音源目录"
echo "  2. 已额外输出 ZIP 和 DMG，便于直接分发"
echo "  3. 若要避免 Gatekeeper 提示，仍建议后续做开发者签名和 notarization"
