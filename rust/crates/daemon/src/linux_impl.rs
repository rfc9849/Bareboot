use crate::{DaemonConfig, dns::parse_default_dns};
use protocol::{
    DEFAULT_SOCKET_PATH, DaemonOptions, DistroStatus, KernelInfo, PROTOCOL_VERSION, Request, Response, SessionInfo, WinSize,
    decode_request, encode_response, recv_frame, send_frame,
};
use runtime::auth::PeerPolicy;
use crate::desktop::Desktop;
use crate::gpu::{self, Renderer};
use runtime::holder::{DistroHolder, GUEST_LOGS, HolderConfig, SpawnSpec, tail_file};
use runtime::image::{
    INIT_SYSTEM, configure_gpu, configure_image, create_distro_image, delete_distro_image, export_image, image_settings, list_images,
    rename_image, resize_distro_image,
};
use runtime::mount::remove_guest_cgroup;
use runtime::kernel::{kernel_report, probe_kernel};
use runtime::pty::resize_pty;
use runtime::scheduler::{foreground_util_min, set_cpu_util_min};
use runtime::validate::{
    parse_allowed_uids, validate_distro_name, validate_image_path, validate_os_label, validate_port_forwards, validate_session_env, validate_ssh_port,
    validate_username,
};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::linux::net::SocketAddrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::{SocketAddr, UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_shutdown_signal(_sig: libc::c_int) {
    SHUTDOWN.store(true, Ordering::SeqCst);
}

struct DistroEntry {
    image_path: String,
    holder: DistroHolder,
    renderer: Option<Renderer>,
    desktop: Option<Desktop>,
    keep_awake: bool,
}

type BusyImages = Arc<Mutex<HashMap<String, &'static str>>>;

const WAKE_LOCK_NAME: &str = "bareboot";

const DETACHED_OUTPUT_LIMIT: usize = 32 * 1024;

struct Client {
    stream: UnixStream,
    uid: u32,
}

#[derive(Default)]
struct TabState {
    client_fd: Option<RawFd>,
    output: VecDeque<u8>,
    pty_eof: bool,
}

struct SessionEntry {
    owner_uid: u32,
    tab: Option<TabState>,
    distro: String,
    pid: i32,
    uid: u32,
    master_fd: OwnedFd,
}

struct DaemonState {
    config: DaemonConfig,
    kernel: KernelInfo,
    distros: HashMap<String, DistroEntry>,
    sessions: HashMap<u32, SessionEntry>,
    next_session_id: u32,
    last_dns: Vec<String>,
    last_dns_check: Instant,
    timezone: String,
    wake_locked: bool,
    boosted: bool,
    boost_min: u32,
    last_boost_check: Instant,
    busy: BusyImages,
}

struct JobContext {
    state_dir: PathBuf,
    kernel: KernelInfo,
    dns: Vec<String>,
    timezone: String,
    busy: BusyImages,
}

enum Job {
    Create {
        name: String,
        size_gb: u32,
        tarball_path: String,
        username: String,
        os_label: String,
    },
    Resize {
        name: String,
        size_gb: u32,
    },
    Export {
        name: String,
        dest_path: String,
    },
}

impl Job {
    fn name(&self) -> &str {
        match self {
            Job::Create { name, .. } | Job::Resize { name, .. } | Job::Export { name, .. } => name,
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Job::Create { .. } => "create",
            Job::Resize { .. } => "resize",
            Job::Export { .. } => "export",
        }
    }
}

enum ClientOutcome {
    Keep,
    Drop,
    Handoff,
}

impl DaemonState {
    fn allowed_uids_path(&self) -> PathBuf {
        self.config.state_dir.join("allowed_uids")
    }

    fn mount_dir(&self, name: &str) -> PathBuf {
        self.config.state_dir.join("mnt").join(name)
    }

    fn distros_dir(&self) -> PathBuf {
        self.config.state_dir.join("distros")
    }

    fn prune_dead(&mut self) {
        for (name, entry) in &mut self.distros {
            if let Some(result) = entry.renderer.as_mut().and_then(Renderer::exit) {
                entry.renderer = None;
                log_event(&format!("GPU renderer for {name} exited: {result}"));
            }
        }
        self.sessions.retain(|_, s| process_alive(s.pid));
        let dead_distros: Vec<String> = self
            .distros
            .iter()
            .filter_map(|(k, d)| {
                if process_alive(d.holder.pid) {
                    None
                } else {
                    Some(k.clone())
                }
            })
            .collect();
        for name in dead_distros {
            self.stop_distro(&name);
        }
    }

    fn stop_distro(&mut self, name: &str) {
        self.sessions.retain(|_, s| s.distro != name);
        if let Some(entry) = self.distros.remove(name) {
            entry.holder.stop();
            remove_guest_cgroup(name);
            log_event(&format!("stopped {name}"));
        }
        self.update_wake_lock();
    }

    fn options(&self) -> DaemonOptions {
        DaemonOptions {
            purge_on_uninstall: self.config.state_dir.join(PURGE_MARKER).is_file(),
            relaxed_sync: self.config.state_dir.join(RELAXED_SYNC_MARKER).is_file(),
        }
    }

    fn set_options(&mut self, purge_on_uninstall: bool, relaxed_sync: bool) -> io::Result<()> {
        for (marker, enabled) in [(PURGE_MARKER, purge_on_uninstall), (RELAXED_SYNC_MARKER, relaxed_sync)] {
            let path = self.config.state_dir.join(marker);
            if enabled {
                fs::write(&path, "1\n")?;
            } else if let Err(e) = fs::remove_file(&path)
                && e.kind() != io::ErrorKind::NotFound
            {
                return Err(e);
            }
        }
        Ok(())
    }

    fn update_foreground_boost(&mut self) {
        if self.last_boost_check.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_boost_check = Instant::now();
        let want = app_in_foreground(&self.allowed_uids_path());
        if want == self.boosted {
            return;
        }
        let previous = if self.boosted { self.boost_min } else { 0 };
        let min = if want { self.boost_min } else { 0 };
        self.boosted = want;
        if let Err(e) = set_cpu_util_min(min) {
            log_event(&format!("could not update the daemon CPU scheduling: {e}"));
        }
        for (name, entry) in &self.distros {
            if let Err(e) = entry.holder.sync_cpu_min(previous, min) {
                log_event(&format!("could not update CPU scheduling for {name}: {e}"));
            }
        }
    }

    fn is_busy(&self, name: &str) -> Option<&'static str> {
        self.busy.lock().ok()?.get(name).copied()
    }

    fn update_wake_lock(&mut self) {
        let want = self.distros.values().any(|d| d.keep_awake);
        if want == self.wake_locked {
            return;
        }
        let file = if want { "/sys/power/wake_lock" } else { "/sys/power/wake_unlock" };
        match fs::write(file, WAKE_LOCK_NAME) {
            Ok(()) => self.wake_locked = want,
            Err(e) => eprintln!("could not update the wake lock through {file}: {e}"),
        }
    }

    fn maybe_sync_dns(&mut self) {
        if self.distros.is_empty() || self.last_dns_check.elapsed() < Duration::from_secs(5) {
            return;
        }
        if self.config.dns_servers.is_empty() {
            self.refresh_dns();
        } else {
            self.last_dns_check = Instant::now();
        }
        let timezone = android_timezone();
        if timezone != self.timezone {
            self.timezone = timezone;
            for entry in self.distros.values() {
                let _ = entry.holder.sync_timezone(&self.timezone);
            }
        }
    }

    fn current_dns(&mut self) -> Vec<String> {
        if self.config.dns_servers.is_empty() {
            self.refresh_dns();
            self.last_dns.clone()
        } else {
            self.config.dns_servers.clone()
        }
    }

    fn job_context(&mut self) -> JobContext {
        JobContext {
            state_dir: self.config.state_dir.clone(),
            kernel: self.kernel.clone(),
            dns: self.current_dns(),
            timezone: self.timezone.clone(),
            busy: Arc::clone(&self.busy),
        }
    }

    fn prepare_job(&mut self, req: &Request) -> Option<Result<Job, Response>> {
        let job = match req.clone() {
            Request::CreateDistro {
                name,
                size_gb,
                tarball_path,
                username,
                os_label,
            } => {
                if let Err(e) = validate_distro_name(&name)
                    .and_then(|()| validate_username(&username))
                    .and_then(|()| validate_os_label(&os_label))
                    .and_then(|()| validate_image_path(&tarball_path))
                {
                    return Some(Err(err_resp(libc::EINVAL as u32, e.to_string())));
                }
                Job::Create {
                    name,
                    size_gb,
                    tarball_path,
                    username,
                    os_label,
                }
            }
            Request::ResizeDistro { name, size_gb } => Job::Resize { name, size_gb },
            Request::ExportImage { name, dest_path } => Job::Export { name, dest_path },
            _ => return None,
        };
        if let Err(e) = validate_distro_name(job.name()) {
            return Some(Err(err_resp(libc::EINVAL as u32, e.to_string())));
        }
        if self.distros.contains_key(job.name()) {
            return Some(Err(err_resp(libc::EBUSY as u32, format!("stop {} first", job.name()))));
        }
        let Ok(mut busy) = self.busy.lock() else {
            return Some(Err(err_resp(libc::EIO as u32, "internal state is unavailable".into())));
        };
        if let Some(label) = busy.get(job.name()) {
            return Some(Err(err_resp(libc::EBUSY as u32, format!("{} is busy ({label})", job.name()))));
        }
        busy.insert(job.name().to_string(), job.label());
        Some(Ok(job))
    }

    fn refresh_dns(&mut self) {
        self.last_dns_check = Instant::now();
        let current = detect_android_dns();
        if current != self.last_dns {
            self.last_dns = current.clone();
            for entry in self.distros.values() {
                let _ = entry.holder.sync_dns(&current);
            }
        }
    }

    fn handle_request(&mut self, req: Request, client: &Client) -> (Response, Option<OwnedFd>) {
        self.prune_dead();
        match req {
            Request::Ping => (
                Response::Pong {
                    version: PROTOCOL_VERSION,
                    daemon_pid: unsafe { libc::getpid() },
                    selinux_enforcing: self.kernel.selinux_enforcing,
                },
                None,
            ),
            Request::Status => {
                let distros = self
                    .distros
                    .iter()
                    .map(|(name, d)| DistroStatus {
                        name: name.clone(),
                        image_path: d.image_path.clone(),
                        holder_pid: d.holder.pid,
                        init_pid: d.holder.init_pid,
                    })
                    .collect();
                let sessions = self.list_sessions(client.uid, false);
                (
                    Response::Status {
                        kernel: self.kernel.clone(),
                        distros,
                        sessions,
                        options: self.options(),
                    },
                    None,
                )
            }
            Request::StartDistro {
                name,
                image_path,
                shared_dir,
            } => {
                if let Err(e) = validate_distro_name(&name) {
                    return (err_resp(libc::EINVAL as u32, e.to_string()), None);
                }
                if let Err(e) = validate_image_path(&image_path) {
                    return (err_resp(libc::EINVAL as u32, e.to_string()), None);
                }
                if !shared_dir.is_empty()
                    && let Err(e) = validate_image_path(&shared_dir)
                {
                    return (err_resp(libc::EINVAL as u32, e.to_string()), None);
                }
                if self.distros.contains_key(&name) {
                    return (Response::Ok, None);
                }
                if let Some(label) = self.is_busy(&name) {
                    return (err_resp(libc::EBUSY as u32, format!("{name} is busy ({label})")), None);
                }
                let img = Path::new(&image_path);
                if !img.is_file() {
                    return (
                        err_resp(libc::ENOENT as u32, format!("image not found: {image_path}")),
                        None,
                    );
                }
                let mnt = self.mount_dir(&name);
                let dns = self.current_dns();
                self.timezone = android_timezone();
                let settings = image_settings(&self.distros_dir().join(&name));
                let shared_opt = if shared_dir.is_empty() {
                    None
                } else {
                    Some(Path::new(shared_dir.as_str()))
                };
                let mut renderer = if settings.gpu_enabled {
                    match gpu::bundle()
                        .ok_or_else(|| io::Error::other("GPU module bundle is missing"))
                        .and_then(|bundle| Renderer::start(&bundle))
                    {
                        Ok(renderer) => Some(renderer),
                        Err(e) => {
                            log_event(&format!("GPU unavailable for {name}: {e}"));
                            None
                        }
                    }
                } else {
                    None
                };
                let mut desktop = match Desktop::prepare(renderer.as_ref().map(Renderer::presentation), renderer.as_ref().map(Renderer::encoder)) {
                    Ok(desktop) => desktop,
                    Err(e) => {
                        log_event(&format!("display unavailable for {name}: {e}"));
                        None
                    }
                };
                let config = HolderConfig {
                    name: &name,
                    image_path: img,
                    mountpoint: &mnt,
                    kernel: &self.kernel,
                    dns_servers: &dns,
                    shared_dir: shared_opt,
                    timezone: &self.timezone,
                    boot_init: settings.init == INIT_SYSTEM,
                    isolated_net: settings.isolated_net,
                    ssh_port: settings.ssh_port,
                    port_forwards: &settings.port_forwards,
                    relaxed_sync: self.options().relaxed_sync,
                    gpu_mounts: renderer.as_ref().map(Renderer::mounts),
                    desktop_dir: desktop.as_ref().map(|d| d.dir.as_path()),
                };
                match DistroHolder::start(&config) {
                    Ok(holder) => {
                        log_event(&format!(
                            "started {name} (holder {}, init {}, boot init {})",
                            holder.pid, holder.init_pid, config.boot_init
                        ));
                        if !holder.gpu_mounted {
                            renderer = None;
                        } else {
                            log_event(&format!("started GPU renderer for {name}"));
                        }
                        if !holder.desktop_mounted {
                            desktop = None;
                        }
                        self.distros.insert(
                            name.clone(),
                            DistroEntry {
                                image_path,
                                holder,
                                renderer,
                                desktop,
                                keep_awake: settings.keep_awake,
                            },
                        );
                        self.update_wake_lock();
                        self.run_autostart(&name);
                        (Response::Ok, None)
                    }
                    Err(e) => {
                        log_event(&format!("could not start {name}: {e}"));
                        (err_resp(libc::EIO as u32, e.to_string()), None)
                    }
                }
            }
            Request::StopDistro { name } => {
                if let Err(e) = validate_distro_name(&name) {
                    return (err_resp(libc::EINVAL as u32, e.to_string()), None);
                }
                self.stop_distro(&name);
                (Response::Ok, None)
            }
            Request::OpenSession {
                distro,
                uid,
                gid,
                size,
                argv,
                env,
            } => {
                self.open_session(
                    distro,
                    SpawnSpec { uid, gid, size, argv, env },
                    client.uid,
                    false,
                )
            }
            Request::CreateTab { distro, uid, argv, env } => {
                if let Err(e) = validate_session_env(&env) {
                    return (err_resp(libc::EINVAL as u32, e.to_string()), None);
                }
                self.open_session(
                    distro,
                    SpawnSpec {
                        uid,
                        gid: uid,
                        size: DEFAULT_WIN_SIZE,
                        argv,
                        env,
                    },
                    client.uid,
                    true,
                )
            }
            Request::AttachTab { session_id, size } => {
                let Some(session) = self.sessions.get_mut(&session_id).filter(|s| s.owner_uid == client.uid) else {
                    return (err_resp(libc::ENOENT as u32, "terminal tab not found".into()), None);
                };
                let Some(tab) = &mut session.tab else {
                    return (err_resp(libc::ENOENT as u32, "terminal tab not found".into()), None);
                };
                if tab.client_fd.is_some() {
                    return (err_resp(libc::EBUSY as u32, "terminal tab is already attached".into()), None);
                }
                let fd = match session.master_fd.try_clone() {
                    Ok(fd) => fd,
                    Err(e) => return (err_resp(libc::EIO as u32, e.to_string()), None),
                };
                if let Err(e) = resize_pty(session.master_fd.as_raw_fd(), size) {
                    return (err_resp(libc::EIO as u32, e.to_string()), None);
                }
                tab.client_fd = Some(client.stream.as_raw_fd());
                (
                    Response::TabAttached(tab.output.iter().copied().collect()),
                    Some(fd),
                )
            }
            Request::ListTabs => (Response::Sessions(self.list_sessions(client.uid, true)), None),
            Request::ResizePty { session_id, size } => {
                let Some(session) = self.sessions.get(&session_id).filter(|s| client.uid == 0 || s.owner_uid == client.uid) else {
                    return (err_resp(libc::ENOENT as u32, "session not found".into()), None);
                };
                match resize_pty(session.master_fd.as_raw_fd(), size) {
                    Ok(()) => (Response::Ok, None),
                    Err(e) => (err_resp(libc::EIO as u32, e.to_string()), None),
                }
            }
            Request::KillSession { session_id, signal } => {
                let Some(session) = self.sessions.get(&session_id).filter(|s| client.uid == 0 || s.owner_uid == client.uid) else {
                    return (err_resp(libc::ENOENT as u32, "session not found".into()), None);
                };
                let sig = if signal == 0 { libc::SIGTERM } else { signal };
                unsafe {
                    let _ = libc::kill(-session.pid, sig);
                    let _ = libc::kill(session.pid, sig);
                }
                (Response::Ok, None)
            }
            Request::ListSessions => (Response::Sessions(self.list_sessions(client.uid, false)), None),
            Request::ListImages => {
                let mut images = list_images(&self.distros_dir());
                let busy = self.busy.lock().map(|b| b.clone()).unwrap_or_default();
                let gpu_available = gpu::memory_available() && gpu::bundle().is_some();
                for image in &mut images {
                    image.gpu_available = gpu_available;
                    image.gpu_running = self.distros.get(&image.name).is_some_and(|d| d.renderer.is_some());
                    if let Some(label) = busy.get(&image.name) {
                        image.busy = (*label).to_string();
                    }
                    if let Some(entry) = self.distros.get(&image.name)
                        && let Ok((size, free)) = entry.holder.statfs()
                    {
                        image.fs_size_bytes = size;
                        image.fs_free_bytes = free;
                    }
                }
                (Response::Images(images), None)
            }
            Request::DeleteDistro { name } => {
                if let Err(e) = validate_distro_name(&name) {
                    return (err_resp(libc::EINVAL as u32, e.to_string()), None);
                }
                if self.distros.contains_key(&name) {
                    return (err_resp(libc::EBUSY as u32, "stop the distro before deleting it".into()), None);
                }
                if let Some(label) = self.is_busy(&name) {
                    return (err_resp(libc::EBUSY as u32, format!("{name} is busy ({label})")), None);
                }
                match delete_distro_image(&name, &self.config.state_dir) {
                    Ok(()) => (Response::Ok, None),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => (err_resp(libc::ENOENT as u32, e.to_string()), None),
                    Err(e) => (err_resp(libc::EIO as u32, e.to_string()), None),
                }
            }
            Request::KernelReport => {
                let (release, checks) = kernel_report();
                (Response::KernelReport { release, checks }, None)
            }
            Request::CreateDistro { .. } | Request::ResizeDistro { .. } | Request::ExportImage { .. } => {
                (err_resp(libc::EINVAL as u32, "request must run as a job".into()), None)
            }
            Request::AttachDesktop { name } => {
                let Some(entry) = self.distros.get_mut(&name) else {
                    return (err_resp(libc::ENOENT as u32, "start the distribution first".into()), None);
                };
                let Some(display) = &mut entry.desktop else {
                    return (err_resp(libc::ENOTSUP as u32, "display is unavailable for this distribution".into()), None);
                };
                match display.attach() {
                    Ok(view) => (Response::Ok, Some(view)),
                    Err(e) => {
                        log_event(&format!("could not open display for {name}: {e}"));
                        (err_resp(libc::EIO as u32, e.to_string()), None)
                    }
                }
            }
            Request::ConfigureGpu { name, enabled } => {
                if let Err(e) = validate_distro_name(&name) {
                    return (err_resp(libc::EINVAL as u32, e.to_string()), None);
                }
                if self.distros.contains_key(&name) || self.is_busy(&name).is_some() {
                    return (err_resp(libc::EBUSY as u32, "stop the distro before changing GPU settings".into()), None);
                }
                if enabled && (!gpu::memory_available() || gpu::bundle().is_none()) {
                    return (err_resp(libc::ENOTSUP as u32, "GPU module bundle or system DMA heap is missing".into()), None);
                }
                match configure_gpu(&self.distros_dir().join(&name), enabled) {
                    Ok(()) => (Response::Ok, None),
                    Err(e) => (err_resp(e.raw_os_error().unwrap_or(libc::EIO) as u32, e.to_string()), None),
                }
            }
            Request::ConfigureImage {
                name,
                os_label,
                username,
                keep_awake,
                init,
                isolated_net,
                ssh_port,
                port_forwards,
            } => {
                if let Err(e) = validate_distro_name(&name)
                    .and_then(|()| validate_username(&username))
                    .and_then(|()| validate_os_label(&os_label))
                    .and_then(|()| validate_ssh_port(ssh_port))
                    .and_then(|()| validate_port_forwards(&port_forwards, ssh_port))
                {
                    return (err_resp(libc::EINVAL as u32, e.to_string()), None);
                }
                if let Some(label) = self.is_busy(&name) {
                    return (err_resp(libc::EBUSY as u32, format!("{name} is busy ({label})")), None);
                }
                if isolated_net && !self.kernel.isolated_network {
                    return (err_resp(libc::ENOTSUP as u32, "isolated networking is not available on this device".into()), None);
                }
                let prev = image_settings(&self.distros_dir().join(&name));
                if self.distros.contains_key(&name) && prev.isolated_net != isolated_net {
                    return (err_resp(libc::EBUSY as u32, format!("stop {name} before changing its network")), None);
                }
                if let Some(entry) = self.distros.get(&name)
                    && isolated_net
                    && (prev.ssh_port != ssh_port || prev.port_forwards != port_forwards)
                    && let Err(e) = entry.holder.sync_forwards(ssh_port, &port_forwards)
                {
                    return (err_resp(libc::EIO as u32, e.to_string()), None);
                }
                match configure_image(
                    &name,
                    &self.config.state_dir,
                    &os_label,
                    &username,
                    keep_awake,
                    &init,
                    isolated_net,
                    ssh_port,
                    &port_forwards,
                ) {
                    Ok(()) => {
                        if let Some(entry) = self.distros.get_mut(&name) {
                            entry.keep_awake = keep_awake;
                        }
                        self.update_wake_lock();
                        (Response::Ok, None)
                    }
                    Err(e) if e.kind() == io::ErrorKind::NotFound => (err_resp(libc::ENOENT as u32, e.to_string()), None),
                    Err(e) if e.kind() == io::ErrorKind::InvalidInput => (err_resp(libc::EINVAL as u32, e.to_string()), None),
                    Err(e) => (err_resp(libc::EIO as u32, e.to_string()), None),
                }
            }
            Request::SetDaemonOptions {
                purge_on_uninstall,
                relaxed_sync,
            } => match self.set_options(purge_on_uninstall, relaxed_sync) {
                Ok(()) => (Response::Ok, None),
                Err(e) => (err_resp(libc::EIO as u32, e.to_string()), None),
            },
            Request::ReadLog { distro, name } => {
                if distro.is_empty() && name == "daemon" {
                    return match tail_file(&self.config.state_dir.join(DAEMON_LOG), 48 * 1024) {
                        Ok(bytes) => (Response::Log(bytes), None),
                        Err(e) => (err_resp(libc::EIO as u32, e.to_string()), None),
                    };
                }
                if !GUEST_LOGS.iter().any(|(n, _)| *n == name) {
                    return (err_resp(libc::EINVAL as u32, format!("unknown log: {name}")), None);
                }
                let Some(entry) = self.distros.get(&distro) else {
                    return (err_resp(libc::ENOENT as u32, format!("distro not running: {distro}")), None);
                };
                match entry.holder.read_log(&name) {
                    Ok(bytes) => (Response::Log(bytes), None),
                    Err(e) => (err_resp(libc::ENOENT as u32, e.to_string()), None),
                }
            }
            Request::RenameImage { name, new_name } => {
                if let Err(e) = validate_distro_name(&name).and_then(|()| validate_distro_name(&new_name)) {
                    return (err_resp(libc::EINVAL as u32, e.to_string()), None);
                }
                if self.distros.contains_key(&name) {
                    return (err_resp(libc::EBUSY as u32, format!("stop {name} before renaming it")), None);
                }
                if let Some(label) = self.is_busy(&name).or_else(|| self.is_busy(&new_name)) {
                    return (err_resp(libc::EBUSY as u32, format!("{name} is busy ({label})")), None);
                }
                match rename_image(&name, &new_name, &self.config.state_dir) {
                    Ok(()) => (Response::Ok, None),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => (err_resp(libc::ENOENT as u32, e.to_string()), None),
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (err_resp(libc::EEXIST as u32, e.to_string()), None),
                    Err(e) => (err_resp(libc::EIO as u32, e.to_string()), None),
                }
            }
        }
    }

    fn run_autostart(&mut self, distro: &str) {
        let spec = SpawnSpec {
            uid: 0,
            gid: 0,
            size: DEFAULT_WIN_SIZE,
            argv: AUTOSTART.iter().map(|s| s.to_string()).collect(),
            env: Vec::new(),
        };
        let (_, fd) = self.open_session(distro.to_string(), spec, 0, false);
        drop(fd);
    }

    fn open_session(
        &mut self,
        distro: String,
        spec: SpawnSpec,
        owner_uid: u32,
        detachable: bool,
    ) -> (Response, Option<OwnedFd>) {
        if let Err(e) = validate_distro_name(&distro) {
            return (err_resp(libc::EINVAL as u32, e.to_string()), None);
        }
        let Some(entry) = self.distros.get(&distro) else {
            return (
                err_resp(libc::ENOENT as u32, format!("distro not running: {distro}")),
                None,
            );
        };
        let (pid, master_fd) = match entry.holder.spawn_session(&spec) {
            Ok(result) => result,
            Err(e) => return (err_resp(libc::EIO as u32, e.to_string()), None),
        };
        let client_fd = if detachable {
            None
        } else {
            match master_fd.try_clone() {
                Ok(fd) => Some(fd),
                Err(e) => {
                    unsafe { libc::kill(pid, libc::SIGHUP) };
                    return (err_resp(libc::EIO as u32, e.to_string()), None);
                }
            }
        };
        let id = self.next_session_id;
        self.next_session_id = self.next_session_id.wrapping_add(1).max(1);
        self.sessions.insert(
            id,
            SessionEntry {
                owner_uid,
                tab: detachable.then(TabState::default),
                distro,
                pid,
                uid: spec.uid,
                master_fd,
            },
        );
        (Response::SessionOpened { session_id: id, pid }, client_fd)
    }

    fn list_sessions(&self, uid: u32, tabs_only: bool) -> Vec<SessionInfo> {
        let mut sessions: Vec<_> = self.sessions
            .iter()
            .filter(|(_, s)| {
                if tabs_only {
                    s.owner_uid == uid && s.tab.is_some()
                } else {
                    uid == 0 || s.owner_uid == uid
                }
            })
            .map(|(&session_id, s)| SessionInfo {
                session_id,
                distro: s.distro.clone(),
                pid: s.pid,
                uid: s.uid,
            })
            .collect();
        sessions.sort_unstable_by_key(|s| s.session_id);
        sessions
    }

    fn detach_client(&mut self, fd: RawFd) {
        for session in self.sessions.values_mut() {
            if let Some(tab) = &mut session.tab
                && tab.client_fd == Some(fd)
            {
                tab.client_fd = None;
            }
        }
    }

    fn capture_output(&mut self, id: u32) {
        let Some(session) = self.sessions.get_mut(&id) else {
            return;
        };
        let Some(tab) = &mut session.tab else {
            return;
        };
        let mut buffer = [0u8; 8192];
        let n = unsafe {
            libc::read(session.master_fd.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len())
        };
        if n > 0 {
            let n = n as usize;
            let excess = (tab.output.len() + n).saturating_sub(DETACHED_OUTPUT_LIMIT);
            tab.output.drain(..excess);
            tab.output.extend(&buffer[..n]);
        } else if n == 0 || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            tab.pty_eof = true;
        }
    }

    fn shutdown_all(&mut self) {
        let names: Vec<String> = self.distros.keys().cloned().collect();
        for name in names {
            self.stop_distro(&name);
        }
        self.sessions.clear();
    }
}

