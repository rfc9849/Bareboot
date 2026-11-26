use crate::validate::format_resolv_conf;
use protocol::DEFAULT_SHARED_DIR;
use std::ffi::CString;
use std::fs::{self, OpenOptions, Permissions};
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt, symlink};
use std::path::Path;

fn cstr_path(p: &Path) -> io::Result<CString> {
    CString::new(p.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))
}

fn cstr(s: &str) -> io::Result<CString> {
    CString::new(s).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "string contains NUL"))
}

fn sys_mount(
    source: Option<&Path>,
    target: &Path,
    fstype: Option<&str>,
    flags: libc::c_ulong,
    data: Option<&str>,
) -> io::Result<()> {
    let c_src = match source {
        Some(p) => Some(cstr_path(p)?),
        None => None,
    };
    let c_tgt = cstr_path(target)?;
    let c_fs = match fstype {
        Some(f) => Some(cstr(f)?),
        None => None,
    };
    let c_data = match data {
        Some(d) => Some(cstr(d)?),
        None => None,
    };
    let rc = unsafe {
        libc::mount(
            c_src.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
            c_tgt.as_ptr(),
            c_fs.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
            flags,
            c_data
                .as_ref()
                .map_or(std::ptr::null(), |s| s.as_ptr() as *const libc::c_void),
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn mknod_chr(path: &Path, mode: libc::mode_t, major: u32, minor: u32) -> io::Result<()> {
    let c_path = cstr_path(path)?;
    let dev = libc::makedev(major, minor);
    let rc = unsafe { libc::mknod(c_path.as_ptr(), libc::S_IFCHR | mode, dev) };
    if rc != 0 {
        let err = io::Error::last_os_error();
        if err.kind() != io::ErrorKind::AlreadyExists {
            return Err(err);
        }
    }
    Ok(())
}

pub fn make_root_private() -> io::Result<()> {
    sys_mount(
        None,
        Path::new("/"),
        None,
        libc::MS_REC | libc::MS_PRIVATE,
        None,
    )
}

pub const GUEST_FILE_CONTEXT: &str = "u:object_r:bareboot_guest_file:s0";

pub fn mount_ext4_simple(loop_dev: &Path, mountpoint: &Path) -> io::Result<()> {
    fs::create_dir_all(mountpoint)?;
    sys_mount(
        Some(loop_dev),
        mountpoint,
        Some("ext4"),
        libc::MS_RELATIME,
        None,
    )
}

pub fn mount_ext4_readonly(loop_dev: &Path, mountpoint: &Path) -> io::Result<()> {
    fs::create_dir_all(mountpoint)?;
    sys_mount(
        Some(loop_dev),
        mountpoint,
        Some("ext4"),
        libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC,
        None,
    )
}

fn mount_ext4_guest(loop_dev: &Path, mountpoint: &Path, relaxed_sync: bool) -> io::Result<()> {
    fs::create_dir_all(mountpoint)?;
    let mut options = format!("context={GUEST_FILE_CONTEXT}");
    if relaxed_sync {
        options.push_str(",barrier=0");
    }
    sys_mount(
        Some(loop_dev),
        mountpoint,
        Some("ext4"),
        libc::MS_RELATIME,
        Some(options.as_str()),
    )
}

pub struct GpuMounts<'a> {
    pub socket: &'a Path,
    pub guest: &'a Path,
}

fn mount_desktop(run: &Path, source: &Path) -> io::Result<()> {
    if !fs::symlink_metadata(run)?.is_dir() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid guest run directory"));
    }
    let target = run.join("bareboot-display");
    fs::create_dir(&target)?;
    sys_mount(Some(source), &target, None, libc::MS_BIND, None)?;
    let mounted = sys_mount(
        None, &target, None,
        libc::MS_BIND | libc::MS_REMOUNT | libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC,
        None,
    );
    if mounted.is_err() {
        let path = cstr_path(&target)?;
        unsafe { libc::umount2(path.as_ptr(), libc::MNT_DETACH) };
    }
    mounted
}

fn mount_guest_gpu(run_dir: &Path, mounts: &GpuMounts<'_>) -> io::Result<()> {
    let gpu_dir = run_dir.join("bareboot-gpu");
    let sock_dir = gpu_dir.join("socket");
    let guest_dir = gpu_dir.join("guest");
    fs::create_dir(&gpu_dir)?;
    fs::create_dir(&sock_dir)?;
    fs::create_dir(&guest_dir)?;
    for (source, target, flags) in [
        (mounts.socket, &sock_dir, libc::MS_NOEXEC),
        (mounts.guest, &guest_dir, 0),
    ] {
        if let Err(err) = sys_mount(Some(source), target, None, libc::MS_BIND, None).and_then(|()| {
            sys_mount(
                None,
                target,
                None,
                libc::MS_BIND | libc::MS_REMOUNT | libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NODEV | flags,
                None,
            )
        }) {
            for path in [&guest_dir, &sock_dir] {
                let path = cstr_path(path)?;
                unsafe { libc::umount2(path.as_ptr(), libc::MNT_DETACH) };
            }
            return Err(err);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn mount_ext4_and_pivot(
    loop_dev: &Path,
    mountpoint: &Path,
    hostname: &str,
    dns_servers: &[String],
    shared_dir: Option<&Path>,
    mount_proc: bool,
    relaxed_sync: bool,
    gpu_mounts: Option<&GpuMounts<'_>>,
    desktop_dir: Option<&Path>,
) -> io::Result<(bool, bool)> {
    mount_ext4_guest(loop_dev, mountpoint, relaxed_sync)?;

    write_resolv_conf(mountpoint, dns_servers)?;
    ensure_etc_hosts(mountpoint, hostname);

    sys_mount(
        Some(mountpoint),
        mountpoint,
        None,
        libc::MS_BIND | libc::MS_REC,
        None,
    )?;

    let proc_dir = mountpoint.join("proc");
    let sys_dir = mountpoint.join("sys");
    let dev_dir = mountpoint.join("dev");
    let tmp_dir = mountpoint.join("tmp");
    let run_dir = mountpoint.join("run");
    let old_root = mountpoint.join(".oldroot");

    for d in [&proc_dir, &sys_dir, &dev_dir, &tmp_dir, &run_dir, &old_root] {
        fs::create_dir_all(d)?;
    }

    if let Some(host_shared) = shared_dir {
        if host_shared == Path::new(DEFAULT_SHARED_DIR) {
            let previous_umask = unsafe { libc::umask(0o007) };
            let _ = fs::DirBuilder::new().mode(0o770).create(host_shared);
            unsafe { libc::umask(previous_umask) };
        } else {
            let _ = fs::create_dir_all(host_shared);
        }
        if host_shared.is_dir() {
            let guest_shared = mountpoint.join("mnt").join("android");
            fs::create_dir_all(&guest_shared)?;
            if sys_mount(
                Some(host_shared),
                &guest_shared,
                None,
                libc::MS_BIND | libc::MS_REC,
                None,
            )
            .is_ok()
            {
                let _ = sys_mount(
                    Some(host_shared),
                    &guest_shared,
                    None,
                    libc::MS_BIND | libc::MS_REMOUNT | libc::MS_NOSUID | libc::MS_NODEV,
                    None,
                );
            }
        }
    }

    if mount_proc {
        sys_mount(
            Some(Path::new("proc")),
            &proc_dir,
            Some("proc"),
            libc::MS_NOSUID | libc::MS_NOEXEC | libc::MS_NODEV,
            None,
        )?;
    }
    sys_mount(
        Some(Path::new("sysfs")),
        &sys_dir,
        Some("sysfs"),
        libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NOEXEC | libc::MS_NODEV,
        None,
    )?;

    setup_dev(&dev_dir)?;

    sys_mount(
        Some(Path::new("tmpfs")),
        &tmp_dir,
        Some("tmpfs"),
        libc::MS_NOSUID | libc::MS_NODEV,
        Some("mode=1777"),
    )?;
    sys_mount(
        Some(Path::new("tmpfs")),
        &run_dir,
        Some("tmpfs"),
        libc::MS_NOSUID | libc::MS_NODEV,
        Some("mode=0755"),
    )?;

    let gpu_mounted = gpu_mounts.is_some_and(|mounts| {
        match mount_guest_gpu(&run_dir, mounts) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("could not mount GPU bundle for {hostname}: {e}");
                false
            }
        }
    });
    let desktop_mounted = desktop_dir.is_some_and(|path| match mount_desktop(&run_dir, path) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("could not mount display socket for {hostname}: {e}");
            false
        }
    });
    std::env::set_current_dir(mountpoint)?;
    let dot = CString::new(".").unwrap();
    let put_old = CString::new(".oldroot").unwrap();
    let rc = unsafe { libc::syscall(libc::SYS_pivot_root, dot.as_ptr(), put_old.as_ptr()) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    std::env::set_current_dir("/")?;

    let c_old = CString::new("/.oldroot").unwrap();
    let rc = unsafe { libc::umount2(c_old.as_ptr(), libc::MNT_DETACH) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    let _ = fs::remove_dir("/.oldroot");

    let host_bytes = hostname.as_bytes();
    unsafe {
        let _ = libc::sethostname(host_bytes.as_ptr() as *const libc::c_char, host_bytes.len());
    }

    apply_sys_masks();
    if mount_proc {
        apply_proc_masks();
    }
    Ok((gpu_mounted, desktop_mounted))
}

pub fn mount_guest_proc(isolated_net: bool) -> io::Result<()> {
    sys_mount(
        Some(Path::new("proc")),
        Path::new("/proc"),
        Some("proc"),
        libc::MS_NOSUID | libc::MS_NOEXEC | libc::MS_NODEV,
        None,
    )?;
    if !isolated_net {
        apply_proc_masks();
        return Ok(());
    }
    use std::os::fd::AsRawFd;
    let net = fs::File::open("/proc/sys/net")?;
    for name in ["/proc/sysrq-trigger", "/proc/kcore", "/proc/timer_list", "/proc/irq"] {
        mask_readonly(Path::new(name));
    }
    let sys = Path::new("/proc/sys");
    sys_mount(Some(sys), sys, None, libc::MS_BIND, None)?;
    sys_mount(None, sys, None, libc::MS_BIND | libc::MS_REMOUNT | libc::MS_RDONLY, None)?;
    let source = format!("/proc/self/fd/{}", net.as_raw_fd());
    sys_mount(Some(Path::new(&source)), Path::new("/proc/sys/net"), None, libc::MS_BIND, None)?;
    Ok(())
}

const CGROUP_ROOT: &str = "/sys/fs/cgroup";
const CGROUP_PARENT: &str = "bareboot";

pub fn delegate_cgroup(name: &str) -> io::Result<()> {
    let root = Path::new(CGROUP_ROOT);
    let cgroup_flags = libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC;
    sys_mount(Some(Path::new("cgroup2")), root, Some("cgroup2"), cgroup_flags, None)
        .map_err(|e| io::Error::new(e.kind(), format!("cgroup2 mount failed: {e}")))?;
    let parent = root.join(CGROUP_PARENT);
    let guest = parent.join(name);
    let moved = enter_guest_cgroup(&parent, &guest);
    let c_root = cstr_path(root)?;
    unsafe { libc::umount2(c_root.as_ptr(), libc::MNT_DETACH) };
    moved.map_err(|e| io::Error::new(e.kind(), format!("could not create the guest cgroup: {e}")))?;
    if unsafe { libc::unshare(libc::CLONE_NEWCGROUP) } != 0 {
        let e = io::Error::last_os_error();
        return Err(io::Error::new(e.kind(), format!("cgroup namespace: {e}")));
    }
    sys_mount(Some(Path::new("cgroup2")), root, Some("cgroup2"), cgroup_flags, None)
}

fn enter_guest_cgroup(parent: &Path, guest: &Path) -> io::Result<()> {
    let _ = fs::create_dir(parent);
    let controllers = fs::read_to_string(parent.join("cgroup.controllers")).unwrap_or_default();
    for controller in controllers.split_whitespace() {
        let _ = fs::write(parent.join("cgroup.subtree_control"), format!("+{controller}"));
    }
    let _ = fs::create_dir(guest);
    fs::write(guest.join("cgroup.procs"), "0")
}

pub fn remove_guest_cgroup(name: &str) {
    fn remove_tree(dir: &Path) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                if entry.file_type().is_ok_and(|t| t.is_dir()) {
                    remove_tree(&entry.path());
                }
            }
        }
        let _ = fs::remove_dir(dir);
    }
    if crate::validate::validate_distro_name(name).is_ok() {
        remove_tree(&Path::new(CGROUP_ROOT).join(CGROUP_PARENT).join(name));
    }
}

