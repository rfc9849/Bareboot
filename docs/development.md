# Development

```sh
./build.sh                                  # tests, APK, dist/bareboot-module.zip
ANDROID_NDK_HOME=... BAREBOOT_GPU=1 ./build.sh   # with the GPU bundle
cargo build --release --manifest-path tools/desktop/Cargo.toml
```

Needs Rust with `aarch64-unknown-linux-musl`, JDK 17, the Android SDK, and Gradle. The GPU build also needs the NDK, Meson, Ninja, Python Mako, Rust 1.88 with `aarch64-linux-android`, and Docker on ARM64.

## Tests

```sh
cd rust && cargo test
python3 tools/check_16k.py
python3 tools/device_smoke.py --rootfs debian-trixie-arm64.tar.xz   # phone over USB
```

An Android 16 ARM64 emulator rooted with [rootAVD](https://gitlab.com/newbit/rootAVD) works for everything except the GPU.

## Rules

- Linux runs natively. No emulation or PRoot.
- SELinux stays Enforcing.
- Bump `PROTOCOL_VERSION` in `rust/crates/protocol` and `BarebootClient.kt` together when the IPC changes.
