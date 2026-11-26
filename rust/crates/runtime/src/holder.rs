use crate::validate::{ANDROID_MEDIA_GID, PasswdEntry, member_groups, parse_passwd_line};
use crate::{caps, loopdev, mount, pty, scheduler, uplink};
use protocol::{KernelInfo, PortForward, WinSize, recv_frame, send_frame};
use std::ffi::CString;
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub struct DistroHolder {
    pub pid: i32,
    pub init_pid: i32,
    pub gpu_mounted: bool,
    pub desktop_mounted: bool,
    boot_init: bool,
    sock: UnixStream,
    mount_ns: Option<PathBuf>,
    image_path: PathBuf,
    journal: Option<JournalWorker>,
}

struct JournalWorker {
    pid: i32,
    comm: String,
    dev: u64,
}

impl JournalWorker {
    fn find(mount: &File) -> io::Result<Option<Self>> {
        let dev = mount.metadata()?.dev();
        if libc::major(dev) != 7 {
            return Ok(None);
        }
        let disk = std::fs::read_link(format!("/sys/dev/block/{}:{}", libc::major(dev), libc::minor(dev)))?;
        let Some(disk) = disk.file_name().and_then(|name| name.to_str()) else {
            return Ok(None);
        };
        let journal = format!("{disk}-8");
        if !Path::new("/proc/fs/jbd2").join(&journal).try_exists()? {
            return Ok(None);
        }
        let comm = format!("jbd2/{journal}");
        if comm.len() > 15 {
            return Ok(None);
        }
        for entry in std::fs::read_dir("/proc")?.flatten() {
            let Some(pid) = entry.file_name().to_str().and_then(|name| name.parse::<i32>().ok()) else {
                continue;
            };
            if pid > 0 && is_journal_worker(pid, &comm) {
                return Ok(Some(Self { pid, comm, dev }));
            }
        }
        Err(io::Error::other("image journal worker was not found"))
    }

    fn sync_cpu_min(&self, mount: &File, previous: u32, min: u32) -> io::Result<()> {
        if mount.metadata()?.dev() != self.dev || !is_journal_worker(self.pid, &self.comm) {
            return Err(io::Error::other("image journal worker no longer matches"));
        }
        scheduler::sync_task_min(self.pid, previous, min)
    }
}

fn is_journal_worker(pid: i32, expected: &str) -> bool {
    const PF_KTHREAD: u32 = 0x0020_0000;

    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    let Some((prefix, fields)) = stat.rsplit_once(") ") else {
        return false;
    };
    let Some((_, comm)) = prefix.split_once(" (") else {
        return false;
    };
    comm == expected && fields.split_ascii_whitespace().nth(6)
        .and_then(|flags| flags.parse::<u32>().ok())
        .is_some_and(|flags| flags & PF_KTHREAD != 0)
}

pub struct HolderConfig<'a> {
    pub name: &'a str,
    pub image_path: &'a Path,
    pub mountpoint: &'a Path,
    pub kernel: &'a KernelInfo,
    pub dns_servers: &'a [String],
    pub shared_dir: Option<&'a Path>,
    pub timezone: &'a str,
    pub boot_init: bool,
    pub isolated_net: bool,
    pub ssh_port: u16,
    pub port_forwards: &'a [PortForward],
    pub relaxed_sync: bool,
    pub gpu_mounts: Option<mount::GpuMounts<'a>>,
    pub desktop_dir: Option<&'a Path>,
    pub cpu_group: Option<&'a Path>,
}

pub struct SpawnSpec {
    pub uid: u32,
    pub gid: u32,
    pub size: WinSize,
    pub argv: Vec<String>,
    pub env: Vec<String>,
}

const CMD_SPAWN: u8 = 1;
const CMD_SHUTDOWN: u8 = 2;
const CMD_SYNC_DNS: u8 = 3;
const CMD_STATFS: u8 = 4;
const CMD_SYNC_TZ: u8 = 5;
const CMD_READ_LOG: u8 = 6;
const CMD_SYNC_FORWARDS: u8 = 7;
const CMD_SYNC_CPU_MIN: u8 = 8;
const LOG_TAIL_LIMIT: u64 = 48 * 1024;
pub const GUEST_LOGS: &[(&str, &str)] = &[
    ("autostart", "/var/log/bareboot-autostart.log"),
    ("sshd", "/var/log/bareboot-sshd.log"),
];
const RESP_READY: u8 = 1;
const RESP_SPAWNED: u8 = 2;
const RESP_OK: u8 = 3;
const RESP_ERR: u8 = 255;