fn log_event(message: &str) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let rem = secs % 86_400;
    eprintln!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC {message}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    );
}

fn err_resp(code: u32, message: String) -> Response {
    Response::Error { code, message }
}

fn process_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    let rc = unsafe { libc::kill(pid, 0) };
    if rc == 0 {
        let stat_path = format!("/proc/{pid}/stat");
        if let Ok(stat) = fs::read_to_string(stat_path)
            && let Some(idx) = stat.rfind(')')
            && let Some(state) = stat[idx + 1..].trim_start().chars().next()
            && state == 'Z'
        {
            return false;
        }
        true
    } else {
        io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

fn android_timezone() -> String {
    std::process::Command::new("/system/bin/getprop")
        .arg("persist.sys.timezone")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_default()
}

fn run_job(job: Job, ctx: JobContext) -> Response {
    let name = job.name().to_string();
    let label = job.label();
    log_event(&format!("{label} {name}: started"));
    let result = match job {
        Job::Create {
            name,
            size_gb,
            tarball_path,
            username,
            os_label,
        } => create_distro_image(&name, size_gb, Path::new(&tarball_path), &username, &os_label, &ctx.state_dir, &ctx.kernel)
            .and_then(|created| {
                if !created.needs_pacman_keyring {
                    return Ok(());
                }
                init_pacman_keyring(&name, &created.image_path, &ctx).map_err(|e| {
                    io::Error::other(format!(
                        "{name} was created, but its pacman keyring could not be set up ({e}). \
                         Run pacman-key --init and pacman-key --populate as root."
                    ))
                })
            }),
        Job::Resize { name, size_gb } => resize_distro_image(&name, size_gb, &ctx.state_dir),
        Job::Export { name, dest_path } => export_image(&name, &ctx.state_dir, &dest_path),
    };
    if let Ok(mut busy) = ctx.busy.lock() {
        busy.remove(&name);
    }
    match &result {
        Ok(()) => log_event(&format!("{label} {name}: finished")),
        Err(e) => log_event(&format!("{label} {name}: {e}")),
    }
    match result {
        Ok(()) => Response::Ok,
        Err(e) => {
            let code = match e.kind() {
                io::ErrorKind::NotFound => libc::ENOENT,
                io::ErrorKind::InvalidInput => libc::EINVAL,
                io::ErrorKind::AlreadyExists => libc::EEXIST,
                io::ErrorKind::ResourceBusy => libc::EBUSY,
                _ => libc::EIO,
            };
            err_resp(code as u32, e.to_string())
        }
    }
}

fn init_pacman_keyring(name: &str, image_path: &Path, ctx: &JobContext) -> io::Result<()> {
    let mountpoint = ctx.state_dir.join("mnt").join(name);
    let holder = DistroHolder::start(&HolderConfig {
        name,
        image_path,
        mountpoint: &mountpoint,
        kernel: &ctx.kernel,
        dns_servers: &ctx.dns,
        shared_dir: None,
        timezone: &ctx.timezone,
        boot_init: false,
        isolated_net: false,
        ssh_port: 0,
        port_forwards: &[],
        relaxed_sync: false,
        gpu_mounts: None,
        desktop_dir: None,
    })?;
    let result = holder
        .spawn_session(&SpawnSpec {
            uid: 0,
            gid: 0,
            size: DEFAULT_WIN_SIZE,
            argv: PACMAN_KEYRING_SETUP.iter().map(|s| s.to_string()).collect(),
            env: Vec::new(),
        })
        .and_then(|(pid, master)| wait_for_setup(pid, &master, KEYRING_SETUP_TIMEOUT));
    holder.stop();
    result
}

fn detect_android_dns() -> Vec<String> {
    if let Ok(out) = std::process::Command::new("/system/bin/dumpsys")
        .args(["-t", "2", "connectivity"])
        .output()
        && out.status.success()
    {
        let servers = parse_default_dns(&String::from_utf8_lossy(&out.stdout));
        if !servers.is_empty() {
            return servers;
        }
    }
    vec!["1.1.1.1".to_string(), "8.8.8.8".to_string()]
}

const AUTOSTART: &[&str] = &[
    "/bin/sh",
    "-c",
    "if [ -x /etc/bareboot/autostart ]; then /etc/bareboot/autostart; fi </dev/null >/var/log/bareboot-autostart.log 2>&1",
];
const PACMAN_KEYRING_SETUP: &[&str] = &[
    "/bin/sh",
    "-c",
    "/usr/bin/pacman-key --init && /usr/bin/pacman-key --populate && echo bareboot-setup-ok",
];
const SETUP_OK_MARKER: &[u8] = b"bareboot-setup-ok";
const KEYRING_SETUP_TIMEOUT: Duration = Duration::from_secs(150);

fn wait_for_setup(pid: i32, master: &OwnedFd, timeout: Duration) -> io::Result<()> {
    let deadline = Instant::now() + timeout;
    let mut tail: Vec<u8> = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            unsafe { libc::kill(-pid, libc::SIGKILL) };
            return Err(io::Error::new(io::ErrorKind::TimedOut, "setup did not finish in time"));
        }
        let mut pfd = libc::pollfd {
            fd: master.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut pfd, 1, left.as_millis().min(1000) as libc::c_int) };
        if ready < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        if ready == 0 {
            continue;
        }
        let n = unsafe { libc::read(master.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len()) };
        if n <= 0 {
            break;
        }
        tail.extend_from_slice(&buffer[..n as usize]);
        let excess = tail.len().saturating_sub(1024);
        tail.drain(..excess);
    }
    if tail.windows(SETUP_OK_MARKER.len()).any(|w| w == SETUP_OK_MARKER) {
        Ok(())
    } else {
        let text = String::from_utf8_lossy(&tail);
        let last = text.lines().rev().map(str::trim).find(|l| !l.is_empty()).unwrap_or("no output");
        Err(io::Error::other(last.to_string()))
    }
}

