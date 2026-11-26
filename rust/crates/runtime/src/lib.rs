const BUSYBOX_PATHS: &[&str] = &[
    "/data/adb/ksu/bin/busybox",
    "/data/adb/magisk/busybox",
    "/data/adb/ap/bin/busybox",
];

pub fn busybox() -> std::io::Result<&'static str> {
    use std::os::unix::fs::MetadataExt;
    BUSYBOX_PATHS
        .iter()
        .copied()
        .find(|path| {
            std::fs::symlink_metadata(path)
                .is_ok_and(|m| m.is_file() && m.uid() == 0 && m.mode() & 0o022 == 0 && m.mode() & 0o100 != 0)
        })
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no root manager busybox found (KernelSU, Magisk, or APatch)",
            )
        })
}

#[cfg(target_os = "linux")]
pub mod auth;
pub mod kernel;
pub mod validate;

#[cfg(target_os = "linux")]
pub mod caps;
#[cfg(target_os = "linux")]
pub mod holder;
#[cfg(target_os = "linux")]
pub mod image;
#[cfg(target_os = "linux")]
pub mod loopdev;
#[cfg(target_os = "linux")]
pub mod mount;
#[cfg(target_os = "linux")]
pub mod pty;
#[cfg(target_os = "linux")]
pub mod scheduler;
#[cfg(target_os = "linux")]
mod uplink;
