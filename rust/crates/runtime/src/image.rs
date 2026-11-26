use crate::validate::{
    ANDROID_MEDIA_GID, enable_pacman_option, format_port_forwards, parse_port_forwards,
    validate_port_forwards, validate_ssh_port,
};
use crate::{busybox, loopdev, mount};
use protocol::{ImageInfo, KernelInfo, PortForward};
use std::fs::{self, OpenOptions};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::{Command, Stdio};

const MKE2FS: &str = "/system/bin/mke2fs";
const E2FSCK: &str = "/system/bin/e2fsck";
const RESIZE2FS: &str = "/system/bin/resize2fs";
const OS_LABEL_FILE: &str = "os-label";
const USERNAME_FILE: &str = "username";
const GPU_FILE: &str = "gpu";
const KEEP_AWAKE_FILE: &str = "keep-awake";
const INIT_FILE: &str = "init";
const ISOLATED_NET_FILE: &str = "isolated-net";
const SSH_PORT_FILE: &str = "ssh-port";
const PORT_FORWARDS_FILE: &str = "port-forwards";
const META_FILES: &[&str] = &[
    OS_LABEL_FILE,
    USERNAME_FILE,
    KEEP_AWAKE_FILE,
    GPU_FILE,
    INIT_FILE,
    ISOLATED_NET_FILE,
    SSH_PORT_FILE,
    PORT_FORWARDS_FILE,
];
pub const INIT_NONE: &str = "none";
pub const INIT_SYSTEM: &str = "system";
const GUEST_UID: u32 = 1000;
const EXIT_USERNAME_TAKEN: i32 = 2;
const EXIT_NEEDS_PACMAN_KEYRING: i32 = 3;

pub struct CreatedImage {
    pub image_path: std::path::PathBuf,
    pub needs_pacman_keyring: bool,
}

pub struct ImageSettings {
    pub keep_awake: bool,
    pub gpu_enabled: bool,
    pub init: String,
    pub isolated_net: bool,
    pub ssh_port: u16,
    pub port_forwards: Vec<PortForward>,
}

pub fn image_settings(distro_dir: &Path) -> ImageSettings {
    let init = read_meta(distro_dir, INIT_FILE);
    let gpu_enabled = match fs::read_to_string(distro_dir.join(GPU_FILE)) {
        Ok(value) => value.trim() == "1",
        Err(e) => e.kind() == io::ErrorKind::NotFound,
    };
    let ssh_port = read_meta(distro_dir, SSH_PORT_FILE)
        .parse::<u16>()
        .ok()
        .filter(|&p| p >= 1024)
        .unwrap_or(0);
    let port_forwards = fs::read_to_string(distro_dir.join(PORT_FORWARDS_FILE))
        .ok()
        .and_then(|s| parse_port_forwards(&s).ok())
        .filter(|f| validate_port_forwards(f, ssh_port).is_ok())
        .unwrap_or_default();
    ImageSettings {
        keep_awake: read_meta(distro_dir, KEEP_AWAKE_FILE) == "1",
        gpu_enabled,
        init: if init == INIT_SYSTEM { init } else { INIT_NONE.to_string() },
        isolated_net: read_meta(distro_dir, ISOLATED_NET_FILE) == "1",
        ssh_port,
        port_forwards,
    }
}

fn read_meta(distro_dir: &Path, file: &str) -> String {
    fs::read_to_string(distro_dir.join(file))
        .map(|s| s.trim().chars().take(64).collect())
        .unwrap_or_default()
}