fn enter_module_domain() -> io::Result<()> {
    const CURRENT: &str = "/proc/self/attr/current";
    const DOMAIN: &str = "u:r:bareboot_daemon:s0";
    if fs::read_to_string("/sys/fs/selinux/enforce")?.trim() != "1" {
        return Err(io::Error::other("the Bareboot module requires SELinux Enforcing"));
    }
    if fs::read_to_string(CURRENT)?.trim_end_matches('\0') != DOMAIN {
        use std::io::Write;
        fs::OpenOptions::new()
            .write(true)
            .open(CURRENT)?
            .write_all(&[DOMAIN.as_bytes(), b"\0"].concat())?;
    }
    if fs::read_to_string(CURRENT)?.trim_end_matches('\0') != DOMAIN {
        return Err(io::Error::other("could not enter the Bareboot SELinux domain"));
    }
    Ok(())
}

const SOCKET_SELINUX_CONTEXT: &str = "u:object_r:bareboot_socket:s0";

/// Sets the label only when it differs, so files that already carry it need no relabel permission.
pub(super) fn ensure_selinux_label(path: &Path, label: &str) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))?;
    let mut current = [0u8; 256];
    let len = unsafe {
        libc::lgetxattr(
            c_path.as_ptr(),
            c"security.selinux".as_ptr(),
            current.as_mut_ptr().cast(),
            current.len(),
        )
    };
    if len > 0 && current[..len as usize].strip_suffix(b"\0").unwrap_or(&current[..len as usize]) == label.as_bytes() {
        return Ok(());
    }
    set_selinux_label(path, label)
}

