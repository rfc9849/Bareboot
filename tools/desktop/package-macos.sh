#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TARGET="${BAREBOOT_MAC_TARGET:-aarch64-apple-darwin}"
MANIFEST="$ROOT/tools/desktop/Cargo.toml"
DIST="$ROOT/dist"
APP_NAME="Bareboot"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

cargo build --release --manifest-path "$MANIFEST" --target-dir "$ROOT/tools/desktop/target" --target "$TARGET"

BUNDLE="$STAGE/$APP_NAME.app"
mkdir -p "$BUNDLE/Contents/MacOS" "$BUNDLE/Contents/Resources"
BIN="$BUNDLE/Contents/MacOS/$APP_NAME"
cp "$ROOT/tools/desktop/target/$TARGET/release/bareboot-desktop" "$BIN"
chmod +x "$BIN"

FFMPEG_LIBDIR="$(pkg-config --variable=libdir libavcodec 2>/dev/null || true)"
while IFS= read -r dependency; do
    case "$dependency" in
        /System/Library/*|/usr/lib/*|@rpath/*) ;;
        */libav*.dylib|*/libsw*.dylib)
            install_name_tool -change "$dependency" "@rpath/${dependency##*/}" "$BIN"
            ;;
        *) printf 'Unexpected library: %s\n' "$dependency" >&2; exit 1 ;;
    esac
done < <(otool -L "$BIN" | awk 'NR > 1 { print $1 }')

for rpath in "$FFMPEG_LIBDIR" /opt/homebrew/opt/ffmpeg/lib /opt/zerobrew/opt/ffmpeg/lib /usr/local/opt/ffmpeg/lib; do
    if [[ -n "$rpath" ]]; then
        install_name_tool -add_rpath "$rpath" "$BIN" 2>/dev/null || true
    fi
done

ICON_PNG="$ROOT/docs/images/icon.png"
if [[ -f "$ICON_PNG" ]] && command -v sips >/dev/null 2>&1 && command -v iconutil >/dev/null 2>&1; then
    ICONSET="$STAGE/AppIcon.iconset"
    mkdir -p "$ICONSET"
    for sz in 16 32 128 256 512; do
        sips -z "$sz" "$sz" "$ICON_PNG" --out "$ICONSET/icon_${sz}x${sz}.png" >/dev/null
        sz2=$((sz * 2))
        sips -z "$sz2" "$sz2" "$ICON_PNG" --out "$ICONSET/icon_${sz}x${sz}@2x.png" >/dev/null
    done
    iconutil -c icns "$ICONSET" -o "$BUNDLE/Contents/Resources/AppIcon.icns"
fi

VERSION="$(awk -F '"' '/^version = "/ { print $2; exit }' "$MANIFEST")"
printf 'APPL????' > "$BUNDLE/Contents/PkgInfo"
cat > "$BUNDLE/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDisplayName</key>
    <string>Bareboot</string>
    <key>CFBundleExecutable</key>
    <string>Bareboot</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>CFBundleIdentifier</key>
    <string>ch.ausfuhrli.bareboot.desktop</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>Bareboot</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundleVersion</key>
    <string>1</string>
    <key>LSMinimumSystemVersion</key>
    <string>14.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSPrincipalClass</key>
    <string>NSApplication</string>
</dict>
</plist>
PLIST

codesign --force --deep --sign - "$BUNDLE"
codesign --verify --deep --strict "$BUNDLE"

mkdir -p "$DIST"
rm -rf "$DIST/$APP_NAME.app"
cp -R "$BUNDLE" "$DIST/$APP_NAME.app"
cp "$ROOT/LICENSE" "$STAGE/"
(cd "$STAGE" && zip -qr "$STAGE/bareboot-desktop-macos-arm64.zip" "$APP_NAME.app" LICENSE)
mv "$STAGE/bareboot-desktop-macos-arm64.zip" "$DIST/"

if [[ "${1:-}" == "install" ]]; then
    osascript -e 'tell application "Bareboot" to quit' >/dev/null 2>&1 || true
    rm -rf "/Applications/$APP_NAME.app"
    cp -R "$DIST/$APP_NAME.app" "/Applications/$APP_NAME.app"
    printf 'Installed /Applications/%s.app\n' "$APP_NAME"
fi

ls -lh "$DIST/bareboot-desktop-macos-arm64.zip"