pub fn list_images(distros_dir: &Path) -> Vec<ImageInfo> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(distros_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let img = entry.path().join("rootfs.img");
        if let Ok(meta) = fs::metadata(&img)
            && meta.is_file()
        {
            let dir = entry.path();
            let settings = image_settings(&dir);
            let (fs_size_bytes, fs_free_bytes) = ext4_usage(&img).unwrap_or((0, 0));
            out.push(ImageInfo {
                name,
                image_path: img.to_string_lossy().into_owned(),
                size_bytes: meta.len(),
                used_bytes: meta.blocks().saturating_mul(512),
                os_label: read_meta(&dir, OS_LABEL_FILE),
                username: read_meta(&dir, USERNAME_FILE),
                fs_size_bytes,
                fs_free_bytes,
                keep_awake: settings.keep_awake,
                init: settings.init,
                isolated_net: settings.isolated_net,
                ssh_port: settings.ssh_port,
                port_forwards: settings.port_forwards,
                busy: String::new(),
                gpu_enabled: settings.gpu_enabled,
                gpu_available: false,
                gpu_running: false,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

pub fn ext4_usage(img_path: &Path) -> io::Result<(u64, u64)> {
    use std::os::unix::fs::FileExt;
    let file = fs::File::open(img_path)?;
    let mut sb = [0u8; 0x160];
    if file.read_at(&mut sb, 1024)? != sb.len() || sb[0x38..0x3a] != [0x53, 0xef] {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "not an ext4 image"));
    }
    Ok(superblock_usage(&sb))
}

fn superblock_usage(sb: &[u8]) -> (u64, u64) {
    let u32_at = |off: usize| u64::from(u32::from_le_bytes(sb[off..off + 4].try_into().unwrap()));
    let is_64bit = u32_at(0x60) & 0x80 != 0;
    let wide = |lo: usize, hi: usize| u32_at(lo) | if is_64bit { u32_at(hi) << 32 } else { 0 };
    let block_size = 1024u64 << u32_at(0x18).min(6);
    let blocks = wide(0x04, 0x150);
    let reserved = wide(0x08, 0x154);
    let free = wide(0x0C, 0x158);
    (
        blocks.saturating_mul(block_size),
        free.saturating_sub(reserved).saturating_mul(block_size),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn configure_image(
    name: &str,
    state_dir: &Path,
    os_label: &str,
    username: &str,
    keep_awake: bool,
    init: &str,
    isolated_net: bool,
    ssh_port: u16,
    port_forwards: &[PortForward],
) -> io::Result<()> {
    let dir = state_dir.join("distros").join(name);
    if !dir.join("rootfs.img").is_file() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("no image named {name}")));
    }
    if init != INIT_NONE && init != INIT_SYSTEM {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "init must be none or system"));
    }
    validate_ssh_port(ssh_port)?;
    validate_port_forwards(port_forwards, ssh_port)?;
    fs::write(dir.join(OS_LABEL_FILE), format!("{os_label}\n"))?;
    fs::write(dir.join(USERNAME_FILE), format!("{username}\n"))?;
    fs::write(dir.join(KEEP_AWAKE_FILE), if keep_awake { "1\n" } else { "0\n" })?;
    fs::write(dir.join(INIT_FILE), format!("{init}\n"))?;
    fs::write(dir.join(ISOLATED_NET_FILE), if isolated_net { "1\n" } else { "0\n" })?;
    fs::write(dir.join(SSH_PORT_FILE), format!("{ssh_port}\n"))?;
    let formatted = format_port_forwards(port_forwards);
    if formatted.is_empty() {
        let _ = fs::remove_file(dir.join(PORT_FORWARDS_FILE));
        Ok(())
    } else {
        fs::write(dir.join(PORT_FORWARDS_FILE), format!("{formatted}\n"))
    }
}

pub fn configure_gpu(distro_dir: &Path, enabled: bool) -> io::Result<()> {
    if !distro_dir.join("rootfs.img").is_file() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "image not found"));
    }
    fs::write(distro_dir.join(GPU_FILE), if enabled { "1\n" } else { "0\n" })
}

pub fn rename_image(name: &str, new_name: &str, state_dir: &Path) -> io::Result<()> {
    let distros = state_dir.join("distros");
    let old_dir = distros.join(name);
    let new_dir = distros.join(new_name);
    let img_path = old_dir.join("rootfs.img");
    if !img_path.is_file() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("no image named {name}")));
    }
    if fs::symlink_metadata(&new_dir).is_ok() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("an image named {new_name} already exists")));
    }
    if loopdev::image_attached(&img_path)? {
        return Err(still_mounted());
    }
    fs::rename(&old_dir, &new_dir)?;
    let _ = fs::remove_dir(state_dir.join("mnt").join(name));
    Ok(())
}

fn still_mounted() -> io::Error {
    io::Error::new(
        io::ErrorKind::ResourceBusy,
        "the image is still mounted by processes from an earlier start; reboot to release it",
    )
}

