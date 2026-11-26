# Development

```sh
./build.sh                  # includes the GPU bundle
BAREBOOT_GPU=0 ./build.sh    # omits the GPU bundle
cargo build --release --manifest-path tools/desktop/Cargo.toml
```

Both builds need Rust with `aarch64-unknown-linux-musl`, JDK 17, the Android SDK, and Gradle. The default build also needs an Android NDK (set `ANDROID_NDK_HOME` to its directory), Meson, Ninja, Python Mako, Rust 1.88 with `aarch64-linux-android`, and a running ARM64 Docker engine. The module ZIP is written to `dist/bareboot-module.zip`.

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