pub fn apply_timezone(timezone: &str) {
    if !crate::validate::valid_timezone(timezone) {
        return;
    }
    if !Path::new("/usr/share/zoneinfo").join(timezone).is_file() {
        return;
    }
    let target = format!("../usr/share/zoneinfo/{timezone}");
    if fs::read_link("/etc/localtime").is_ok_and(|current| current == Path::new(&target)) {
        return;
    }
    let temporary = Path::new("/etc/.bareboot-localtime");
    let _ = fs::remove_file(temporary);
    if symlink(&target, temporary).is_ok() && fs::rename(temporary, "/etc/localtime").is_err() {
        let _ = fs::remove_file(temporary);
    }
    if fs::symlink_metadata("/etc/timezone").is_ok_and(|m| m.is_file()) {
        let _ = fs::write("/etc/timezone", format!("{timezone}\n"));
    }
}

pub fn write_resolv_conf(rootfs: &Path, dns_servers: &[String]) -> io::Result<()> {
    let etc = rootfs.join("etc");
    fs::create_dir_all(&etc)?;
    let resolv = etc.join("resolv.conf");
    let temporary = etc.join(format!(".bareboot-resolv-{}", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(format_resolv_conf(dns_servers).as_bytes())?;
        file.set_permissions(Permissions::from_mode(0o644))?;
        fs::rename(&temporary, resolv)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn ensure_etc_hosts(rootfs: &Path, hostname: &str) {
    let hosts_path = rootfs.join("etc").join("hosts");
    let current = fs::read_to_string(&hosts_path).unwrap_or_default();
    if current.lines().any(|l| l.split_whitespace().any(|w| w == hostname)) {
        return;
    }
    let mut updated = current;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    if !updated.contains("127.0.0.1") {
        updated.push_str("127.0.0.1 localhost\n");
    }
    updated.push_str(&format!("127.0.0.1 {hostname}\n"));
    let _ = fs::write(&hosts_path, updated);
}

fn setup_dev(dev_dir: &Path) -> io::Result<()> {
    sys_mount(
        Some(Path::new("tmpfs")),
        dev_dir,
        Some("tmpfs"),
        libc::MS_NOSUID | libc::MS_STRICTATIME,
        Some("mode=0755,size=64k"),
    )?;

    let old_mask = unsafe { libc::umask(0) };
    let node_res = (|| -> io::Result<()> {
        mknod_chr(&dev_dir.join("null"), 0o666, 1, 3)?;
        mknod_chr(&dev_dir.join("zero"), 0o666, 1, 5)?;
        mknod_chr(&dev_dir.join("full"), 0o666, 1, 7)?;
        mknod_chr(&dev_dir.join("random"), 0o666, 1, 8)?;
        mknod_chr(&dev_dir.join("urandom"), 0o666, 1, 9)?;
        mknod_chr(&dev_dir.join("tty"), 0o666, 5, 0)?;
        Ok(())
    })();
    unsafe {
        libc::umask(old_mask);
    }
    node_res?;

    symlink("/proc/self/fd", dev_dir.join("fd"))?;
    symlink("/proc/self/fd/0", dev_dir.join("stdin"))?;
    symlink("/proc/self/fd/1", dev_dir.join("stdout"))?;
    symlink("/proc/self/fd/2", dev_dir.join("stderr"))?;

    let pts_dir = dev_dir.join("pts");
    let shm_dir = dev_dir.join("shm");
    fs::create_dir_all(&pts_dir)?;
    fs::create_dir_all(&shm_dir)?;

    sys_mount(
        Some(Path::new("devpts")),
        &pts_dir,
        Some("devpts"),
        libc::MS_NOSUID | libc::MS_NOEXEC,
        Some("newinstance,ptmxmode=0666,mode=0620,gid=5"),
    )?;
    symlink("pts/ptmx", dev_dir.join("ptmx"))?;

    sys_mount(
        Some(Path::new("tmpfs")),
        &shm_dir,
        Some("tmpfs"),
        libc::MS_NOSUID | libc::MS_NODEV,
        Some("mode=1777"),
    )?;
    Ok(())
}

fn mask_readonly(path: &Path) {
    if !path.exists() {
        return;
    }
    if sys_mount(Some(path), path, None, libc::MS_BIND, None).is_ok() {
        let _ = sys_mount(
            Some(path),
            path,
            None,
            libc::MS_BIND | libc::MS_REMOUNT | libc::MS_RDONLY,
            None,
        );
    }
}

fn apply_proc_masks() {
    for p in ["/proc/sysrq-trigger", "/proc/kcore", "/proc/timer_list", "/proc/irq", "/proc/sys"] {
        mask_readonly(Path::new(p));
    }
}

fn apply_sys_masks() {
    let paths = [
        "/sys/firmware",
        "/sys/kernel/security",
        "/sys/kernel/debug",
        "/sys/kernel/tracing",
        "/sys/block",
    ];
    for p in paths {
        mask_readonly(Path::new(p));
    }

    if let Ok(entries) = fs::read_dir("/sys/fs/cgroup") {
        for entry in entries.flatten() {
            let agent = entry.path().join("release_agent");
            mask_readonly(&agent);
        }
    }
}