pub fn create_distro_image(
    name: &str,
    size_gb: u32,
    tarball_path: &Path,
    username: &str,
    os_label: &str,
    state_dir: &Path,
    kernel: &KernelInfo,
) -> io::Result<CreatedImage> {
    if !(1..=256).contains(&size_gb) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "size_gb must be 1..=256",
        ));
    }
    if !tarball_path.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("tarball not found: {}", tarball_path.display()),
        ));
    }

    let distro_dir = state_dir.join("distros").join(name);
    let img_path = distro_dir.join("rootfs.img");
    if img_path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("distro image already exists: {}", img_path.display()),
        ));
    }
    fs::create_dir_all(&distro_dir)?;

    let res = (|| -> io::Result<bool> {
        let f = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&img_path)?;
        let size_bytes = u64::from(size_gb) * 1024 * 1024 * 1024;
        f.set_len(size_bytes)?;
        drop(f);

        format_ext4(&img_path)?;

        let mnt = state_dir.join("mnt").join(format!(".init-{name}"));
        let needs_pacman_keyring = unpack_in_private_ns(&img_path, tarball_path, username, &mnt)?;
        fs::write(distro_dir.join(OS_LABEL_FILE), format!("{os_label}\n"))?;
        fs::write(distro_dir.join(USERNAME_FILE), format!("{username}\n"))?;
        let init = if kernel.has_pid_ns { INIT_SYSTEM } else { INIT_NONE };
        fs::write(distro_dir.join(INIT_FILE), format!("{init}\n"))?;
        if kernel.isolated_network {
            fs::write(distro_dir.join(ISOLATED_NET_FILE), "1\n")?;
        }
        Ok(needs_pacman_keyring)
    })();

    match res {
        Ok(needs_pacman_keyring) => Ok(CreatedImage {
            image_path: img_path,
            needs_pacman_keyring,
        }),
        Err(e) => {
            remove_distro_files(&distro_dir);
            Err(e)
        }
    }
}

pub fn delete_distro_image(name: &str, state_dir: &Path) -> io::Result<()> {
    let distro_dir = state_dir.join("distros").join(name);
    let img_path = distro_dir.join("rootfs.img");
    if !img_path.is_file() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("no image named {name}")));
    }
    if loopdev::image_attached(&img_path)? {
        return Err(still_mounted());
    }
    fs::remove_file(&img_path)?;
    for file in META_FILES {
        let _ = fs::remove_file(distro_dir.join(file));
    }
    let _ = fs::remove_dir(state_dir.join("mnt").join(name));
    fs::remove_dir(&distro_dir)
}

pub fn remove_distro_files(distro_dir: &Path) {
    let _ = fs::remove_file(distro_dir.join("rootfs.img"));
    for file in META_FILES {
        let _ = fs::remove_file(distro_dir.join(file));
    }
    let _ = fs::remove_dir(distro_dir);
}

pub fn resize_distro_image(name: &str, size_gb: u32, state_dir: &Path) -> io::Result<()> {
    if !(1..=256).contains(&size_gb) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "size_gb must be 1..=256",
        ));
    }
    let img_path = state_dir.join("distros").join(name).join("rootfs.img");
    if loopdev::image_attached(&img_path)? {
        return Err(still_mounted());
    }
    let meta = fs::metadata(&img_path)?;
    let new_size = u64::from(size_gb) * 1024 * 1024 * 1024;
    if new_size <= meta.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "new size must be larger than current image size",
        ));
    }

    let f = OpenOptions::new().write(true).open(&img_path)?;
    f.set_len(new_size)?;
    drop(f);

    let img_str = img_path.to_string_lossy().into_owned();
    let status = Command::new(E2FSCK)
        .args(["-f", "-y", &img_str])
        .status()
        .map_err(|e| io::Error::new(e.kind(), format!("{E2FSCK}: {e}")))?;
    let code = status.code().unwrap_or(16);
    if code > 2 {
        return Err(io::Error::other(format!("e2fsck exited with status {code}")));
    }

    run_checked(RESIZE2FS, &[&img_str])?;
    Ok(())
}

struct ExportTarget {
    dir: std::os::fd::OwnedFd,
    name: std::ffi::CString,
    file: fs::File,
}