pub(super) fn set_selinux_label(path: &Path, label: &str) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))?;
    let c_attr = CString::new("security.selinux").unwrap();
    let mut bytes = label.as_bytes().to_vec();
    bytes.push(0);
    let rc = unsafe {
        libc::lsetxattr(
            c_path.as_ptr(),
            c_attr.as_ptr(),
            bytes.as_ptr().cast(),
            bytes.len(),
            0,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn bind_filesystem_socket(path: &Path) -> io::Result<UnixListener> {
    let is_default = path == Path::new(DEFAULT_SOCKET_PATH);
    if let Some(parent) = path.parent() {
        let existed = parent.exists();
        fs::create_dir_all(parent)?;
        if is_default || !existed {
            fs::set_permissions(parent, fs::Permissions::from_mode(0o711))?;
            if let Err(e) = set_selinux_label(parent, SOCKET_SELINUX_CONTEXT)
                && is_default
            {
                return Err(io::Error::new(e.kind(), format!("could not label socket dir: {e}")));
            }
        }
    }
    if let Ok(meta) = fs::symlink_metadata(path)
        && !meta.is_dir()
    {
        fs::remove_file(path)?;
    }
    let prev_umask = unsafe { libc::umask(0o111) };
    let listener = UnixListener::bind(path);
    unsafe { libc::umask(prev_umask) };
    let listener = listener?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o666))?;
    if let Err(e) = set_selinux_label(path, SOCKET_SELINUX_CONTEXT)
        && is_default
    {
        return Err(io::Error::new(e.kind(), format!("could not label socket: {e}")));
    }
    Ok(listener)
}

