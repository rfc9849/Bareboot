use crate::validate::{validate_port_forwards, validate_ssh_port};
use protocol::PortForward;
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const HELPER: &str = "/data/adb/modules/bareboot/bin/slirp4netns";
const TUN: &str = "/dev/tun";
const IP: &str = "/system/bin/ip";
const TAP_NAME: &str = "bareboot0";
const SUBNET: &str = "10.231.0.0/24";
const GUEST_IP: &str = "10.231.0.100";
const ADDRESS: &str = "10.231.0.100/24";
const GATEWAY: &str = "10.231.0.2";
const API_SOCKET: &str = "api.sock";
const NOBODY: u32 = 65534;

fn valid_helper(m: &fs::Metadata) -> bool {
    m.is_file() && m.uid() == 0 && m.mode() & 0o022 == 0 && m.mode() & 0o100 != 0
}

pub fn available() -> bool {
    fs::symlink_metadata(HELPER).is_ok_and(|m| valid_helper(&m))
        && fs::symlink_metadata(TUN)
            .is_ok_and(|m| m.file_type().is_char_device() && m.rdev() == libc::makedev(10, 200))
        && fs::metadata(IP).is_ok_and(|m| m.is_file())
}

fn open_tap() -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(TUN)?;
    let mut request = unsafe { std::mem::zeroed::<libc::ifreq>() };
    for (dst, src) in request.ifr_name.iter_mut().zip(TAP_NAME.bytes()) {
        *dst = src as libc::c_char;
    }
    request.ifr_ifru.ifru_flags = (libc::IFF_TAP | libc::IFF_NO_PI) as libc::c_short;
    if unsafe { libc::ioctl(file.as_raw_fd(), libc::TUNSETIFF, &mut request) } != 0 {
        return Err(io::Error::last_os_error());
    }
    for argv in [
        &["link", "set", "lo", "up"][..],
        &["link", "set", TAP_NAME, "up"],
        &["addr", "add", ADDRESS, "dev", TAP_NAME],
        &["route", "add", "default", "via", GATEWAY, "dev", TAP_NAME],
    ] {
        let out = Command::new(IP).args(argv).stdin(Stdio::null()).output()?;
        if !out.status.success() {
            return Err(io::Error::other(format!(
                "could not configure isolated network: {}",
                String::from_utf8_lossy(&out.stderr).trim(),
            )));
        }
    }
    Ok(file)
}

struct ApiMountGuard {
    path: PathBuf,
    c_path: CString,
    mounted: bool,
}

impl ApiMountGuard {
    fn mount(mountpoint: &Path) -> io::Result<(Self, File)> {
        let mut os = mountpoint.as_os_str().to_os_string();
        os.push(".uplink");
        let path = PathBuf::from(os);
        let c_path = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))?;
        fs::create_dir_all(&path)?;
        let c_fs = CString::new("tmpfs").unwrap();
        let c_opts = CString::new("mode=0700,uid=65534,gid=65534,size=64k").unwrap();
        let rc = unsafe {
            libc::mount(
                c_fs.as_ptr(),
                c_path.as_ptr(),
                c_fs.as_ptr(),
                libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC,
                c_opts.as_ptr() as *const libc::c_void,
            )
        };
        if rc != 0 {
            let err = io::Error::last_os_error();
            let _ = fs::remove_dir(&path);
            return Err(err);
        }
        let guard = Self {
            path,
            c_path,
            mounted: true,
        };
        let dir = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(&guard.path)?;
        Ok((guard, dir))
    }
}

impl Drop for ApiMountGuard {
    fn drop(&mut self) {
        if self.mounted {
            unsafe {
                libc::umount2(self.c_path.as_ptr(), libc::MNT_DETACH);
            }
            self.mounted = false;
        }
        let _ = fs::remove_dir(&self.path);
    }
}

fn parse_hostfwd_id(response: &str) -> io::Result<u32> {
    if response.contains("\"error\"") {
        return Err(io::Error::other(format!("uplink API error: {}", response.trim())));
    }
    let rest = response
        .split_once("\"id\":")
        .map(|(_, r)| r.trim_start())
        .ok_or_else(|| io::Error::other(format!("unexpected uplink API reply: {}", response.trim())))?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits
        .parse::<u32>()
        .map_err(|_| io::Error::other(format!("invalid hostfwd id in reply: {}", response.trim())))
}

pub struct Uplink {
    child: Child,
    _exit: UnixStream,
    api_dir: File,
    forwards: Vec<(u32, u16, u16)>,
}