impl DistroHolder {
    pub fn start(config: &HolderConfig<'_>) -> io::Result<Self> {
        if config.boot_init && !config.kernel.has_pid_ns {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "starting the distribution's init needs a kernel with PID namespaces (CONFIG_PID_NS)",
            ));
        }
        if config.isolated_net && !config.kernel.isolated_network {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "isolated networking needs PID and network namespaces, TUN, and the Bareboot uplink helper",
            ));
        }
        if loopdev::image_attached(config.image_path)? {
            return Err(io::Error::new(
                io::ErrorKind::ResourceBusy,
                "the image is still mounted by processes from an earlier start; reboot to release it",
            ));
        }
        let (daemon_sock, holder_sock) = UnixStream::pair()?;

        let pid = unsafe { libc::fork() };
        if pid < 0 {
            return Err(io::Error::last_os_error());
        }
        if pid == 0 {
            drop(daemon_sock);
            run_holder_process(holder_sock, config);
        }

        drop(holder_sock);
        let (payload, mount_fd) = recv_frame(daemon_sock.as_raw_fd())?;
        if payload.first() == Some(&RESP_READY) && payload.len() == 11 {
            let real_pid = i32::from_le_bytes(payload[1..5].try_into().unwrap());
            let init_pid = i32::from_le_bytes(payload[5..9].try_into().unwrap());
            let pid = if real_pid > 0 { real_pid } else { pid };
            let root_mount = mount_fd.map(File::from);
            let journal = match root_mount.as_ref().map(JournalWorker::find).transpose() {
                Ok(journal) => journal.flatten(),
                Err(e) => {
                    eprintln!("Could not locate the image journal worker: {e}");
                    None
                }
            };
            if let (Some(journal), Some(mount)) = (&journal, &root_mount)
                && let Ok(min) = scheduler::current_cpu_min()
                && min != 0
                && let Err(e) = journal.sync_cpu_min(mount, 0, min)
            {
                eprintln!("Could not set CPU scheduling for the image journal: {e}");
            }
            Ok(Self {
                pid,
                init_pid,
                gpu_mounted: payload[9] != 0,
                desktop_mounted: payload[10] != 0,
                boot_init: config.boot_init,
                sock: daemon_sock,
                mount_ns: std::fs::read_link(format!("/proc/{pid}/ns/mnt")).ok(),
                image_path: config.image_path.to_path_buf(),
                journal,
            })
        } else {
            let msg = if payload.first() == Some(&RESP_ERR) {
                String::from_utf8_lossy(&payload[1..]).into_owned()
            } else {
                "holder failed to initialize".to_string()
            };
            unsafe {
                let mut status = 0;
                libc::waitpid(pid, &mut status, 0);
            }
            Err(io::Error::other(msg))
        }
    }

    pub fn spawn_session(&self, spec: &SpawnSpec) -> io::Result<(i32, OwnedFd)> {
        let mut buf = Vec::with_capacity(128);
        buf.push(CMD_SPAWN);
        buf.extend_from_slice(&spec.uid.to_le_bytes());
        buf.extend_from_slice(&spec.gid.to_le_bytes());
        buf.extend_from_slice(&spec.size.cols.to_le_bytes());
        buf.extend_from_slice(&spec.size.rows.to_le_bytes());
        buf.extend_from_slice(&spec.size.xpixel.to_le_bytes());
        buf.extend_from_slice(&spec.size.ypixel.to_le_bytes());
        write_str_vec(&mut buf, &spec.argv);
        write_str_vec(&mut buf, &spec.env);

        send_frame(self.sock.as_raw_fd(), &buf, None)?;
        let (resp, fd) = recv_frame(self.sock.as_raw_fd())?;
        match resp.first().copied() {
            Some(RESP_SPAWNED) if resp.len() >= 5 => {
                let child_pid = i32::from_le_bytes(resp[1..5].try_into().unwrap());
                let master = fd.ok_or_else(|| io::Error::other("missing pty master fd"))?;
                Ok((child_pid, master))
            }
            Some(RESP_ERR) => Err(io::Error::other(
                String::from_utf8_lossy(&resp[1..]).into_owned(),
            )),
            _ => Err(io::Error::other("unexpected holder response")),
        }
    }

    pub fn sync_dns(&self, dns_servers: &[String]) -> io::Result<()> {
        let mut buf = Vec::with_capacity(64);
        buf.push(CMD_SYNC_DNS);
        write_str_vec(&mut buf, dns_servers);
        send_frame(self.sock.as_raw_fd(), &buf, None)?;
        let (resp, _) = recv_frame(self.sock.as_raw_fd())?;
        if resp.first().copied() == Some(RESP_OK) {
            Ok(())
        } else {
            Err(io::Error::other("failed to sync dns"))
        }
    }

    pub fn statfs(&self) -> io::Result<(u64, u64)> {
        send_frame(self.sock.as_raw_fd(), &[CMD_STATFS], None)?;
        let (resp, _) = recv_frame(self.sock.as_raw_fd())?;
        if resp.first().copied() == Some(RESP_OK) && resp.len() == 17 {
            Ok((
                u64::from_le_bytes(resp[1..9].try_into().unwrap()),
                u64::from_le_bytes(resp[9..17].try_into().unwrap()),
            ))
        } else {
            Err(io::Error::other("failed to read file system usage"))
        }
    }

    pub fn sync_timezone(&self, timezone: &str) -> io::Result<()> {
        let mut buf = vec![CMD_SYNC_TZ];
        buf.extend_from_slice(timezone.as_bytes());
        send_frame(self.sock.as_raw_fd(), &buf, None)?;
        let (resp, _) = recv_frame(self.sock.as_raw_fd())?;
        if resp.first().copied() == Some(RESP_OK) {
            Ok(())
        } else {
            Err(io::Error::other("failed to sync time zone"))
        }
    }

    pub fn sync_forwards(&self, ssh_port: u16, port_forwards: &[PortForward]) -> io::Result<()> {
        let count = u16::try_from(port_forwards.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "too many port forwards"))?;
        let mut buf = Vec::with_capacity(5 + port_forwards.len() * 4);
        buf.push(CMD_SYNC_FORWARDS);
        buf.extend_from_slice(&ssh_port.to_le_bytes());
        buf.extend_from_slice(&count.to_le_bytes());
        for fwd in port_forwards {
            buf.extend_from_slice(&fwd.host_port.to_le_bytes());
            buf.extend_from_slice(&fwd.guest_port.to_le_bytes());
        }
        send_frame(self.sock.as_raw_fd(), &buf, None)?;
        let (resp, _) = recv_frame(self.sock.as_raw_fd())?;
        match resp.first().copied() {
            Some(RESP_OK) => Ok(()),
            Some(RESP_ERR) => Err(io::Error::other(
                String::from_utf8_lossy(&resp[1..]).into_owned(),
            )),
            _ => Err(io::Error::other("failed to sync port forwards")),
        }
    }

    pub fn sync_cpu_min(&self, previous: u32, min: u32) -> io::Result<()> {
        let mut buf = [CMD_SYNC_CPU_MIN; 9];
        buf[1..5].copy_from_slice(&previous.to_le_bytes());
        buf[5..9].copy_from_slice(&min.to_le_bytes());
        send_frame(self.sock.as_raw_fd(), &buf, None)?;
        let (resp, mount_fd) = recv_frame(self.sock.as_raw_fd())?;
        match resp.first().copied() {
            Some(RESP_OK) => {
                if let Some(worker) = &self.journal {
                    let mount_fd = mount_fd.ok_or_else(|| io::Error::other("missing image mount fd"))?;
                    worker.sync_cpu_min(&File::from(mount_fd), previous, min)?;
                }
                Ok(())
            }
            Some(RESP_ERR) => Err(io::Error::other(
                String::from_utf8_lossy(&resp[1..]).into_owned(),
            )),
            _ => Err(io::Error::other("unexpected holder response")),
        }
    }

    pub fn read_log(&self, name: &str) -> io::Result<Vec<u8>> {
        let mut buf = vec![CMD_READ_LOG];
        buf.extend_from_slice(name.as_bytes());
        send_frame(self.sock.as_raw_fd(), &buf, None)?;
        let (resp, _) = recv_frame(self.sock.as_raw_fd())?;
        match resp.first().copied() {
            Some(RESP_OK) => Ok(resp[1..].to_vec()),
            Some(RESP_ERR) => Err(io::Error::other(String::from_utf8_lossy(&resp[1..]).into_owned())),
            _ => Err(io::Error::other("unexpected holder response")),
        }
    }

    pub fn stop(self) {
        if self.init_pid > 0 {
            if self.boot_init {
                stop_init(self.init_pid);
            } else if let Some(ns) = &self.mount_ns {
                terminate_namespace(ns, &[self.pid, self.init_pid]);
            }
        }
        let _ = send_frame(self.sock.as_raw_fd(), &[CMD_SHUTDOWN], None);
        drop(self.sock);
        unsafe {
            let _ = libc::kill(self.pid, libc::SIGTERM);
            let mut status = 0;
            for _ in 0..30 {
                let r = libc::waitpid(self.pid, &mut status, libc::WNOHANG);
                if r == self.pid || r < 0 {
                    break;
                }
                libc::usleep(50_000);
            }
            if libc::waitpid(self.pid, &mut status, libc::WNOHANG) == 0 {
                let _ = libc::kill(self.pid, libc::SIGKILL);
                let _ = libc::waitpid(self.pid, &mut status, 0);
            }
        }
        if let Some(ns) = &self.mount_ns {
            kill_namespace(ns);
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while loopdev::image_attached(&self.image_path).unwrap_or(false) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

fn stop_init(init_pid: i32) {
    let comm = std::fs::read_to_string(format!("/proc/{init_pid}/comm")).unwrap_or_default();
    let signal = if comm.trim() == "systemd" { libc::SIGRTMIN() + 4 } else { libc::SIGTERM };
    unsafe { libc::kill(init_pid, signal) };
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && unsafe { libc::kill(init_pid, 0) } == 0 && !is_zombie(init_pid) {
        std::thread::sleep(Duration::from_millis(100));
    }
    unsafe { libc::kill(init_pid, libc::SIGKILL) };
}

fn is_zombie(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| stat.rfind(')').and_then(|i| stat[i + 1..].trim_start().chars().next()))
        == Some('Z')
}

fn terminate_namespace(ns: &Path, keep: &[i32]) {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut signalled = false;
    while Instant::now() < deadline {
        let members: Vec<i32> = namespace_members(ns)
            .into_iter()
            .filter(|pid| !keep.contains(pid) && !is_zombie(*pid))
            .collect();
        if members.is_empty() {
            return;
        }
        if !signalled {
            for pid in members {
                unsafe { libc::kill(pid, libc::SIGTERM) };
            }
            signalled = true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn namespace_members(ns: &Path) -> Vec<i32> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<i32>().ok())
        .filter(|pid| std::fs::read_link(format!("/proc/{pid}/ns/mnt")).is_ok_and(|link| link == ns))
        .collect()
}

fn kill_namespace(ns: &Path) {
    let term_deadline = Instant::now() + Duration::from_secs(2);
    let kill_deadline = term_deadline + Duration::from_secs(3);
    loop {
        let members = namespace_members(ns);
        if members.is_empty() {
            return;
        }
        let now = Instant::now();
        if now >= kill_deadline {
            return;
        }
        let signal = if now < term_deadline { libc::SIGTERM } else { libc::SIGKILL };
        for pid in members {
            unsafe { libc::kill(pid, signal) };
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn write_str_vec(buf: &mut Vec<u8>, items: &[String]) {
    buf.extend_from_slice(&(items.len() as u32).to_le_bytes());
    for s in items {
        buf.extend_from_slice(&(s.len() as u32).to_le_bytes());
        buf.extend_from_slice(s.as_bytes());
    }
}

fn read_str_vec(data: &[u8], pos: &mut usize) -> io::Result<Vec<String>> {
    if *pos + 4 > data.len() {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "truncated vec"));
    }
    let count = u32::from_le_bytes(data[*pos..*pos + 4].try_into().unwrap()) as usize;
    *pos += 4;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        if *pos + 4 > data.len() {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "truncated str len"));
        }
        let len = u32::from_le_bytes(data[*pos..*pos + 4].try_into().unwrap()) as usize;
        *pos += 4;
        if *pos + len > data.len() {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "truncated str"));
        }
        let s = String::from_utf8_lossy(&data[*pos..*pos + len]).into_owned();
        *pos += len;
        out.push(s);
    }
    Ok(out)
}

