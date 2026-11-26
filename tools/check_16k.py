from pathlib import Path
import struct
import sys
import zipfile

PAGE_SIZE = 16 * 1024
APK = Path("dist/bareboot-debug.apk")
DAEMON = Path("dist/bareboot-aarch64-musl")
MODULE = Path("dist/bareboot-module.zip")


def load_segments(data):
    if data[:6] != b"\x7fELF\x02\x01" or struct.unpack_from("<H", data, 18)[0] != 183:
        raise ValueError("expected a little-endian AArch64 ELF")
    offset = struct.unpack_from("<Q", data, 32)[0]
    size, count = struct.unpack_from("<HH", data, 54)
    headers = [struct.unpack_from("<IIQQQQQQ", data, offset + n * size) for n in range(count)]
    return [(h[2], h[3], h[7]) for h in headers if h[0] == 1]


def main():
    files = {DAEMON.name: DAEMON.read_bytes()}
    with zipfile.ZipFile(APK) as apk:
        libraries = [n for n in apk.namelist() if n.startswith("lib/arm64-v8a/") and n.endswith(".so")]
        if not libraries:
            raise ValueError("APK has no ARM64 libraries")
        files.update((n, apk.read(n)) for n in libraries)
    with zipfile.ZipFile(MODULE) as module:
        helper = 'bin/slirp4netns'
        files[helper] = module.read(helper)
        files["bin/bareboot-display"] = module.read("bin/bareboot-display")
        if "gpu/host/kumquat" in module.namelist():
            gpu = [
                "gpu/host/kumquat",
                "gpu/host/libgfxstream_backend.so",
                *(
                    f"gpu/guest/{abi}/{binary}"
                    for abi in ("glibc", "musl")
                    for binary in ("bin/check", "lib/libvulkan_gfxstream.so")
                ),
            ]
            files.update((name, module.read(name)) for name in gpu)
    good = True
    for name, data in files.items():
        segments = load_segments(data)
        passed = bool(segments) and all(a >= PAGE_SIZE and (o - v) % PAGE_SIZE == 0 for o, v, a in segments)
        good &= passed
        alignment = min((a for _, _, a in segments), default=0)
        print(f"{'Pass' if passed else 'Fail'}: {name}, minimum load alignment {alignment} bytes")
    return 0 if good else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, KeyError, struct.error, zipfile.BadZipFile) as e:
        raise SystemExit(str(e))