const DAEMON_LOG: &str = "barebootd.log";
const PURGE_MARKER: &str = "purge-on-uninstall";
const RELAXED_SYNC_MARKER: &str = "relaxed-sync";
const DEFAULT_WIN_SIZE: WinSize = WinSize { cols: 80, rows: 24, xpixel: 0, ypixel: 0 };
const TOP_APP_PROCS: &str = "/dev/cpuset/top-app/cgroup.procs";

fn app_in_foreground(allowed_uids_path: &Path) -> bool {
    let Ok(allowed) = fs::read_to_string(allowed_uids_path) else {
        return false;
    };
    let uids = parse_allowed_uids(&allowed);
    let Ok(procs) = fs::read_to_string(TOP_APP_PROCS) else {
        return false;
    };
    procs
        .lines()
        .filter_map(|line| line.trim().parse::<u32>().ok())
        .any(|pid| fs::metadata(format!("/proc/{pid}")).is_ok_and(|meta| uids.contains(&meta.uid())))
}

pub fn run(config: DaemonConfig) -> io::Result<()> {
    enter_module_domain()?;
    let peers = PeerPolicy::new()?;
    log_event(&format!(
        "bareboot daemon started (protocol {PROTOCOL_VERSION}, kernel pid namespaces {})",
        Path::new("/proc/self/ns/pid").exists()
    ));

    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
        libc::signal(
            libc::SIGTERM,
            handle_shutdown_signal as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGINT,
            handle_shutdown_signal as *const () as libc::sighandler_t,
        );
    }

    fs::create_dir_all(&config.state_dir)?;
    let sock_str = config.socket_path.to_string_lossy();
    let is_abstract = sock_str.starts_with('@');
    let listener = if let Some(abs_name) = sock_str.strip_prefix('@') {
        let addr = SocketAddr::from_abstract_name(abs_name.as_bytes())?;
        UnixListener::bind_addr(&addr)?
    } else {
        bind_filesystem_socket(&config.socket_path)?
    };
    listener.set_nonblocking(true)?;

    let mut state = DaemonState {
        config,
        kernel: probe_kernel(),
        distros: HashMap::new(),
        sessions: HashMap::new(),
        next_session_id: 1,
        last_dns: Vec::new(),
        last_dns_check: Instant::now(),
        timezone: android_timezone(),
        wake_locked: false,
        boosted: false,
        boost_min: foreground_util_min(),
        last_boost_check: Instant::now(),
        busy: Arc::new(Mutex::new(HashMap::new())),
    };

    let lfd = listener.as_raw_fd();
    let mut clients: Vec<Client> = Vec::new();
    while !SHUTDOWN.load(Ordering::SeqCst) {
        let mut pfds = Vec::with_capacity(1 + clients.len());
        pfds.push(libc::pollfd {
            fd: lfd,
            events: libc::POLLIN,
            revents: 0,
        });
        for c in &clients {
            pfds.push(libc::pollfd {
                fd: c.stream.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            });
        }

        let client_end = pfds.len();
        let mut detached = Vec::new();
        for (&id, session) in &state.sessions {
            if session.tab.as_ref().is_some_and(|tab| tab.client_fd.is_none() && !tab.pty_eof) {
                detached.push(id);
                pfds.push(libc::pollfd {
                    fd: session.master_fd.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                });
            }
        }

        let prc = unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, 500) };
        if prc < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        state.prune_dead();
        state.maybe_sync_dns();
        state.update_foreground_boost();
        if prc == 0 {
            continue;
        }

        if (pfds[0].revents & libc::POLLIN) != 0 {
            loop {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        let fd = stream.as_raw_fd();
                        let allowed_path = state.allowed_uids_path();
                        if let Some(uid) = peers.authorized_uid(fd, &allowed_path) {
                            if clients.len() < 128 {
                                clients.push(Client { stream, uid });
                            }
                        } else {
                            if let Ok(bytes) = encode_response(&err_resp(
                                libc::EACCES as u32,
                                "unauthorized peer".to_string(),
                            )) {
                                let _ = send_frame(fd, &bytes, None);
                            }
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(_) => break,
                }
            }
        }

        for (id, pfd) in detached.into_iter().zip(&pfds[client_end..]) {
            if pfd.revents != 0 {
                state.capture_output(id);
            }
        }
        let mut dead_indices = Vec::new();
        for (idx, pfd) in pfds[1..client_end].iter().enumerate() {
            if (pfd.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR)) == 0 {
                continue;
            }
            match handle_client_frame(&mut state, &clients[idx]) {
                ClientOutcome::Keep => {}
                ClientOutcome::Drop => {
                    state.detach_client(pfd.fd);
                    dead_indices.push(idx);
                }
                ClientOutcome::Handoff => dead_indices.push(idx),
            }
        }
        for idx in dead_indices.into_iter().rev() {
            clients.swap_remove(idx);
        }
    }

    clients.clear();
    state.shutdown_all();
    if !is_abstract {
        let _ = fs::remove_file(&state.config.socket_path);
    }
    Ok(())
}

