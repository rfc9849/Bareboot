#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TOOLS="$ROOT/tools/gpu"
BUILD="$ROOT/dist/.gpu-host"
OUT="$ROOT/dist/gpu/host"
: "${ANDROID_NDK_HOME:?Install the Android NDK and set ANDROID_NDK_HOME to its directory, or use BAREBOOT_GPU=0 to build without the GPU bundle.}"
TAG="$(uname -s | tr '[:upper:]' '[:lower:]')-x86_64"
NDK="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$TAG/bin"
CC="$NDK/aarch64-linux-android30-clang"
CXX="$NDK/aarch64-linux-android30-clang++"
if [[ ! -x "$CC" || ! -x "$CXX" || ! -x "$NDK/llvm-ar" || ! -x "$NDK/llvm-strip" ]]; then
    echo "Android NDK tools are missing under $NDK. Set ANDROID_NDK_HOME to an NDK installed for this host, or use BAREBOOT_GPU=0 to build without the GPU bundle." >&2
    exit 1
fi
mkdir -p "$BUILD" "$OUT"

source_tree() {
    local name=$1 url=$2 revision=$3
    if [[ ! -d "$BUILD/$name/.git" ]]; then
        git init -q "$BUILD/$name"
    fi
    if ! git -C "$BUILD/$name" cat-file -e "$revision^{commit}" 2>/dev/null; then
        git -C "$BUILD/$name" fetch --depth 1 "$url" "$revision"
    fi
    git -C "$BUILD/$name" checkout -q --force --detach "$revision"
    git -C "$BUILD/$name" apply "$TOOLS/$name.patch"
}
source_tree gfxstream https://github.com/google/gfxstream.git 513f2e424dbc183b8adf9c884099220ae18b9f97
source_tree rutabaga https://github.com/magma-gpu/rutabaga_gfx.git 87273a082c89aa8ef6a63e9fa8da7cc56eb573c6
mkdir -p "$BUILD/nativewindow"
cp -R "$TOOLS/nativewindow/." "$BUILD/nativewindow/"
cp "$TOOLS/Cargo.lock" "$BUILD/rutabaga/Cargo.lock"
cp "$TOOLS/ahb_host.rs" "$BUILD/rutabaga/kumquat/server/src/bareboot_ahb.rs"
cp "$ROOT/rust/crates/protocol/src/ahb_wire.rs" "$BUILD/rutabaga/kumquat/server/src/bareboot_ahb_wire.rs"

GFX="$BUILD/gfxstream"
cat > "$BUILD/android.cross" <<EOF
[binaries]
c = '$CC'
cpp = '$CXX'
ar = '$NDK/llvm-ar'
strip = '$NDK/llvm-strip'
pkg-config = 'false'

[host_machine]
system = 'android'
cpu_family = 'aarch64'
cpu = 'armv8'
endian = 'little'

[built-in options]
c_args = ['-DANDROID', '-DANDROID_API_LEVEL=30', '-DHAVE_MEMFD_CREATE', '-DVK_USE_PLATFORM_ANDROID_KHR', '-I$GFX/third_party/x11/include', '-I$GFX/third_party/xcb/include', '-I$GFX/third_party/android/include']
cpp_args = ['-DANDROID', '-DANDROID_API_LEVEL=30', '-DHAVE_MEMFD_CREATE', '-DVK_USE_PLATFORM_ANDROID_KHR', '-I$GFX/third_party/x11/include', '-I$GFX/third_party/xcb/include', '-I$GFX/third_party/android/include']
EOF
meson setup --reconfigure "$BUILD/backend" "$GFX" --cross-file "$BUILD/android.cross" \
    --buildtype=release -Dgfxstream-build=host -Ddecoders=gles,vulkan,composer -Dbuild-tests=false
ninja -C "$BUILD/backend"

(
    cd "$BUILD/rutabaga"
    export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$CC"
    export GFXSTREAM_PATH="$BUILD/backend/host"
    export RUSTFLAGS='-C link-arg=-Wl,-rpath,/data/local/bareboot/gpu-host'
    cargo +1.88.0 build --locked --release -p kumquat_virtio --features gfxstream \
        --target aarch64-linux-android
)
install -m 0755 "$BUILD/rutabaga/target/aarch64-linux-android/release/kumquat" "$OUT/"
install -m 0644 "$BUILD/backend/host/libgfxstream_backend.so" "$OUT/"
"$CC" -O2 -Wall -Wextra -Werror -Wno-deprecated-declarations \
    -Wl,-z,max-page-size=16384 "$TOOLS/media_host.c" \
    -lmediandk -lnativewindow -lEGL -lGLESv2 -ldl -o "$OUT/bareboot-media"
"$NDK/llvm-strip" --strip-unneeded "$OUT/kumquat" "$OUT/libgfxstream_backend.so" "$OUT/bareboot-media"
mkdir -p "$OUT/licenses"
cp "$GFX/LICENSE" "$OUT/licenses/gfxstream"
cp "$BUILD/rutabaga/LICENSE" "$OUT/licenses/rutabaga"
for license in "$GFX"/third_party/*/LICENSE; do
    cp "$license" "$OUT/licenses/$(basename "$(dirname "$license")")"
done
(
    cd "$BUILD/rutabaga"
    cargo +1.88.0 tree --locked -e normal --target aarch64-linux-android \
        -p kumquat_virtio --features gfxstream --prefix none --format '{p}'
) | sort -u | while read -r name version rest; do
    crate="$name-${version#v}"
    for source in "${CARGO_HOME:-$HOME/.cargo}"/registry/src/*/"$crate"; do
        for license in "$source"/LICENSE* "$source"/COPYING* "$source"/NOTICE* "$source"/UNLICENSE; do
            [[ -f "$license" ]] || continue
            mkdir -p "$OUT/licenses/$crate"
            cp "$license" "$OUT/licenses/$crate/"
        done
    done
done
