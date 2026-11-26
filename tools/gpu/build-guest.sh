#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BUILD="$ROOT/dist/.gpu-guest"
OUT="$ROOT/dist/gpu/guest"
SRC="$BUILD/mesa"
REV=e175081251f7d61f6c7790db08d7c3e6f6e77ae3
if ! command -v docker >/dev/null; then
    echo "Install Docker and start an ARM64 Docker engine, or use BAREBOOT_GPU=0 to build without the GPU bundle." >&2
    exit 1
fi
if ! arch="$(docker info --format '{{.Architecture}}')"; then
    echo "Start the Docker engine or select a working ARM64 Docker context, or use BAREBOOT_GPU=0 to build without the GPU bundle." >&2
    exit 1
fi
case "$arch" in
    aarch64|arm64) ;;
    *) echo "The Docker engine reports $arch. Select an ARM64 Docker engine, or use BAREBOOT_GPU=0 to build without the GPU bundle." >&2; exit 1 ;;
esac
mkdir -p "$BUILD" "$OUT"
if [[ ! -d "$SRC/.git" ]]; then
    git init -q "$SRC"
fi
if ! git -C "$SRC" cat-file -e "$REV^{commit}" 2>/dev/null; then
    git -C "$SRC" fetch --depth 1 https://gitlab.freedesktop.org/mesa/mesa.git "$REV"
fi
git -C "$SRC" checkout -q --force --detach "$REV"
git -C "$SRC" apply "$ROOT/tools/gpu/mesa.patch"
cp "$ROOT/rust/crates/display/bareboot-ahb-v1.xml" "$SRC/src/vulkan/wsi/bareboot-ahb-v1.xml"
for abi in glibc musl; do
    case "$abi" in
        glibc) image=debian@sha256:3cda3ca472ce45507a5109c6a3907093e24d9ae67d55954c9a07a45665dafba2 ;;
        musl) image=alpine@sha256:f59abe656b692d901e184be3445e084786602108599d2a51d229c0f5a48c71bf ;;
    esac
    docker run --rm -i --platform linux/arm64 -v "$BUILD:/build" -v "$OUT:/out" \
    -v "$ROOT/tools/gpu:/tools:ro" \
    "$image" /bin/sh -s "$abi" <<'EOF'
set -eu
abi=$1
case "$abi" in
glibc)
apt-get update -qq
DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends \
    ca-certificates build-essential meson ninja-build pkg-config python3-mako python3-yaml \
    libdrm-dev libvulkan-dev glslang-tools bison flex libx11-dev libxcb1-dev libx11-xcb-dev \
    libxcb-dri3-dev libxcb-present-dev libxcb-shm0-dev libxshmfence-dev libxrandr-dev libxfixes-dev \
    libwayland-dev libwayland-egl-backend-dev wayland-protocols libexpat1-dev zlib1g-dev libglvnd-dev patchelf rustc cargo bindgen libclang-dev \
    python3-packaging libavcodec-dev libavutil-dev
    glvnd_opts="-Dglvnd=enabled -Dglvnd-vendor-name=bareboot"
    ;;
musl)
    apk add --no-cache build-base pkgconf meson ninja py3-mako py3-yaml libdrm-dev \
        vulkan-loader-dev glslang glslang-dev bison flex libx11-dev libxcb-dev libxshmfence-dev \
        libxrandr-dev libxfixes-dev wayland-dev wayland-protocols expat-dev zlib-dev patchelf rust cargo clang-dev \
        rust-bindgen py3-packaging
    glvnd_opts="-Dglvnd=disabled"
    ;;
