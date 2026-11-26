# Usage

## Images

Each distribution is an image. Tap the gear on its card for settings: root shell, SSH, init, isolated network, port forwarding, GPU, rename, backup, enlarge, delete. Settings that change startup need the image stopped.

**Add a distribution** installs another one or imports a backup or any ARM64 rootfs archive.

## Terminal

Tabs keep running when you close the app. Ctrl+Tab and Ctrl+Shift+T work on hardware keyboards.

## Shared folder

The folder you picked is `/mnt/android` in Linux and shows up in Android's Files app.

## SSH

**SSH access** installs OpenSSH, adds your keys, and starts `sshd` with the image. Key login only. Over USB:

```sh
adb forward tcp:2222 tcp:2222
ssh -p 2222 debian@127.0.0.1
```

## Init

**Boot the distribution's init** runs systemd or OpenRC as PID 1. Needs a kernel with PID namespaces.

A script at `/etc/bareboot/autostart` runs as root every time the image starts.

## Networking

By default Linux shares the phone's network. Root in Linux can change the phone's routing, so be careful.

**Isolated network** gives the image its own network. Expose ports with **Port forwarding**.

## Command line

As root on the phone:

```sh
/data/adb/modules/bareboot/bin/bareboot images
/data/adb/modules/bareboot/bin/bareboot shell debian
```

## Known limits

- No user namespaces on stock kernels, so no Flatpak, bubblewrap, or rootless Podman.
- No camera, sensors, or audio.
