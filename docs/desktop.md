# GUI apps

Linux apps open as normal windows on your computer, connected over USB. Vulkan and OpenGL run on the phone's GPU. Experimental; tested with a Mac.

## Setup

On the computer: `adb`, Rust, and FFmpeg (`brew install ffmpeg`, or `libavcodec-dev` etc. on Linux). Allow root for the ADB shell in your root manager. Set `ANDROID_SERIAL` to the device serial from `adb devices` to select a specific device.

The module includes the GPU bundle by default. Images start with GPU acceleration when it is available. To disable it for an image, stop the image and turn off **GPU acceleration** in its settings. An image can start without GPU acceleration if the bundle is missing or the renderer fails to start.

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
- On Adreno drivers without dma-buf import, the renderer exports host-visible Vulkan memory through Android hardware buffers. OpenGL through Zink requires `VK_KHR_maintenance5`.
