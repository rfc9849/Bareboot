# Install

You need an ARM64 phone rooted with KernelSU or Magisk, Android 11+, and SELinux Enforcing. The kernel must support `SO_PEERPIDFD`. Linux 6.1 needs a backport of this option.

1. Install `bareboot-module.zip` from **Modules** in your root manager. Magisk and older KernelSU need a reboot.
2. Install the APK and open Bareboot.
3. If the app says it isn't allowed, tap **Action** on the Bareboot module. Repeat after reinstalling the app.
4. Pick a shared folder and a distribution.

## Update

Install the new module, then the APK from the same build.

## Uninstall

Remove the module and the app. Your images stay in `/data/adb/bareboot` unless **Delete images with the module** is on in the app's settings.

## Problems

- **Service not running:** check the module is enabled, reboot if needed, read `/data/adb/bareboot/barebootd.log`.
- **App not allowed:** tap **Action** on the module.
- **Service exits at start:** SELinux must be Enforcing. If the log says `SO_PEERPIDFD check failed` with `Protocol not available`, use a kernel that supports `SO_PEERPIDFD`.

The app's **Diagnostics** screen shows the logs.