impl ExportTarget {
    fn remove(&self) {
        use std::os::fd::AsRawFd;
        unsafe { libc::unlinkat(self.dir.as_raw_fd(), self.name.as_ptr(), 0) };
    }
}

fn open_export_target(dest_path: &str) -> io::Result<ExportTarget> {
    let invalid = |msg: &str| io::Error::new(io::ErrorKind::InvalidInput, msg.to_string());
    let rest = dest_path
        .strip_prefix("/data/media/")
        .ok_or_else(|| invalid("backups must be saved in shared storage under /data/media"))?;
    let parts: Vec<&str> = rest.split('/').collect();
    let (file_name, dirs) = parts.split_last().ok_or_else(|| invalid("missing file name"))?;
    if dirs.is_empty()
        || !dirs[0].bytes().all(|b| b.is_ascii_digit())
        || parts.iter().any(|p| p.is_empty() || *p == "." || *p == ".." || p.contains('\0'))
        || !file_name.ends_with(".tar.gz")
    {
        return Err(invalid("invalid backup path"));
    }
    match create_below("/storage/emulated", dirs, file_name, false) {
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(e),
        Err(_) => create_below("/data/media", dirs, file_name, true),
        ok => ok,
    }
}

fn create_below(root: &str, dirs: &[&str], file_name: &str, set_owner: bool) -> io::Result<ExportTarget> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    let invalid = || io::Error::new(io::ErrorKind::InvalidInput, "invalid backup path");
    let open_flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    let root = CString::new(root).map_err(|_| invalid())?;
    let fd = unsafe { libc::open(root.as_ptr(), open_flags) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut dir = unsafe { OwnedFd::from_raw_fd(fd) };
    for component in dirs {
        let c = CString::new(*component).map_err(|_| invalid())?;
        let fd = unsafe { libc::openat(dir.as_raw_fd(), c.as_ptr(), open_flags) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        dir = unsafe { OwnedFd::from_raw_fd(fd) };
    }
    let name = CString::new(file_name).map_err(|_| invalid())?;
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o660,
        )
    };
    if fd < 0 {
        let e = io::Error::last_os_error();
        return Err(if e.kind() == io::ErrorKind::AlreadyExists {
            io::Error::new(io::ErrorKind::AlreadyExists, format!("{file_name} already exists"))
        } else {
            e
        });
    }
    if set_owner {
        unsafe {
            let _ = libc::fchown(fd, ANDROID_MEDIA_GID, ANDROID_MEDIA_GID);
            let _ = libc::fchmod(fd, 0o660);
        }
    }
    Ok(ExportTarget {
        dir,
        name,
        file: unsafe { fs::File::from_raw_fd(fd) },
    })
}

pub fn export_image(name: &str, state_dir: &Path, dest_path: &str) -> io::Result<()> {
    let img_path = state_dir.join("distros").join(name).join("rootfs.img");
    if !img_path.is_file() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("no image named {name}")));
    }
    if loopdev::image_attached(&img_path)? {
        return Err(still_mounted());
    }
    let busybox = busybox()?;
    let target = open_export_target(dest_path)?;
    let mnt = state_dir.join("mnt").join(format!(".export-{name}"));
    let result = run_in_private_ns(|| {
        let attached = loopdev::attach_image(&img_path)?;
        mount::mount_ext4_readonly(&attached.device_path, &mnt)?;
        let status = Command::new(busybox)
            .args(["tar", "--numeric-owner", "-czf", "-", "-C", &mnt.to_string_lossy(), "."])
            .stdin(Stdio::null())
            .stdout(Stdio::from(target.file.try_clone()?))
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!("tar failed with status {:?}", status.code())))
        }
    });
    let _ = fs::remove_dir(&mnt);
    let result = result.and_then(|()| target.file.sync_all());
    if result.is_err() {
        target.remove();
    }
    result
}

fn run_in_private_ns(work: impl FnOnce() -> io::Result<()>) -> io::Result<()> {
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(io::Error::last_os_error());
    }
    if pid == 0 {
        let rc = match (|| {
            if unsafe { libc::unshare(libc::CLONE_NEWNS) } != 0 {
                return Err(io::Error::last_os_error());
            }
            mount::make_root_private()?;
            work()
        })() {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("{e}");
                1
            }
        };
        unsafe { libc::_exit(rc) };
    }
    let mut status = 0;
    if unsafe { libc::waitpid(pid, &mut status, 0) } < 0 {
        return Err(io::Error::last_os_error());
    }
    if libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0 {
        Ok(())
    } else {
        Err(io::Error::other("the backup could not be written; see barebootd.log"))
    }
}