esac
dest=/out/$abi
prefix=/run/bareboot-gpu/guest/$abi
rm -rf "/build/install-$abi"
meson setup --reconfigure "/build/obj-$abi" /build/mesa --prefix="$prefix" --libdir=lib --buildtype=release \
    -Dc_link_args="-Wl,-z,max-page-size=16384 -Wl,-rpath,\$ORIGIN" \
    -Dcpp_link_args="-Wl,-z,max-page-size=16384 -Wl,-rpath,\$ORIGIN" \
    -Dvulkan-drivers=gfxstream -Dgallium-drivers=zink -Dplatforms=x11,wayland -Dvirtgpu_kumquat=true \
    -Dglx=disabled -Degl=enabled -Dgbm=disabled -Dllvm=disabled -Dopengl=true \
    -Dgles1=disabled -Dgles2=enabled $glvnd_opts -Dvulkan-layers= -Dtools= -Dbuild-tests=false -Dvideo-codecs=
ninja -C "/build/obj-$abi"
DESTDIR="/build/install-$abi" ninja -C "/build/obj-$abi" install
mkdir -p /out/bin /out/licenses "$dest/bin" "$dest/lib" "$dest/share/vulkan/icd.d" "$dest/share/bareboot-gpu"
install -m 0644 "/build/install-$abi$prefix/lib/libvulkan_gfxstream.so" "$dest/lib/"
install -m 0644 "/build/install-$abi$prefix/share/vulkan/icd.d/gfxstream_vk_icd.aarch64.json" "$dest/share/vulkan/icd.d/"
for lib in "/build/install-$abi$prefix/lib"/libgallium-*.so; do
    install -m 0644 "$lib" "$dest/lib/"
    strip --strip-unneeded "$dest/lib/$(basename "$lib")"
done
case "$abi" in
glibc)
    mkdir -p "$dest/share/glvnd/egl_vendor.d" "$dest/lib/hwdec"
    cc -shared -fPIC -O2 -Wall -Wextra -Werror -Wno-deprecated-declarations \
        -Wl,-soname,libavcodec.so.63 -Wl,-z,max-page-size=16384 \
        /tools/hwdec_avcodec.c -Wl,--no-as-needed -lavcodec -lavutil -ldl -lpthread \
        -o "$dest/lib/hwdec/libavcodec.so.63"
    strip --strip-unneeded "$dest/lib/hwdec/libavcodec.so.63"
    install -m 0644 "/build/install-$abi$prefix/lib/libEGL_bareboot.so.0.0.0" "$dest/lib/libEGL_bareboot.so.0"
    patchelf --set-rpath '$ORIGIN' "$dest/lib/libEGL_bareboot.so.0"
    strip --strip-unneeded "$dest/lib/libEGL_bareboot.so.0"
    cat > "$dest/share/glvnd/egl_vendor.d/10_bareboot.json" <<JSON
{
    "file_format_version" : "1.0.0",
    "ICD" : {
        "library_path" : "$prefix/lib/libEGL_bareboot.so.0"
    }
}
JSON
    ;;
musl)
    for base in libEGL.so.1 libGLESv2.so.2; do
        src=$(readlink -f "/build/install-$abi$prefix/lib/$base")
        install -m 0644 "$src" "$dest/lib/$base"
        patchelf --set-rpath '$ORIGIN' "$dest/lib/$base"
        strip --strip-unneeded "$dest/lib/$base"
    done
    ;;
esac
install -m 0755 /tools/run /out/bin/bareboot-gpu
cc -O2 -Wl,-z,max-page-size=16384 -Wall -Wextra -Wno-missing-field-initializers /tools/compute.c -lvulkan -o "$dest/bin/check"
mkdir -p "$dest/lib/gbm"
cc -shared -fPIC -O2 -Wall -Wextra -Werror -fvisibility=hidden -Wl,-z,max-page-size=16384 \
    -I/build/mesa/src/gbm/main /tools/gbm_backend.c -lvulkan -o "$dest/lib/gbm/bareboot_gbm.so"
strip --strip-unneeded "$dest/lib/gbm/bareboot_gbm.so"
glslangValidator -V /tools/compute.comp -o "$dest/share/bareboot-gpu/compute.spv"
strip --strip-unneeded "$dest/lib/libvulkan_gfxstream.so" "$dest/bin/check"
cp /build/mesa/docs/license.rst /out/licenses/mesa
EOF
done
