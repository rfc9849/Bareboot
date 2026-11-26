# Kernel features

A stock Android kernel runs shells, packages, SSH, and GUI apps. Some features need kernel options most phones don't ship. The app's **System check** shows what your kernel has.

| Option | Needed for |
|---|---|
| `CONFIG_PID_NS` | systemd/OpenRC, Docker, isolated network |
| `CONFIG_IPC_NS`, `CONFIG_POSIX_MQUEUE` | Docker without extra flags |
| `CONFIG_NF_TABLES` | Default iptables in Debian and Arch |
| `CONFIG_NETFILTER_XT_MATCH_ADDRTYPE` | Docker networking |
| `CONFIG_USER_NS` | Flatpak, rootless Podman |

## Custom kernels

Test with `fastboot boot` before flashing, and keep the stock boot image. Options that change the kernel ABI, such as `CONFIG_SYSVIPC`, can stop vendor modules from loading and leave the phone unbootable.

Pixel 10 Pro: [rfc9849/kernel_google_blazer_6.12](https://github.com/rfc9849/kernel_google_blazer_6.12) has everything above except user namespaces.
