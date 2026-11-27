#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
DIST="$ROOT/dist"
GPU="${BAREBOOT_GPU:-1}"
CARGO_OFFLINE="${BAREBOOT_CARGO_OFFLINE:-1}"
GRADLE_BIN="${GRADLE_BIN:-$(command -v gradle || find "$HOME/.gradle/wrapper/dists" -path "*/bin/gradle" -type f 2>/dev/null | sort | tail -1)}"

if [[ -n "${BAREBOOT_KEYSTORE:-}" ]]; then
    : "${BAREBOOT_KEYSTORE_PASSWORD:?Set BAREBOOT_KEYSTORE_PASSWORD to sign the release APK}"
    : "${BAREBOOT_KEY_ALIAS:?Set BAREBOOT_KEY_ALIAS to sign the release APK}"
    : "${BAREBOOT_KEY_PASSWORD:?Set BAREBOOT_KEY_PASSWORD to sign the release APK}"
fi

sha256_check() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum -c "$@"
    else
        shasum -a 256 -c "$@"
    fi
}

mkdir -p "$DIST"

echo "==> Building static aarch64-unknown-linux-musl bareboot"
CARGO_NET_ARGS=(--locked)
if [[ "$CARGO_OFFLINE" != 0 ]]; then
    CARGO_NET_ARGS+=(--offline)
fi
(
  cd "$ROOT/rust"
  cargo test "${CARGO_NET_ARGS[@]}"
  cargo clippy "${CARGO_NET_ARGS[@]}" --target aarch64-unknown-linux-musl --all-targets -- -D warnings
  cargo build "${CARGO_NET_ARGS[@]}" --release --target aarch64-unknown-linux-musl
)

BIN="$ROOT/rust/target/aarch64-unknown-linux-musl/release/bareboot"
cp "$BIN" "$DIST/bareboot-aarch64-musl"

echo "==> Building the screen host helper"
: "${ANDROID_NDK_HOME:?Install the Android NDK and set ANDROID_NDK_HOME to its directory to build the screen host helper.}"
NDK_BIN="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$(uname -s | tr '[:upper:]' '[:lower:]')-x86_64/bin"
"$NDK_BIN/aarch64-linux-android30-clang" -shared -fPIC -O2 -Wall -Wextra -Werror -Wl,-z,max-page-size=16384 \
    "$ROOT/tools/screen/screen_jni.c" -landroid -lnativewindow -o "$DIST/libbareboot_screen.so"

if [[ "$GPU" == prebuilt ]]; then
    echo "==> Using prebuilt GPU bundle in $DIST/gpu"
    [[ -d "$DIST/gpu/host" && -d "$DIST/gpu/guest" ]] || { echo "Missing prebuilt GPU bundle under $DIST/gpu" >&2; exit 1; }
elif [[ "$GPU" != 0 ]]; then
    echo "==> Building the GPU bundle"
    bash "$ROOT/tools/gpu/build-host.sh"
    bash "$ROOT/tools/gpu/build-guest.sh"
fi

echo "==> Building Android APK"
(
  cd "$ROOT/android"
  "$GRADLE_BIN" :app:assembleDebug :app:assembleRelease --no-daemon
)
cp "$ROOT/android/app/build/outputs/apk/debug/app-debug.apk" "$DIST/bareboot-debug.apk"
rm -f "$DIST/bareboot-release.apk" "$DIST/bareboot-release-unsigned.apk"
APK_DIR="$ROOT/android/app/build/outputs/apk/release"
if [[ -n "${BAREBOOT_KEYSTORE:-}" ]]; then
    cp "$APK_DIR/app-release.apk" "$DIST/bareboot-release.apk"
else
    cp "$APK_DIR/app-release-unsigned.apk" "$DIST/bareboot-release-unsigned.apk"
fi

echo "==> Packaging the root module zip (not installed automatically)"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
cp -R "$ROOT/ksu-module/"* "$STAGE/"
mkdir -p "$STAGE/bin"
cp "$BIN" "$STAGE/bin/bareboot"
install -m 0755 "$ROOT/rust/target/aarch64-unknown-linux-musl/release/bareboot-display" "$STAGE/bin/bareboot-display"
SCREEN_APK="$APK_DIR/app-release.apk"
[[ -f "$SCREEN_APK" ]] || SCREEN_APK="$APK_DIR/app-release-unsigned.apk"
install -m 0644 "$SCREEN_APK" "$STAGE/bin/bareboot-screen.apk"
install -m 0644 "$DIST/libbareboot_screen.so" "$STAGE/bin/libbareboot_screen.so"

SLIRP="$DIST/.slirp4netns-1.3.7"
mkdir -p "$SLIRP"
if [ ! -f "$SLIRP/slirp4netns" ]; then
    curl -fsSL https://github.com/rootless-containers/slirp4netns/releases/download/v1.3.7/slirp4netns-aarch64 -o "$STAGE/slirp4netns"
    printf '%s  %s\n' 674e84649eeeac9603449c4f2b6e104531341dc2c720c70276029c62ace17a02 "$STAGE/slirp4netns" | sha256_check
    mv "$STAGE/slirp4netns" "$SLIRP/slirp4netns"
fi
if [ ! -f "$SLIRP/COPYING" ]; then
    curl -fsSL https://raw.githubusercontent.com/rootless-containers/slirp4netns/v1.3.7/COPYING -o "$STAGE/slirp4netns-COPYING"
    printf '%s  %s\n' 58db3cfabf8ec806bc060e54ab5466a31de4a3ae80fe4a8aa02fb6b1c053c93f "$STAGE/slirp4netns-COPYING" | sha256_check
    mv "$STAGE/slirp4netns-COPYING" "$SLIRP/COPYING"
fi
(cd "$SLIRP" && sha256_check <<'SUMS'
674e84649eeeac9603449c4f2b6e104531341dc2c720c70276029c62ace17a02  slirp4netns
58db3cfabf8ec806bc060e54ab5466a31de4a3ae80fe4a8aa02fb6b1c053c93f  COPYING
SUMS
)
if [[ "$GPU" != 0 ]]; then
    mkdir -p "$STAGE/gpu/guest"
    cp -R "$DIST/gpu/host" "$STAGE/gpu/"
    for part in bin glibc musl licenses; do
        cp -R "$DIST/gpu/guest/$part" "$STAGE/gpu/guest/"
    done
    cat "$ROOT/tools/gpu/sepolicy.rule" >> "$STAGE/sepolicy.rule"
fi

install -m 0755 "$SLIRP/slirp4netns" "$STAGE/bin/slirp4netns"
cp "$SLIRP/COPYING" "$STAGE/slirp4netns-COPYING"
printf '%s\n' https://github.com/rootless-containers/slirp4netns/releases/tag/v1.3.7 > "$STAGE/slirp4netns-SOURCE"
chmod 0755 "$STAGE/service.sh" "$STAGE/action.sh" "$STAGE/hotinstall.sh" "$STAGE/uninstall.sh" "$STAGE/bin/bareboot"
rm -f "$DIST/bareboot-module.zip"
(cd "$STAGE" && zip -qr "$DIST/bareboot-module.zip" .)

ls -lh "$DIST"