fn ipv4_dns(servers: &[String], isolated: bool) -> Vec<String> {
    servers
        .iter()
        .filter(|server| !isolated || server.parse::<std::net::Ipv4Addr>().is_ok())
        .cloned()
        .collect()
}

fn send_holder_err(sock_fd: libc::c_int, err: &str) {
    let mut buf = Vec::with_capacity(1 + err.len());
    buf.push(RESP_ERR);
    buf.extend_from_slice(err.as_bytes());
    let _ = send_frame(sock_fd, &buf, None);
}

fn run_holder_process(sock: UnixStream, config: &HolderConfig<'_>) -> ! {
    unsafe { libc::umask(0o022) };
    if let Some(procs) = config.cpu_group
        && let Err(e) = scheduler::enter_cpu_group(procs)
    {
        eprintln!("Could not move {} into its CPU group: {e}", config.name);
    }
    let use_pid_ns = config.kernel.has_pid_ns;
    let mut uplink = None;
    let setup = (|| -> io::Result<(File, bool, bool)> {
        let host_net = config.isolated_net
            .then(|| File::open("/proc/self/ns/net"))
            .transpose()?;
        let mut flags = libc::CLONE_NEWNS | libc::CLONE_NEWUTS;
        if config.isolated_net {
            flags |= libc::CLONE_NEWNET;
        }
        if config.kernel.has_ipc_ns {
            flags |= libc::CLONE_NEWIPC;
        }
        if use_pid_ns && !config.isolated_net {
            flags |= libc::CLONE_NEWPID;
        }
        if unsafe { libc::unshare(flags) } != 0 {
            return Err(io::Error::last_os_error());
        }
        mount::make_root_private()?;
        if let Some(host_net) = host_net {
            uplink = Some(uplink::Uplink::start(&host_net, config.mountpoint, config.ssh_port, config.port_forwards)?);
            if unsafe { libc::unshare(libc::CLONE_NEWPID) } != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        let dns = ipv4_dns(config.dns_servers, config.isolated_net);
        if config.isolated_net && dns.is_empty() {
            return Err(io::Error::other("isolated networking needs an IPv4 DNS server"));
        }
        let attached = loopdev::attach_image(config.image_path)?;
        let (gpu_mounted, desktop_mounted) = mount::mount_ext4_and_pivot(
            &attached.device_path,
            config.mountpoint,
            config.name,
            &dns,
            config.shared_dir,
            !use_pid_ns,
            config.relaxed_sync,
            config.gpu_mounts.as_ref(),
            config.desktop_dir,
        )?;
        drop(attached);
        mount::apply_timezone(config.timezone);
        replace_template_hostname(config.name);
        if gpu_mounted && desktop_mounted {
            install_firefox_frame_rate_policy();
        }
        Ok((File::open("/")?, gpu_mounted, desktop_mounted))
    })();

    let (root_mount, gpu_mounted, desktop_mounted) = match setup {
        Ok(mount) => mount,
        Err(e) => {
            send_holder_err(sock.as_raw_fd(), &format!("mount setup failed: {e}"));
            unsafe { libc::_exit(1) };
        }
    };

    let init_pid = if use_pid_ns {
        match start_guest_init(config) {
            Ok(pid) => pid,
            Err(e) => {
                send_holder_err(sock.as_raw_fd(), &format!("guest init failed: {e}"));
                unsafe { libc::_exit(1) };
            }
        }
    } else {
        0
    };

    let mut ready = Vec::with_capacity(11);
    ready.push(RESP_READY);
    ready.extend_from_slice(&(unsafe { libc::getpid() }).to_le_bytes());
    ready.extend_from_slice(&init_pid.to_le_bytes());
    ready.push(u8::from(gpu_mounted));
    ready.push(u8::from(desktop_mounted));
    if send_frame(sock.as_raw_fd(), &ready, Some(root_mount.as_raw_fd())).is_err() {
        unsafe { libc::_exit(1) };
    }
    let mut children: Vec<i32> = Vec::new();
    let sock_fd = sock.as_raw_fd();

    loop {
        reap_children(&mut children);
        if uplink.as_mut().is_some_and(|uplink| !uplink.running()) || (init_pid > 0 && !children_alive(init_pid)) {
            break;
        }

        let mut pfd = libc::pollfd {
            fd: sock_fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let prc = unsafe { libc::poll(&mut pfd, 1, 500) };
        if prc < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        if prc == 0 {
            continue;
        }

        let (payload, _) = match recv_frame(sock_fd) {
            Ok(frame) => frame,
            Err(_) => break,
        };

        match payload.first().copied() {
            Some(CMD_SHUTDOWN) => break,
            Some(CMD_SPAWN) => match handle_spawn(&payload[1..], sock_fd) {
                Ok(child_pid) => children.push(child_pid),
                Err(e) => send_holder_err(sock_fd, &format!("spawn failed: {e}")),
            },
            Some(CMD_SYNC_DNS) => {
                let mut pos = 0;
                match read_str_vec(&payload[1..], &mut pos)
                    .and_then(|servers| {
                        let servers = ipv4_dns(&servers, config.isolated_net);
                        if config.isolated_net && servers.is_empty() {
                            Ok(())
                        } else {
                            mount::write_resolv_conf(Path::new("/"), &servers)
                        }
                    })
                {
                    Ok(()) => {
                        let _ = send_frame(sock_fd, &[RESP_OK], None);
                    }
                    Err(e) => send_holder_err(sock_fd, &format!("dns sync failed: {e}")),
                }
            }
            Some(CMD_STATFS) => match root_usage() {
                Ok((size, free)) => {
                    let mut resp = vec![RESP_OK];
                    resp.extend_from_slice(&size.to_le_bytes());
                    resp.extend_from_slice(&free.to_le_bytes());
                    let _ = send_frame(sock_fd, &resp, None);
                }
                Err(e) => send_holder_err(sock_fd, &format!("statfs failed: {e}")),
            },
            Some(CMD_READ_LOG) => {
                let name = String::from_utf8_lossy(&payload[1..]).into_owned();
                match GUEST_LOGS.iter().find(|(n, _)| *n == name) {
                    Some((_, path)) => match tail_file(Path::new(path), LOG_TAIL_LIMIT) {
                        Ok(bytes) => {
                            let mut resp = vec![RESP_OK];
                            resp.extend_from_slice(&bytes);
                            let _ = send_frame(sock_fd, &resp, None);
                        }
                        Err(e) => send_holder_err(sock_fd, &format!("{path}: {e}")),
                    },
                    None => send_holder_err(sock_fd, "unknown log"),
                }
            }
            Some(CMD_SYNC_TZ) => {
                mount::apply_timezone(&String::from_utf8_lossy(&payload[1..]));
                let _ = send_frame(sock_fd, &[RESP_OK], None);
            }
            Some(CMD_SYNC_FORWARDS) => {
                if payload.len() < 5 {
                    send_holder_err(sock_fd, "truncated port forwards");
                } else {
                    let ssh_port = u16::from_le_bytes([payload[1], payload[2]]);
                    let count = u16::from_le_bytes([payload[3], payload[4]]) as usize;
                    if payload.len() != 5 + count * 4 {
                        send_holder_err(sock_fd, "invalid port forwards payload");
                    } else {
                        let mut forwards = Vec::with_capacity(count);
                        for i in 0..count {
                            let off = 5 + i * 4;
                            forwards.push(PortForward {
                                host_port: u16::from_le_bytes([payload[off], payload[off + 1]]),
                                guest_port: u16::from_le_bytes([payload[off + 2], payload[off + 3]]),
                            });
                        }
                        match uplink.as_mut() {
                            Some(u) => match u.sync_forwards(ssh_port, &forwards) {
                                Ok(()) => {
                                    let _ = send_frame(sock_fd, &[RESP_OK], None);
                                }
                                Err(e) => send_holder_err(sock_fd, &e.to_string()),
                            },
                            None => {
                                let _ = send_frame(sock_fd, &[RESP_OK], None);
                            }
                        }
                    }
                }
            }
            Some(CMD_SYNC_CPU_MIN) if payload.len() == 9 => {
                let previous = u32::from_le_bytes(payload[1..5].try_into().unwrap());
                let min = u32::from_le_bytes(payload[5..9].try_into().unwrap());
                match sync_cpu_min(previous, min, use_pid_ns) {
                    Ok(()) => {
                        let _ = send_frame(sock_fd, &[RESP_OK], Some(root_mount.as_raw_fd()));
                    }
                    Err(e) => send_holder_err(sock_fd, &e.to_string()),
                }
            }
            _ => send_holder_err(sock_fd, "unknown holder cmd"),
        }
    }

    if init_pid > 0 {
        unsafe {
            let _ = libc::kill(init_pid, libc::SIGKILL);
            let mut status = 0;
            let _ = libc::waitpid(init_pid, &mut status, 0);
        }
    }
    for &cpid in &children {
        unsafe {
            let _ = libc::kill(-cpid, libc::SIGTERM);
            let _ = libc::kill(cpid, libc::SIGTERM);
        }
    }
    unsafe {
        libc::usleep(100_000);
    }
    for &cpid in &children {
        unsafe {
            let _ = libc::kill(-cpid, libc::SIGKILL);
            let _ = libc::kill(cpid, libc::SIGKILL);
            let mut status = 0;
            let _ = libc::waitpid(cpid, &mut status, libc::WNOHANG);
        }
    }
    drop(uplink);
    unsafe { libc::_exit(0) };
}

fn sync_cpu_min(previous: u32, min: u32, use_pid_ns: bool) -> io::Result<()> {
    scheduler::set_cpu_util_min(min)?;
    if !use_pid_ns {
        return Ok(());
    }
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(io::Error::last_os_error());
    }
    if pid == 0 {
        let result = scheduler::sync_guest_min(previous, min);
        if let Err(e) = &result {
            eprintln!("Could not update guest CPU scheduling: {e}");
        }
        unsafe { libc::_exit(i32::from(result.is_err())) };
    }
    let mut status = 0;
    loop {
        if unsafe { libc::waitpid(pid, &mut status, 0) } == pid {
            return if status == 0 {
                Ok(())
            } else {
                Err(io::Error::other("could not update guest CPU scheduling"))
            };
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

pub fn tail_file(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a regular file"));
    }
    let len = file.metadata()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(limit)))?;
    let mut out = Vec::new();
    file.take(limit).read_to_end(&mut out)?;
    Ok(out)
}

fn install_firefox_frame_rate_policy() {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    const POLICY: &[u8] =
        br#"{"policies":{"Preferences":{"layout.frame_rate":{"Value":120,"Status":"default"},"widget.dmabuf.force-enabled":{"Value":true,"Status":"default"},"gfx.blacklist.dmabuf.surface-export":{"Value":1,"Status":"default"},"media.rdd-ffvpx.enabled":{"Value":false,"Status":"default"},"media.rdd-vpx.enabled":{"Value":false,"Status":"default"},"media.av1.use-dav1d":{"Value":false,"Status":"default"}}}}"#;
    let dir = Path::new("/etc/firefox/policies");
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .custom_flags(libc::O_NOFOLLOW)
        .open(dir.join("policies.json"))
    {
        let _ = file.write_all(POLICY);
    }
    const PREFS: &[u8] = b"pref(\"layout.frame_rate\", 120);\npref(\"widget.dmabuf.force-enabled\", true);\npref(\"gfx.blacklist.dmabuf.surface-export\", 1);\npref(\"media.rdd-ffvpx.enabled\", false);\npref(\"media.rdd-vpx.enabled\", false);\npref(\"media.av1.use-dav1d\", false);\n";
    for pref_dir in [
        "/usr/lib/firefox-esr/browser/defaults/preferences",
        "/usr/lib/firefox/browser/defaults/preferences",
        "/usr/lib64/firefox/browser/defaults/preferences",
    ] {
        let p = Path::new(pref_dir);
        if std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir())
            && let Ok(mut file) = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o644)
                .custom_flags(libc::O_NOFOLLOW)
                .open(p.join("bareboot.js"))
        {
            let _ = file.write_all(PREFS);
        }
    }
}