fn unpack_in_private_ns(img_path: &Path, tarball_path: &Path, username: &str, mnt: &Path) -> io::Result<bool> {
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(io::Error::last_os_error());
    }
    if pid == 0 {
        let rc = match do_unpack(img_path, tarball_path, username, mnt) {
            Ok(false) => 0,
            Ok(true) => EXIT_NEEDS_PACMAN_KEYRING,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => EXIT_USERNAME_TAKEN,
            Err(e) => {
                eprintln!("unpack error: {e}");
                1
            }
        };
        unsafe { libc::_exit(rc) };
    }

    let mut status = 0;
    let w = unsafe { libc::waitpid(pid, &mut status, 0) };
    let _ = fs::remove_dir(mnt);
    if w < 0 {
        return Err(io::Error::last_os_error());
    }
    let code = if libc::WIFEXITED(status) { libc::WEXITSTATUS(status) } else { -1 };
    if code == 0 || code == EXIT_NEEDS_PACMAN_KEYRING {
        Ok(code == EXIT_NEEDS_PACMAN_KEYRING)
    } else if code == EXIT_USERNAME_TAKEN {
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("the image already has a user named {username}"),
        ))
    } else {
        Err(io::Error::other("failed to unpack rootfs tarball into ext4 image"))
    }
}

fn do_unpack(img_path: &Path, tarball_path: &Path, username: &str, mnt: &Path) -> io::Result<bool> {
    if unsafe { libc::unshare(libc::CLONE_NEWNS) } != 0 {
        return Err(io::Error::last_os_error());
    }
    mount::make_root_private()?;
    let attached = loopdev::attach_image(img_path)?;
    mount::mount_ext4_simple(&attached.device_path, mnt)?;

    let busybox = busybox()?;
    let tar_str = tarball_path.to_string_lossy();
    let mnt_str = mnt.to_string_lossy();
    if tar_str.ends_with(".xz") {
        let mut decomp = Command::new(busybox)
            .args(["xz", "-dc", &tar_str])
            .stdout(Stdio::piped())
            .spawn()?;
        let stdout = decomp.stdout.take().ok_or_else(|| io::Error::other("pipe error"))?;
        let tar_status = Command::new(busybox)
            .args(["tar", "--numeric-owner", "-xf", "-", "-C", &mnt_str])
            .stdin(Stdio::from(stdout))
            .status()?;
        let decomp_status = decomp.wait()?;
        if !tar_status.success() || !decomp_status.success() {
            return Err(io::Error::other("xz/tar extraction failed"));
        }
    } else {
        let compression = if tar_str.ends_with(".tar") { "-xf" } else { "-xzf" };
        run_checked(busybox, &["tar", "--numeric-owner", compression, &tar_str, "-C", &mnt_str])?;
    }

    enter_root(mnt)?;
    let _ = fs::remove_file("/etc/resolv.conf");
    let _ = fs::write("/etc/resolv.conf", "nameserver 1.1.1.1\nnameserver 8.8.8.8\n");
    let _ = fs::create_dir_all("/mnt/android");
    if let Ok(conf) = fs::read_to_string("/etc/pacman.conf")
        && let Some(updated) = enable_pacman_option(&conf, "DisableSandboxFilesystem")
    {
        fs::write("/etc/pacman.conf", updated)?;
    }
    disable_runit_services(Path::new(RUNIT_SERVICE_DIR));
    provision_user(username)?;
    Ok(Path::new("/usr/bin/pacman-key").is_file() && !Path::new("/etc/pacman.d/gnupg").exists())
}

const RUNIT_SERVICE_DIR: &str = "/etc/runit/runsvdir/default";
const RUNIT_UNUSABLE_SERVICES: &[&str] = &["agetty-console", "dhclient-eth0", "nanoklogd"];