impl Uplink {
    pub fn start(
        host_namespace: &File,
        mountpoint: &Path,
        ssh_port: u16,
        port_forwards: &[PortForward],
    ) -> io::Result<Self> {
        validate_ssh_port(ssh_port)?;
        validate_port_forwards(port_forwards, ssh_port)?;
        let helper = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(HELPER)?;
        if !valid_helper(&helper.metadata()?) {
            return Err(io::Error::other(
                "uplink helper is not a root-owned executable",
            ));
        }
        let tap = open_tap()?;
        let (api_guard, api_dir) = ApiMountGuard::mount(mountpoint)?;
        let (mut ready, ready_child) = UnixStream::pair()?;
        let (exit, exit_child) = UnixStream::pair()?;
        ready.set_read_timeout(Some(Duration::from_secs(5)))?;
        let host_fd = host_namespace.as_raw_fd();
        let api_dir_fd = api_dir.as_raw_fd();
        let ready_fd = ready_child.as_raw_fd();
        let exit_fd = exit_child.as_raw_fd();
        let tap_fd = tap.as_raw_fd();
        let mut cmd = Command::new(format!("/proc/self/fd/{}", helper.as_raw_fd()));
        cmd.args([
            "--cidr",
            SUBNET,
            "--disable-host-loopback",
            "--disable-dns",
            "--enable-seccomp",
            "--api-socket",
            API_SOCKET,
            "--netns-type=tapfd",
            "--ready-fd",
            &ready_fd.to_string(),
            "--exit-fd",
            &exit_fd.to_string(),
            &tap_fd.to_string(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null());
        unsafe {
            cmd.pre_exec(move || {
                if libc::fchdir(api_dir_fd) != 0
                    || libc::setns(host_fd, libc::CLONE_NEWNET) != 0
                    || libc::setgroups(0, std::ptr::null()) != 0
                    || libc::setresgid(NOBODY, NOBODY, NOBODY) != 0
                    || libc::setresuid(NOBODY, NOBODY, NOBODY) != 0
                    || libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0
                {
                    return Err(io::Error::last_os_error());
                }
                for fd in [tap_fd, ready_fd, exit_fd] {
                    if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        let mut uplink = Self {
            child: cmd.spawn()?,
            _exit: exit,
            api_dir,
            forwards: Vec::new(),
        };
        drop(ready_child);
        drop(exit_child);
        let mut byte = [0];
        ready
            .read_exact(&mut byte)
            .map_err(|err| io::Error::other(format!("IPv4 uplink did not start: {err}")))?;
        drop(api_guard);
        if byte != *b"1" {
            return Err(io::Error::other("unexpected IPv4 uplink readiness signal"));
        }
        if ssh_port != 0 || !port_forwards.is_empty() {
            uplink.sync_forwards(ssh_port, port_forwards)?;
        }
        Ok(uplink)
    }

    fn api_call(&self, request: &str) -> io::Result<String> {
        let saved_cwd = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
            .open(".")?;
        if unsafe { libc::fchdir(self.api_dir.as_raw_fd()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let connected = UnixStream::connect(API_SOCKET);
        let _ = unsafe { libc::fchdir(saved_cwd.as_raw_fd()) };
        let mut stream = connected?;
        let timeout = Some(Duration::from_secs(3));
        stream.set_read_timeout(timeout)?;
        stream.set_write_timeout(timeout)?;
        stream.write_all(request.as_bytes())?;
        let mut response = String::new();
        stream.read_to_string(&mut response)?;
        Ok(response)
    }

    fn add_hostfwd(&self, host_port: u16, guest_port: u16) -> io::Result<u32> {
        let req = format!(
            "{{\"execute\":\"add_hostfwd\",\"arguments\":{{\"proto\":\"tcp\",\"host_addr\":\"0.0.0.0\",\"host_port\":{host_port},\"guest_addr\":\"{GUEST_IP}\",\"guest_port\":{guest_port}}}}}"
        );
        let resp = self.api_call(&req)?;
        parse_hostfwd_id(&resp).map_err(|e| {
            io::Error::other(format!(
                "could not forward TCP port {host_port}->{guest_port} (port already in use?): {e}"
            ))
        })
    }

    fn remove_hostfwd(&self, id: u32) -> io::Result<()> {
        let rm = format!("{{\"execute\":\"remove_hostfwd\",\"arguments\":{{\"id\":{id}}}}}");
        let resp = self.api_call(&rm)?;
        if resp.contains("\"error\"") {
            return Err(io::Error::other(format!(
                "could not remove port forward {id}: {}",
                resp.trim()
            )));
        }
        Ok(())
    }

    pub fn sync_forwards(&mut self, ssh_port: u16, port_forwards: &[PortForward]) -> io::Result<()> {
        validate_ssh_port(ssh_port)?;
        validate_port_forwards(port_forwards, ssh_port)?;
        let mut desired: Vec<(u16, u16)> = Vec::with_capacity(port_forwards.len() + 1);
        if ssh_port != 0 {
            desired.push((ssh_port, ssh_port));
        }
        for fwd in port_forwards {
            desired.push((fwd.host_port, fwd.guest_port));
        }

        let mut removed = Vec::new();
        let mut idx = 0;
        while idx < self.forwards.len() {
            let (id, hp, gp) = self.forwards[idx];
            if !desired.contains(&(hp, gp)) {
                self.remove_hostfwd(id)?;
                self.forwards.remove(idx);
                removed.push((hp, gp));
            } else {
                idx += 1;
            }
        }

        let mut added_ids = Vec::new();
        for &(hp, gp) in &desired {
            if self.forwards.iter().any(|&(_, chp, cgp)| chp == hp && cgp == gp) {
                continue;
            }
            match self.add_hostfwd(hp, gp) {
                Ok(id) => {
                    self.forwards.push((id, hp, gp));
                    added_ids.push(id);
                }
                Err(e) => {
                    for id in added_ids {
                        let _ = self.remove_hostfwd(id);
                        self.forwards.retain(|&(fid, _, _)| fid != id);
                    }
                    for (ohp, ogp) in removed {
                        if let Ok(id) = self.add_hostfwd(ohp, ogp) {
                            self.forwards.push((id, ohp, ogp));
                        }
                    }
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    pub fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for Uplink {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