fn replace_template_hostname(name: &str) {
    let path = Path::new("/etc/hostname");
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file())
        && std::fs::read_to_string(path).is_ok_and(|text| matches!(text.trim(), "LXC_NAME" | "LXCNAME"))
    {
        let _ = std::fs::write(path, format!("{name}\n"));
    }
}

fn children_alive(init_pid: i32) -> bool {
    let mut status = 0;
    unsafe { libc::waitpid(init_pid, &mut status, libc::WNOHANG) == 0 }
}

fn root_usage() -> io::Result<(u64, u64)> {
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    let root = CString::new("/").unwrap();
    if unsafe { libc::statvfs(root.as_ptr(), &mut st) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let frsize = st.f_frsize as u64;
    Ok((st.f_blocks as u64 * frsize, st.f_bavail as u64 * frsize))
}

fn start_guest_init(config: &HolderConfig<'_>) -> io::Result<i32> {
    let (parent_end, child_end) = UnixStream::pair()?;
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(io::Error::last_os_error());
    }
    if pid == 0 {
        drop(parent_end);
        let result = mount::mount_guest_proc(config.isolated_net).and_then(|()| {
            if config.boot_init {
                mount::delegate_cgroup(config.name)
            } else {
                Ok(())
            }
        });
        let status = match &result {
            Ok(()) => vec![RESP_OK],
            Err(e) => {
                let mut buf = vec![RESP_ERR];
                buf.extend_from_slice(e.to_string().as_bytes());
                buf
            }
        };
        let _ = send_frame(child_end.as_raw_fd(), &status, None);
        drop(child_end);
        if result.is_err() {
            unsafe { libc::_exit(1) };
        }
        if config.boot_init {
            let init = find_system_init();
            if is_systemd_init(init) {
                mask_host_network_units();
            }
            exec_system_init(init);
        }
        run_reaper();
    }
    drop(child_end);
    let (resp, _) = recv_frame(parent_end.as_raw_fd())?;
    if resp.first().copied() == Some(RESP_OK) {
        Ok(pid)
    } else {
        unsafe {
            let mut status = 0;
            libc::waitpid(pid, &mut status, 0);
        }
        Err(io::Error::other(String::from_utf8_lossy(resp.get(1..).unwrap_or_default()).into_owned()))
    }
}

