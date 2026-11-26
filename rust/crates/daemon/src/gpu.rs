use std::ffi::CString;
use std::fs::{self, Permissions};
use std::io::{self, Read};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt, chown};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use runtime::mount::GpuMounts;

const SOCKET: &str = "kumquat-gpu-0";
const MEDIA_SOCKET: &str = "bareboot-media-0";
const HOST_MOUNT: &str = "/data/local/bareboot/gpu-host";
const GPU_UID: u32 = 65534;
const GUEST_GROUP: u32 = 1023;
const GPU_FEATURES: &str =
    "VulkanAllocateHostVisibleAsUdmabuf:enabled,ExternalBlob:enabled,VulkanEnsureCachedCoherentMemoryAvailable:disabled";

pub struct Renderer {
    dir: PathBuf,
    guest: PathBuf,
    child: Option<Child>,
    media: Option<Child>,
    presentation: OwnedFd,
    encoder: OwnedFd,
}

fn trusted_file(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.uid() == 0 && meta.mode() & 0o022 == 0)
}

pub fn bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let root = exe.parent()?.parent()?.join("gpu");
    [
        "host/kumquat",
        "host/libgfxstream_backend.so",
        "host/bareboot-media",
        "guest/bin/bareboot-gpu",
        "guest/glibc/bin/check",
        "guest/glibc/lib/libvulkan_gfxstream.so",
        "guest/glibc/lib/libEGL_bareboot.so.0",
        "guest/glibc/share/vulkan/icd.d/gfxstream_vk_icd.aarch64.json",
        "guest/glibc/share/glvnd/egl_vendor.d/10_bareboot.json",
        "guest/glibc/share/bareboot-gpu/compute.spv",
        "guest/musl/bin/check",
        "guest/musl/lib/libvulkan_gfxstream.so",
        "guest/musl/lib/libEGL.so.1",
        "guest/musl/lib/libGLESv2.so.2",
        "guest/musl/share/vulkan/icd.d/gfxstream_vk_icd.aarch64.json",
        "guest/musl/share/bareboot-gpu/compute.spv",
    ]
    .iter()
    .all(|file| trusted_file(&root.join(file)))
    .then_some(root)
}

pub fn heap_present() -> bool {
    fs::metadata("/dev/dma_heap/system-uncached")
        .is_ok_and(|meta| meta.file_type().is_char_device())
}

fn prepare_host_mount() -> io::Result<()> {
    for path in ["/data/local/bareboot", HOST_MOUNT] {
        match fs::create_dir(path) {
            Ok(()) => fs::set_permissions(path, Permissions::from_mode(0o711))?,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e),
        }
        let meta = fs::symlink_metadata(path)?;
        if !meta.is_dir() || meta.uid() != 0 || meta.mode() & 0o022 != 0 || meta.mode() & 0o001 == 0
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("invalid GPU mount directory: {path}"),
            ));
        }
    }
    Ok(())
}