fn disable_runit_services(dir: &Path) {
    for service in RUNIT_UNUSABLE_SERVICES {
        let link = dir.join(service);
        if fs::symlink_metadata(&link).is_ok_and(|m| m.file_type().is_symlink()) {
            let _ = fs::remove_file(&link);
        }
    }
}

fn enter_root(root: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c = CString::new(root.as_os_str().as_bytes()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    if unsafe { libc::chroot(c.as_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    std::env::set_current_dir("/")
}

fn provision_user(username: &str) -> io::Result<()> {
    let passwd = fs::read_to_string("/etc/passwd").unwrap_or_default();
    let entries: Vec<Vec<&str>> = passwd.lines().map(|l| l.split(':').collect()).collect();
    let uid_owner = entries
        .iter()
        .find(|f| f.get(2) == Some(&"1000"))
        .and_then(|f| f.first().copied());
    let taken_by_other = entries
        .iter()
        .any(|f| f.first() == Some(&username) && f.get(2) != Some(&"1000"));
    if taken_by_other {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "username taken"));
    }
    match uid_owner {
        Some(existing) => adopt_user(existing, username)?,
        None => add_user(username, &passwd)?,
    }
    let name = username;
    add_android_group(name)?;
    if fs::create_dir_all("/etc/sudoers.d").is_ok() {
        let rule = Path::new("/etc/sudoers.d/bareboot");
        if fs::write(rule, format!("{name} ALL=(ALL) NOPASSWD:ALL\n")).is_ok() {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(rule, fs::Permissions::from_mode(0o440));
        }
    }
    Ok(())
}

fn adopt_user(old: &str, new: &str) -> io::Result<()> {
    let old_home = format!("/home/{old}");
    let new_home = format!("/home/{new}");
    let move_home = old != new && Path::new(&old_home).is_dir() && !Path::new(&new_home).exists();
    rewrite_db("/etc/passwd", |f| {
        if f[0] == old {
            f[0] = new.to_string();
            if move_home && f.len() > 5 && f[5] == old_home {
                f[5] = new_home.clone();
            }
        }
    })?;
    rewrite_db("/etc/shadow", |f| {
        if f[0] == old {
            f[0] = new.to_string();
            if f.len() > 1 {
                f[1] = "*".to_string();
            }
        }
    })?;
    for db in ["/etc/group", "/etc/gshadow"] {
        rewrite_db(db, |f| {
            if f[0] == old {
                f[0] = new.to_string();
            }
            if let Some(members) = f.get_mut(3) {
                *members = members
                    .split(',')
                    .map(|m| if m == old { new } else { m })
                    .collect::<Vec<_>>()
                    .join(",");
            }
        })?;
    }
    if move_home {
        fs::rename(&old_home, &new_home)?;
    }
    Ok(())
}

fn rewrite_db(path: &str, edit: impl Fn(&mut Vec<String>)) -> io::Result<()> {
    let Ok(text) = fs::read_to_string(path) else {
        return Ok(());
    };
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let mut fields: Vec<String> = line.split(':').map(str::to_string).collect();
        if fields.len() > 1 {
            edit(&mut fields);
        }
        out.push_str(&fields.join(":"));
        out.push('\n');
    }
    fs::write(path, out)
}

fn add_android_group(username: &str) -> io::Result<()> {
    let group = fs::read_to_string("/etc/group").unwrap_or_default();
    let gid = ANDROID_MEDIA_GID.to_string();
    if group.lines().any(|l| l.split(':').nth(2) == Some(gid.as_str())) {
        return rewrite_db("/etc/group", |f| {
            if f.get(2).map(String::as_str) == Some(gid.as_str())
                && let Some(members) = f.get_mut(3)
                && !members.split(',').any(|m| m == username)
            {
                if !members.is_empty() {
                    members.push(',');
                }
                members.push_str(username);
            }
        });
    }
    if group.lines().any(|l| l.split(':').next() == Some("android")) {
        return Ok(());
    }
    append_line("/etc/group", &group, &format!("android:x:{gid}:{username}"))
}