fn run_reaper() -> ! {
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigfillset(&mut set);
        libc::sigprocmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        for fd in 3..1024 {
            libc::close(fd);
        }
        let name = b"bareboot-init\0";
        libc::prctl(libc::PR_SET_NAME, name.as_ptr() as libc::c_ulong, 0, 0, 0);
        loop {
            let mut status = 0;
            if libc::waitpid(-1, &mut status, 0) < 0 && *libc::__errno_location() == libc::ECHILD {
                libc::pause();
            }
        }
    }
}

const MASKED_UNITS: &[&str] = &[
    "systemd-networkd.service",
    "systemd-networkd.socket",
    "systemd-networkd-wait-online.service",
    "systemd-timesyncd.service",
    "systemd-firstboot.service",
    "NetworkManager.service",
    "NetworkManager-wait-online.service",
    "systemd-resolved.service",
    "networking.service",
    "netplan-configure.service",
];

fn mask_host_network_units() {
    use std::os::unix::fs::DirBuilderExt;
    let dir = Path::new("/run/systemd/system");
    if std::fs::DirBuilder::new().recursive(true).mode(0o755).create(dir).is_err() {
        return;
    }
    for unit in MASKED_UNITS {
        let _ = std::os::unix::fs::symlink("/dev/null", dir.join(unit));
    }
}