fn handle_client_frame(state: &mut DaemonState, client: &Client) -> ClientOutcome {
    let fd = client.stream.as_raw_fd();
    let (payload, _) = match recv_frame(fd) {
        Ok(f) => f,
        Err(_) => return ClientOutcome::Drop,
    };
    let req = decode_request(&payload);
    if let Ok(request) = &req {
        state.prune_dead();
        match state.prepare_job(request) {
            Some(Ok(job)) => {
                let ctx = state.job_context();
                let label = job.label();
                let busy = Arc::clone(&state.busy);
                let name = job.name().to_string();
                let stream = match client.stream.try_clone() {
                    Ok(stream) => stream,
                    Err(e) => {
                        if let Ok(mut b) = busy.lock() {
                            b.remove(&name);
                        }
                        return reply(fd, &err_resp(libc::EIO as u32, e.to_string()), None);
                    }
                };
                let spawned = std::thread::Builder::new()
                    .name(format!("{label}-{name}"))
                    .spawn(move || {
                        let resp = run_job(job, ctx);
                        if let Ok(bytes) = encode_response(&resp) {
                            let _ = send_frame(stream.as_raw_fd(), &bytes, None);
                        }
                    });
                if let Err(e) = spawned {
                    if let Ok(mut b) = busy.lock() {
                        b.remove(&name);
                    }
                    return reply(fd, &err_resp(libc::EIO as u32, e.to_string()), None);
                }
                return ClientOutcome::Handoff;
            }
            Some(Err(resp)) => return reply(fd, &resp, None),
            None => {}
        }
    }
    let attach_id = match &req {
        Ok(Request::AttachTab { session_id, .. }) => Some(*session_id),
        _ => None,
    };
    let (resp, out_fd) = match req {
        Ok(req) => state.handle_request(req, client),
        Err(e) => (err_resp(libc::EINVAL as u32, e.to_string()), None),
    };
    let outcome = reply(fd, &resp, out_fd.as_ref().map(|f| f.as_raw_fd()));
    if matches!(outcome, ClientOutcome::Keep)
        && matches!(resp, Response::TabAttached(_))
        && let Some(id) = attach_id
        && let Some(tab) = state.sessions.get_mut(&id).and_then(|s| s.tab.as_mut())
    {
        tab.output.clear();
    }
    outcome
}

fn reply(fd: RawFd, resp: &Response, out_fd: Option<RawFd>) -> ClientOutcome {
    let Ok(bytes) = encode_response(resp) else {
        return ClientOutcome::Drop;
    };
    if send_frame(fd, &bytes, out_fd).is_err() {
        return ClientOutcome::Drop;
    }
    ClientOutcome::Keep
}