impl Renderer {
    pub fn start(bundle: &Path) -> io::Result<Self> {
        if !heap_present() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "system-uncached DMA heap is missing",
            ));
        }
        prepare_host_mount()?;
        let host = CString::new(bundle.join("host").as_os_str().as_bytes())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let host_mount = CString::new(HOST_MOUNT).unwrap();
        let mut template = *b"/dev/socket/bareboot-gpu-XXXXXX\0";
        if unsafe { libc::mkdtemp(template.as_mut_ptr().cast()) }.is_null() {
            return Err(io::Error::last_os_error());
        }
        let dir = PathBuf::from(std::ffi::OsStr::from_bytes(&template[..template.len() - 1]));
        let [presentation, bridge] = protocol::ahb_wire::pair()?;
        let [gpu_media, media_ahb] = protocol::ahb_wire::pair()?;
        let [display_enc, media_enc] = protocol::ahb_wire::pair()?;
        let bridge_fd = bridge.as_raw_fd();
        let gpu_media_fd = gpu_media.as_raw_fd();
        let media_ahb_fd = media_ahb.as_raw_fd();
        let media_enc_fd = media_enc.as_raw_fd();
        let mut renderer = Self {
            dir,
            guest: bundle.join("guest"),
            child: None,
            media: None,
            presentation,
            encoder: display_enc,
        };
        super::linux_impl::set_selinux_label(&renderer.dir, "u:object_r:bareboot_gpu_socket:s0")?;
        chown(&renderer.dir, Some(GPU_UID), Some(GUEST_GROUP))?;
        fs::set_permissions(&renderer.dir, Permissions::from_mode(0o750))?;

        let parent = unsafe { libc::getpid() };
        let mut server = Command::new(Path::new(HOST_MOUNT).join("kumquat"));
        server
            .args([
                "--gpu-socket-path",
                SOCKET,
                "--renderer-features",
                GPU_FEATURES,
            ])
            .arg("--parent-pid")
            .arg(parent.to_string())
            .env_clear()
            .env("BAREBOOT_AHB_FD", bridge_fd.to_string())
            .env("BAREBOOT_MEDIA_AHB_FD", gpu_media_fd.to_string())
            .current_dir(&renderer.dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let boost = runtime::scheduler::foreground_util_min();
        unsafe {
            server.pre_exec(move || {
                let _ = runtime::scheduler::set_cpu_util_min(boost);
                let context = b"u:r:bareboot_gpu_renderer:s0\0";
                let fd = libc::open(
                    c"/proc/thread-self/attr/exec".as_ptr(),
                    libc::O_WRONLY | libc::O_CLOEXEC,
                );
                if fd < 0 {
                    return Err(io::Error::last_os_error());
                }
                let wrote = libc::write(fd, context.as_ptr().cast(), context.len());
                let error = io::Error::last_os_error();
                libc::close(fd);
                if wrote != context.len() as isize {
                    return Err(error);
                }
                libc::umask(0o117);
                if libc::unshare(libc::CLONE_NEWNET | libc::CLONE_NEWNS) != 0
                    || libc::mount(
                        std::ptr::null(),
                        c"/".as_ptr(),
                        std::ptr::null(),
                        libc::MS_REC | libc::MS_PRIVATE,
                        std::ptr::null(),
                    ) != 0
                    || libc::mount(
                        host.as_ptr(),
                        host_mount.as_ptr(),
                        std::ptr::null(),
                        libc::MS_BIND,
                        std::ptr::null(),
                    ) != 0
                    || libc::mount(
                        std::ptr::null(),
                        host_mount.as_ptr(),
                        std::ptr::null(),
                        libc::MS_BIND
                            | libc::MS_REMOUNT
                            | libc::MS_RDONLY
                            | libc::MS_NOSUID
                            | libc::MS_NODEV,
                        std::ptr::null(),
                    ) != 0
                {
                    return Err(io::Error::last_os_error());
                }
                if libc::setgroups(0, std::ptr::null()) != 0
                    || libc::setgid(GUEST_GROUP) != 0
                    || libc::setuid(GPU_UID) != 0
                    || libc::syscall(
                        libc::SYS_close_range,
                        3u32,
                        u32::MAX,
                        libc::CLOSE_RANGE_CLOEXEC,
                    ) != 0
                    || libc::fcntl(bridge_fd, libc::F_SETFD, 0) != 0
                    || libc::fcntl(gpu_media_fd, libc::F_SETFD, 0) != 0
                {
                    return Err(io::Error::last_os_error());
                }
                if libc::getppid() != parent {
                    return Err(io::Error::other("GPU renderer parent exited"));
                }
                Ok(())
            });
        }
        renderer.child = Some(
            server
                .spawn()
                .map_err(|e| io::Error::other(format!("could not launch Kumquat: {e}")))?,
        );
        drop(bridge);
        drop(gpu_media);

        let host_media = CString::new(bundle.join("host").as_os_str().as_bytes())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let host_mount_media = CString::new(HOST_MOUNT).unwrap();
        let mut media_cmd = Command::new(Path::new(HOST_MOUNT).join("bareboot-media"));
        media_cmd
            .env_clear()
            .env("BAREBOOT_PARENT_PID", parent.to_string())
            .env("BAREBOOT_MEDIA_SOCKET", MEDIA_SOCKET)
            .env("BAREBOOT_MEDIA_AHB_FD", media_ahb_fd.to_string())
            .env("BAREBOOT_ENC_FD", media_enc_fd.to_string())
            .current_dir(&renderer.dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        unsafe {
            media_cmd.pre_exec(move || {
                let _ = runtime::scheduler::set_cpu_util_min(boost);
                let context = b"u:r:bareboot_media:s0 ";
                let fd = libc::open(
                    c"/proc/thread-self/attr/exec".as_ptr(),
                    libc::O_WRONLY | libc::O_CLOEXEC,
                );
                if fd < 0 {
                    return Err(io::Error::last_os_error());
                }
                let wrote = libc::write(fd, context.as_ptr().cast(), context.len());
                let error = io::Error::last_os_error();
                libc::close(fd);
                if wrote != context.len() as isize {
                    return Err(error);
                }
                libc::umask(0o117);
                if libc::unshare(libc::CLONE_NEWNET | libc::CLONE_NEWNS) != 0
                    || libc::mount(
                        std::ptr::null(),
                        c"/".as_ptr(),
                        std::ptr::null(),
                        libc::MS_REC | libc::MS_PRIVATE,
                        std::ptr::null(),
                    ) != 0
                    || libc::mount(
                        host_media.as_ptr(),
                        host_mount_media.as_ptr(),
                        std::ptr::null(),
                        libc::MS_BIND,
                        std::ptr::null(),
                    ) != 0
                    || libc::mount(
                        std::ptr::null(),
                        host_mount_media.as_ptr(),
                        std::ptr::null(),
                        libc::MS_BIND
                            | libc::MS_REMOUNT
                            | libc::MS_RDONLY
                            | libc::MS_NOSUID
                            | libc::MS_NODEV,
                        std::ptr::null(),
                    ) != 0
                {
                    return Err(io::Error::last_os_error());
                }
                if libc::setgroups(0, std::ptr::null()) != 0
                    || libc::setgid(GUEST_GROUP) != 0
                    || libc::setuid(GPU_UID) != 0
                    || libc::syscall(
                        libc::SYS_close_range,
                        3u32,
                        u32::MAX,
                        libc::CLOSE_RANGE_CLOEXEC,
                    ) != 0
                    || libc::fcntl(media_ahb_fd, libc::F_SETFD, 0) != 0
                    || libc::fcntl(media_enc_fd, libc::F_SETFD, 0) != 0
                {
                    return Err(io::Error::last_os_error());
                }
                if libc::getppid() != parent {
                    return Err(io::Error::other("media decoder parent exited"));
                }
                Ok(())
            });
        }
        renderer.media = media_cmd.spawn().ok();
        drop(media_ahb);
        drop(media_enc);
        let child = renderer.child.as_mut().unwrap();
        let stderr = child.stderr.as_ref().unwrap();
        if unsafe { libc::fcntl(stderr.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let socket = renderer.dir.join(SOCKET);
        let deadline = Instant::now() + Duration::from_secs(4);
        let initialized = loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let details = diagnostics(child);
                    break Err(io::Error::other(format!(
                        "Kumquat exited: {status}{details}"
                    )));
                }
                Err(e) => break Err(e),
                Ok(None) => (),
            }
            if fs::symlink_metadata(&socket).is_ok_and(|meta| {
                meta.file_type().is_socket()
                    && meta.uid() == GPU_UID
                    && meta.gid() == GUEST_GROUP
                    && meta.mode() & 0o777 == 0o660
            }) {
                break Ok(());
            }
            if Instant::now() >= deadline {
                let details = diagnostics(child);
                break Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("Kumquat did not create its socket{details}"),
                ));
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        initialized?;
        Ok(renderer)
    }

    pub fn presentation(&self) -> &OwnedFd {
        &self.presentation
    }

    pub fn encoder(&self) -> &OwnedFd {
        &self.encoder
    }

    pub fn mounts(&self) -> GpuMounts<'_> {
        GpuMounts {
            socket: &self.dir,
            guest: &self.guest,
        }
    }

    pub fn exit(&mut self) -> Option<String> {
        let child = self.child.as_mut()?;
        let details = diagnostics(child);
        match child.try_wait() {
            Ok(None) => None,
            Ok(Some(status)) => Some(format!("{status}{details}")),
            Err(e) => Some(e.to_string()),
        }
    }
}

fn diagnostics(child: &mut Child) -> String {
    let mut bytes = [0u8; 4096];
    let size = child
        .stderr
        .as_mut()
        .and_then(|stderr| stderr.read(&mut bytes).ok())
        .unwrap_or(0);
    let details = String::from_utf8_lossy(&bytes[..size]);
    if details.trim().is_empty() {
        String::new()
    } else {
        format!(": {}", details.trim())
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        if let Some(media) = self.media.as_mut() {
            let _ = media.kill();
            let _ = media.wait();
        }
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = fs::remove_file(self.dir.join(MEDIA_SOCKET));
        let _ = fs::remove_file(self.dir.join(SOCKET));
        let _ = fs::remove_dir(&self.dir);
    }
}