fn find_system_init() -> &'static str {
    ["/sbin/init", "/usr/lib/systemd/systemd", "/lib/systemd/systemd"]
        .into_iter()
        .find(|p| Path::new(p).exists())
        .unwrap_or("/sbin/init")
}

fn is_systemd_init(init: &str) -> bool {
    std::fs::canonicalize(init)
        .ok()
        .and_then(|p| p.file_name().map(|n| n == "systemd"))
        .unwrap_or(false)
}

fn reset_signals() {
    unsafe {
        for sig in 1..=64 {
            if sig != libc::SIGKILL && sig != libc::SIGSTOP {
                libc::signal(sig, libc::SIG_DFL);
            }
        }
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigprocmask(libc::SIG_SETMASK, &set, std::ptr::null_mut());
    }
}

fn exec_system_init(init: &str) -> ! {
    reset_signals();
    unsafe {
        for fd in 3..1024 {
            libc::close(fd);
        }
        libc::setsid();
        let null = CString::new("/dev/null").unwrap();
        let fd = libc::open(null.as_ptr(), libc::O_RDWR);
        if fd >= 0 {
            libc::dup2(fd, 0);
            libc::dup2(fd, 1);
            libc::dup2(fd, 2);
            if fd > 2 {
                libc::close(fd);
            }
        }
    }
    if let Err(e) = caps::restrict_init_capabilities().and_then(|()| set_exec_context(GUEST_DOMAIN)) {
        eprintln!("guest init: {e}");
        unsafe { libc::_exit(126) };
    }
    let program = CString::new(init).unwrap();
    let argv = [program.as_ptr(), std::ptr::null()];
    let env = [
        CString::new("container=bareboot").unwrap(),
        CString::new("PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin").unwrap(),
        CString::new("TERM=linux").unwrap(),
    ];
    let envp = [env[0].as_ptr(), env[1].as_ptr(), env[2].as_ptr(), std::ptr::null()];
    unsafe {
        libc::execve(program.as_ptr(), argv.as_ptr(), envp.as_ptr());
        libc::_exit(127);
    }
}

