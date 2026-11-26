# How it works

```text
Android app ────────────┐
                        ├── Unix socket ──▶ bareboot service (root)
desktop client ── adb ──┘                       ├── holder ──▶ Linux processes   (one per running image)
                                                ├── bareboot-display              (Wayland compositor)
                                                ├── Kumquat                       (GPU renderer)
                                                └── bareboot-media                (hardware video)
```

**Service.** The module starts `bareboot daemon` at boot, outside the app's process tree, so Android doesn't kill Linux processes when it reclaims memory. The app talks to it over `/dev/socket/bareboot/barebootd.sock`.

**Images.** Each distribution is an ext4 image file on a loop device, so Linux gets a normal file system instead of Android's `/data`.

**Starting an image.** A holder process creates mount, UTS, PID, and IPC namespaces, mounts the image, `pivot_root`s into it, and starts PID 1. Linux programs then run with plain `execve`.

**GPU.** Linux apps use a Mesa Vulkan driver that sends commands to Kumquat (gfxstream) on Android, which runs them on the phone's GPU. OpenGL goes through Zink.

**Display.** `bareboot-display` is a Wayland compositor. It encodes each GPU window with the phone's hardware H.264 encoder and sends CPU-drawn windows as compressed pixels. The desktop client decodes on the computer's GPU and shows each window natively.

## Source

```text
rust/crates/protocol   IPC messages
rust/crates/runtime    images, namespaces, holder
rust/crates/daemon     service
rust/crates/cli        bareboot command
rust/crates/display    Wayland compositor
android/               app
ksu-module/            module scripts and SELinux policy
tools/desktop/         desktop client
tools/gpu/             GPU and media builds
```
