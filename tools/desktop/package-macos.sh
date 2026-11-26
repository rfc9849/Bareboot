#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TARGET=aarch64-apple-darwin
MANIFEST="$ROOT/tools/desktop/Cargo.toml"
DIST="$ROOT/dist"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

cargo build --locked --release --manifest-path "$MANIFEST" --target-dir "$ROOT/tools/desktop/target" --target "$TARGET"
cp "$ROOT/tools/desktop/target/$TARGET/release/bareboot-desktop" "$STAGE/"
cp "$ROOT/LICENSE" "$STAGE/"
BIN="$STAGE/bareboot-desktop"
while IFS= read -r dependency; do
    case "$dependency" in
        /System/Library/*|/usr/lib/*) ;;
        */libav*.dylib|*/libsw*.dylib)
            install_name_tool -change "$dependency" "@rpath/${dependency##*/}" "$BIN"
            ;;
        *) printf 'Unexpected library: %s\n' "$dependency" >&2; exit 1 ;;
    esac
done < <(otool -L "$BIN" | awk 'NR > 1 { print $1 }')
install_name_tool -add_rpath /opt/homebrew/opt/ffmpeg/lib "$BIN"
codesign --force --sign - "$BIN"
codesign --verify "$BIN"
mkdir -p "$DIST"
(cd "$STAGE" && zip -q "$STAGE/bareboot-desktop-macos-arm64.zip" bareboot-desktop LICENSE)
mv "$STAGE/bareboot-desktop-macos-arm64.zip" "$DIST/"