fn reap_children(children: &mut Vec<i32>) {
    loop {
        let mut status = 0;
        let pid = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
        if pid <= 0 {
            break;
        }
        children.retain(|&p| p != pid);
    }
}

fn handle_spawn(data: &[u8], sock_fd: libc::c_int) -> io::Result<i32> {
    if data.len() < 16 {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "truncated spawn"));
    }
    let uid = u32::from_le_bytes(data[0..4].try_into().unwrap());
    let gid = u32::from_le_bytes(data[4..8].try_into().unwrap());
    let size = WinSize {
        cols: u16::from_le_bytes(data[8..10].try_into().unwrap()),
        rows: u16::from_le_bytes(data[10..12].try_into().unwrap()),
        xpixel: u16::from_le_bytes(data[12..14].try_into().unwrap()),
        ypixel: u16::from_le_bytes(data[14..16].try_into().unwrap()),
    };
    let mut pos = 16;
    let argv = read_str_vec(data, &mut pos)?;
    let env = read_str_vec(data, &mut pos)?;

    let guest_pty = pty::open_guest_pty(size)?;
    let child_pid = unsafe { libc::fork() };
    if child_pid < 0 {
        return Err(io::Error::last_os_error());
    }
    if child_pid == 0 {
        drop(guest_pty.master);
        unsafe {
            let _ = libc::close(sock_fd);
        }
        exec_guest_session(guest_pty.slave, uid, gid, argv, env);
    }

    drop(guest_pty.slave);
    let mut resp = Vec::with_capacity(5);
    resp.push(RESP_SPAWNED);
    resp.extend_from_slice(&child_pid.to_le_bytes());
    send_frame(sock_fd, &resp, Some(guest_pty.master.as_raw_fd()))?;
    drop(guest_pty.master);
    Ok(child_pid)
}

