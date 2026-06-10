#!/bin/bash
# 构建 Android APK 脚本
# 使用方法: bash build-android.sh

set -e

# 设置 Java 21 环境
export JAVA_HOME="/opt/homebrew/opt/openjdk@21"
export ANDROID_HOME="$HOME/Library/Android/sdk"
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/27.0.12077973"
export GRADLE_USER_HOME="$(pwd)/.gradle-home"
export NO_COLOR=false
export PATH="$JAVA_HOME/bin:$PATH"
BUILD_TOOLS_VERSION="36.0.0"
BUILD_TOOLS_DIR="$ANDROID_HOME/build-tools/$BUILD_TOOLS_VERSION"
UNSIGNED_APK="src-tauri/gen/android/app/build/outputs/apk/universal/release/app-universal-release-unsigned.apk"
SIGNED_APK="src-tauri/gen/android/app/build/outputs/apk/universal/release/app-universal-release-signed.apk"
ALIGNED_APK="src-tauri/gen/android/app/build/outputs/apk/universal/release/app-universal-release-aligned.apk"
DEBUG_KEYSTORE="$HOME/.android/debug.keystore"
DEBUG_KEY_ALIAS="androiddebugkey"
DEBUG_KEY_PASSWORD="android"

mkdir -p .tmp .kotlin-cache "$GRADLE_USER_HOME"
mkdir -p src-tauri/gen/android/.tmp src-tauri/gen/android/.kotlin-cache

ANDROID_TMP_DIR="$(pwd)/src-tauri/gen/android/.tmp"
ANDROID_KOTLIN_CACHE_DIR="$(pwd)/src-tauri/gen/android/.kotlin-cache"
export GRADLE_OPTS="-Djava.io.tmpdir=$ANDROID_TMP_DIR -Dkotlin.compiler.cache.dir=$ANDROID_KOTLIN_CACHE_DIR"

python3 - <<PY
from pathlib import Path
path = Path("src-tauri/gen/android/gradle.properties")
text = path.read_text()
lines = text.splitlines()
target = "org.gradle.jvmargs="
replacement = f"org.gradle.jvmargs=-Xmx4g -Dkotlin.compiler.cache.dir={Path('$ANDROID_KOTLIN_CACHE_DIR')} -Djava.io.tmpdir={Path('$ANDROID_TMP_DIR')}"
updated = []
replaced = False
for line in lines:
    if line.startswith(target):
        updated.append(replacement)
        replaced = True
    else:
        updated.append(line)
if not replaced:
    updated.insert(0, replacement)
path.write_text("\\n".join(updated) + "\\n")
PY

echo "JAVA_HOME=$JAVA_HOME"
echo "ANDROID_HOME=$ANDROID_HOME"
echo "ANDROID_NDK_HOME=$ANDROID_NDK_HOME"

# 构建 Android APK
npx tauri android build

if [ ! -f "$DEBUG_KEYSTORE" ]; then
  mkdir -p "$(dirname "$DEBUG_KEYSTORE")"
  keytool -genkeypair \
    -v \
    -keystore "$DEBUG_KEYSTORE" \
    -storepass "$DEBUG_KEY_PASSWORD" \
    -alias "$DEBUG_KEY_ALIAS" \
    -keypass "$DEBUG_KEY_PASSWORD" \
    -keyalg RSA \
    -keysize 2048 \
    -validity 10000 \
    -dname "CN=Android Debug,O=Android,C=US"
fi

rm -f "$ALIGNED_APK" "$SIGNED_APK"
"$BUILD_TOOLS_DIR/zipalign" -f -p 4 "$UNSIGNED_APK" "$ALIGNED_APK"
"$BUILD_TOOLS_DIR/apksigner" sign \
  --ks "$DEBUG_KEYSTORE" \
  --ks-key-alias "$DEBUG_KEY_ALIAS" \
  --ks-pass "pass:$DEBUG_KEY_PASSWORD" \
  --key-pass "pass:$DEBUG_KEY_PASSWORD" \
  --out "$SIGNED_APK" \
  "$ALIGNED_APK"
"$BUILD_TOOLS_DIR/apksigner" verify -v "$SIGNED_APK"

echo ""
echo "APK 构建完成！"
echo "可安装 APK: $SIGNED_APK"
echo "原始未签名 APK: $UNSIGNED_APK"
