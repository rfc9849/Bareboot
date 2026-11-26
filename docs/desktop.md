# GUI apps

Linux apps open as normal windows on your computer, connected over USB. Vulkan and OpenGL run on the phone's GPU. Experimental; tested with a Mac.

## Setup

On the computer: `adb`, Rust, and FFmpeg (`brew install ffmpeg`, or `libavcodec-dev` etc. on Linux). Allow root for the ADB shell in your root manager.

For the GPU, build the module with `BAREBOOT_GPU=1` and turn on **Experimental GPU** for the image.

## Run

```sh
cargo run --release --manifest-path tools/desktop/Cargo.toml -- debian
```

Then start apps in the image:

```sh
firefox-esr &
```

Check the GPU with `/run/bareboot-gpu/guest/bin/bareboot-gpu check`.

## Limits

- No clipboard sharing
- Pointer is always an arrow
- US keyboard layout only
- Linux and Windows clients are untested
- Adreno (tested: Adreno 810): Vulkan works; OpenGL through Zink does not, because the driver lacks `VK_KHR_maintenance5`