fn add_user(username: &str, passwd: &str) -> io::Result<()> {
    let shell = if Path::new("/bin/bash").exists() { "/bin/bash" } else { "/bin/sh" };
    let home = format!("/home/{username}");
    append_line("/etc/passwd", passwd, &format!("{username}:x:{GUEST_UID}:{GUEST_UID}::{home}:{shell}"))?;

    let group = fs::read_to_string("/etc/group").unwrap_or_default();
    let groups: Vec<Vec<&str>> = group.lines().map(|l| l.split(':').collect()).collect();
    let gid_taken = groups.iter().any(|f| f.get(2) == Some(&"1000"));
    let name_taken = groups.iter().any(|f| f.first() == Some(&username));
    if !gid_taken && !name_taken {
        append_line("/etc/group", &group, &format!("{username}:x:{GUEST_UID}:"))?;
    }

    if let Ok(shadow) = fs::read_to_string("/etc/shadow")
        && !shadow.lines().any(|l| l.split(':').next() == Some(username))
    {
        append_line("/etc/shadow", &shadow, &format!("{username}:*:19000:0:99999:7:::"))?;
    }

    fs::create_dir_all(&home)?;
    if let Ok(entries) = fs::read_dir("/etc/skel") {
        for entry in entries.flatten() {
            let src = entry.path();
            if src.is_file() {
                let dst = Path::new(&home).join(entry.file_name());
                if fs::copy(&src, &dst).is_ok() {
                    chown_path(&dst, GUEST_UID, GUEST_UID);
                }
            }
        }
    }
    chown_path(Path::new(&home), GUEST_UID, GUEST_UID);
    Ok(())
}

fn append_line(path: &str, current: &str, line: &str) -> io::Result<()> {
    let mut next = current.to_string();
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(line);
    next.push('\n');
    fs::write(path, next)
}

fn chown_path(path: &Path, uid: u32, gid: u32) {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    if let Ok(c) = CString::new(path.as_os_str().as_bytes()) {
        unsafe {
            let _ = libc::chown(c.as_ptr(), uid, gid);
        }
    }
}

fn format_ext4(path: &Path) -> io::Result<()> {
    let output = Command::new(MKE2FS)
        .args(["-F", "-q", "-t", "ext4", "-O", "fast_commit"])
        .env("LC_ALL", "C")
        .arg(path)
        .output()?;
    if output.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&output.stderr);
    if output.status.code() == Some(1)
        && err.lines().any(|line| line == "Invalid filesystem option set: fast_commit")
    {
        return run_checked(MKE2FS, &["-F", "-q", "-t", "ext4", &path.to_string_lossy()]);
    }
    Err(io::Error::other(format!("{MKE2FS}: {}", err.trim())))
}

fn run_checked(bin: &str, args: &[&str]) -> io::Result<()> {
    let status = Command::new(bin)
        .args(args)
        .status()
        .map_err(|e| io::Error::new(e.kind(), format!("{bin}: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{bin} failed with status {:?}",
            status.code()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::{disable_runit_services, superblock_usage};
    use std::fs;
    use std::os::unix::fs::symlink;

    #[test]
    fn disable_runit_services_removes_only_the_unusable_links() {
        let dir = std::env::temp_dir().join(format!("bareboot-runit-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for name in ["agetty-console", "dhclient-eth0", "nanoklogd", "udevd"] {
            symlink(format!("/etc/sv/{name}"), dir.join(name)).unwrap();
        }
        disable_runit_services(&dir);
        assert!(fs::symlink_metadata(dir.join("agetty-console")).is_err());
        assert!(fs::symlink_metadata(dir.join("dhclient-eth0")).is_err());
        assert!(fs::symlink_metadata(dir.join("nanoklogd")).is_err());
        assert!(fs::symlink_metadata(dir.join("udevd")).is_ok());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn superblock_usage_counts_available_blocks() {
        let mut sb = [0u8; 0x160];
        sb[0x04..0x08].copy_from_slice(&1000u32.to_le_bytes());
        sb[0x08..0x0C].copy_from_slice(&50u32.to_le_bytes());
        sb[0x0C..0x10].copy_from_slice(&400u32.to_le_bytes());
        sb[0x18..0x1C].copy_from_slice(&2u32.to_le_bytes());
        assert_eq!(superblock_usage(&sb), (4_096_000, 1_433_600));
        sb[0x60] = 0x80;
        sb[0x150..0x154].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(superblock_usage(&sb).0, (1000 + (1u64 << 32)) * 4096);
    }
}