fn exec_guest_session(
    slave: OwnedFd,
    uid: u32,
    gid: u32,
    argv: Vec<String>,
    extra_env: Vec<String>,
) -> ! {
    reset_signals();
    unsafe {
        let _ = libc::setsid();
        let sfd = slave.as_raw_fd();
        let _ = libc::ioctl(sfd, libc::TIOCSCTTY, 0);
        let _ = libc::dup2(sfd, 0);
        let _ = libc::dup2(sfd, 1);
        let _ = libc::dup2(sfd, 2);
        if sfd > 2 {
            drop(slave);
        } else {
            std::mem::forget(slave);
        }
    }

    let account = guest_account(uid);
    let mut extra_groups = Vec::new();
    if uid != 0 {
        if let (Some(a), Ok(text)) = (&account, std::fs::read_to_string("/etc/group")) {
            extra_groups = member_groups(&text, &a.name);
        }
        extra_groups.push(ANDROID_MEDIA_GID);
    }
    if let Err(e) = caps::restrict_guest_capabilities()
        .and_then(|()| caps::switch_credentials(uid, gid, &extra_groups))
        .and_then(|()| set_exec_context(GUEST_DOMAIN))
    {
        eprintln!("guest credentials: {e}");
        unsafe { libc::_exit(126) };
    }

    let home = account
        .as_ref()
        .map(|a| a.home.clone())
        .unwrap_or_else(|| if uid == 0 { "/root".to_string() } else { "/tmp".to_string() });
    let _ = std::env::set_current_dir(&home);

    let shell = account
        .as_ref()
        .map(|a| a.shell.as_str())
        .into_iter()
        .chain(["/bin/bash", "/bin/sh"])
        .find(|s| !s.is_empty() && std::path::Path::new(s).is_file())
        .unwrap_or("/bin/sh")
        .to_string();
    let login_name = format!("-{}", shell.rsplit('/').next().unwrap_or("sh"));
    let (program, args) = if argv.is_empty() {
        (shell.clone(), vec![login_name])
    } else {
        (argv[0].clone(), argv)
    };
    let c_program = match CString::new(program) {
        Ok(p) => p,
        Err(_) => unsafe { libc::_exit(127) },
    };
    let c_args: Vec<CString> = args
        .iter()
        .filter_map(|s| CString::new(s.as_str()).ok())
        .collect();
    if c_args.is_empty() {
        unsafe { libc::_exit(127) };
    }

    let user = account.map(|a| a.name).unwrap_or_else(|| uid.to_string());
    let mut env_list = vec![
        format!("HOME={home}"),
        format!("USER={user}"),
        format!("LOGNAME={user}"),
        format!("SHELL={shell}"),
        "TERM=xterm-256color".to_string(),
        "COLORTERM=truecolor".to_string(),
        "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string(),
        "LANG=C.UTF-8".to_string(),
    ];
    if std::path::Path::new("/run/bareboot-display/wayland-0").exists() {
        env_list.push("WAYLAND_DISPLAY=/run/bareboot-display/wayland-0".to_string());
        env_list.push("XDG_RUNTIME_DIR=/tmp".to_string());
    }
    if std::path::Path::new("/run/bareboot-gpu/socket/kumquat-gpu-0").exists() {
        let is_musl = std::path::Path::new("/lib/ld-musl-aarch64.so.1").exists();
        let abi = if is_musl { "musl" } else { "glibc" };
        let bundle = format!("/run/bareboot-gpu/guest/{abi}");
        env_list.push("VIRTGPU_KUMQUAT=1".to_string());
        env_list.push("VIRTGPU_KUMQUAT_SOCKET_PATH=/run/bareboot-gpu/socket/kumquat-gpu-0".to_string());
        env_list.push(format!(
            "VK_DRIVER_FILES={bundle}/share/vulkan/icd.d/gfxstream_vk_icd.aarch64.json"
        ));
        env_list.push("MESA_LOADER_DRIVER_OVERRIDE=zink".to_string());
        if std::path::Path::new(&format!("{bundle}/lib/gbm/bareboot_gbm.so")).exists() {
            env_list.push("GBM_BACKEND=bareboot".to_string());
            env_list.push(format!("GBM_BACKENDS_PATH={bundle}/lib/gbm"));
        }
        env_list.push("MOZ_DRM_DEVICE=/dev/null".to_string());
        env_list.push("MOZ_DISABLE_RDD_SANDBOX=1".to_string());
        if !is_musl {
            let hwdec = format!("{bundle}/lib/hwdec");
            if std::path::Path::new(&hwdec).is_dir() {
                env_list.push(format!("LD_LIBRARY_PATH={hwdec}"));
            }
        }
        if is_musl {
            let egl = format!("{bundle}/lib/libEGL.so.1");
            if std::path::Path::new(&egl).exists() {
                env_list.push(format!("LD_LIBRARY_PATH={bundle}/lib"));
            }
        } else {
            let icd = format!("{bundle}/share/glvnd/egl_vendor.d/10_bareboot.json");
            if std::path::Path::new(&icd).exists() {
                env_list.push(format!("__EGL_VENDOR_LIBRARY_FILENAMES={icd}"));
            }
        }
    }
    for e in extra_env {
        if let Some((k, _)) = e.split_once('=') {
            env_list.retain(|existing| !existing.starts_with(&format!("{k}=")));
            env_list.push(e);
        }
    }
    let c_env: Vec<CString> = env_list
        .iter()
        .filter_map(|s| CString::new(s.as_str()).ok())
        .collect();

    let search_path = env_list
        .iter()
        .find_map(|e| e.strip_prefix("PATH="))
        .unwrap_or_default()
        .to_string();
    let c_program = resolve_in_path(c_program, &search_path);
    let mut argv_ptrs: Vec<*const libc::c_char> = c_args.iter().map(|s| s.as_ptr()).collect();
    argv_ptrs.push(std::ptr::null());
    let mut env_ptrs: Vec<*const libc::c_char> = c_env.iter().map(|s| s.as_ptr()).collect();
    env_ptrs.push(std::ptr::null());

    unsafe {
        libc::execve(c_program.as_ptr(), argv_ptrs.as_ptr(), env_ptrs.as_ptr());
    }
    eprintln!("bareboot: cannot execute {}: {}", c_program.to_string_lossy(), io::Error::last_os_error());
    unsafe { libc::_exit(127) };
}

fn resolve_in_path(program: CString, path: &str) -> CString {
    let name = program.to_string_lossy().into_owned();
    if name.contains('/') {
        return program;
    }
    path.split(':')
        .filter(|dir| !dir.is_empty())
        .find_map(|dir| {
            let candidate = format!("{dir}/{name}");
            let meta = std::fs::metadata(&candidate).ok()?;
            if meta.is_file() && meta.mode() & 0o111 != 0 {
                CString::new(candidate).ok()
            } else {
                None
            }
        })
        .unwrap_or(program)
}

const GUEST_DOMAIN: &str = "u:r:bareboot_guest:s0";

fn set_exec_context(context: &str) -> io::Result<()> {
    use std::io::Write;
    std::fs::OpenOptions::new()
        .write(true)
        .open("/proc/thread-self/attr/exec")
        .and_then(|mut f| f.write_all(context.as_bytes()))
        .map_err(|e| io::Error::new(e.kind(), format!("could not set SELinux domain {context}: {e}")))
}

fn guest_account(uid: u32) -> Option<PasswdEntry> {
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    passwd.lines().filter_map(parse_passwd_line).find(|entry| entry.uid == uid)
}
