use std::io;

pub const MAX_FRAME_SIZE: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WinSize {
    pub cols: u16,
    pub rows: u16,
    pub xpixel: u16,
    pub ypixel: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelInfo {
    pub page_size: u32,
    pub selinux_enforcing: i8,
    pub has_mnt_ns: bool,
    pub has_uts_ns: bool,
    pub has_pid_ns: bool,
    pub has_ipc_ns: bool,
    pub has_net_ns: bool,
    pub has_user_ns: bool,
    pub has_cgroup_ns: bool,
    pub isolated_network: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckState {
    Missing,
    Present,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelCheck {
    pub id: String,
    pub state: CheckState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DistroStatus {
    pub name: String,
    pub image_path: String,
    pub holder_pid: i32,
    pub init_pid: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    pub session_id: u32,
    pub distro: String,
    pub pid: i32,
    pub uid: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PortForward {
    pub host_port: u16,
    pub guest_port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageInfo {
    pub name: String,
    pub image_path: String,
    pub size_bytes: u64,
    pub used_bytes: u64,
    pub os_label: String,
    pub username: String,
    pub fs_size_bytes: u64,
    pub fs_free_bytes: u64,
    pub keep_awake: bool,
    pub init: String,
    pub isolated_net: bool,
    pub ssh_port: u16,
    pub port_forwards: Vec<PortForward>,
    pub busy: String,
    pub gpu_enabled: bool,
    pub gpu_available: bool,
    pub gpu_running: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Ping,
    Status,
    StartDistro {
        name: String,
        image_path: String,
        shared_dir: String,
    },
    StopDistro {
        name: String,
    },
    OpenSession {
        distro: String,
        uid: u32,
        gid: u32,
        size: WinSize,
        argv: Vec<String>,
        env: Vec<String>,
    },
    ResizePty {
        session_id: u32,
        size: WinSize,
    },
    KillSession {
        session_id: u32,
        signal: i32,
    },
    ListSessions,
    CreateTab {
        distro: String,
        uid: u32,
        argv: Vec<String>,
        env: Vec<String>,
    },
    AttachTab {
        session_id: u32,
        size: WinSize,
    },
    ListTabs,
    ListImages,
    CreateDistro {
        name: String,
        size_gb: u32,
        tarball_path: String,
        username: String,
        os_label: String,
    },
    ResizeDistro {
        name: String,
        size_gb: u32,
    },
    KernelReport,
    DeleteDistro {
        name: String,
    },
    ConfigureImage {
        name: String,
        os_label: String,
        username: String,
        keep_awake: bool,
        init: String,
        isolated_net: bool,
        ssh_port: u16,
        port_forwards: Vec<PortForward>,
    },
    RenameImage {
        name: String,
        new_name: String,
    },
    ExportImage {
        name: String,
        dest_path: String,
    },
    SetDaemonOptions {
        purge_on_uninstall: bool,
        relaxed_sync: bool,
    },
    ReadLog {
        distro: String,
        name: String,
    },
    AttachDesktop { name: String },
    StartScreen {
        name: String,
        display_id: u32,
    },
    ConfigureGpu {
        name: String,
        enabled: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonOptions {
    pub purge_on_uninstall: bool,
    pub relaxed_sync: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    Ok,
    Pong {
        version: u32,
        daemon_pid: i32,
        selinux_enforcing: i8,
    },
    Status {
        kernel: KernelInfo,
        distros: Vec<DistroStatus>,
        sessions: Vec<SessionInfo>,
        options: DaemonOptions,
    },
    Log(Vec<u8>),
    SessionOpened {
        session_id: u32,
        pid: i32,
    },
    Sessions(Vec<SessionInfo>),
    TabAttached(Vec<u8>),
    Images(Vec<ImageInfo>),
    KernelReport {
        release: String,
        checks: Vec<KernelCheck>,
    },
    Error {
        code: u32,
        message: String,
    },
}

struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    fn new() -> Self {
        Self {
            buf: Vec::with_capacity(128),
        }
    }

    fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    fn i8(&mut self, v: i8) {
        self.buf.push(v as u8);
    }

    fn bool(&mut self, v: bool) {
        self.buf.push(u8::from(v));
    }

    fn win_size(&mut self, size: WinSize) {
        self.u16(size.cols);
        self.u16(size.rows);
        self.u16(size.xpixel);
        self.u16(size.ypixel);
    }

    fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn i32(&mut self, v: i32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn str(&mut self, s: &str) -> io::Result<()> {
        let len = u32::try_from(s.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "string too long"))?;
        self.u32(len);
        self.buf.extend_from_slice(s.as_bytes());
        Ok(())
    }

    fn str_vec(&mut self, items: &[String]) -> io::Result<()> {
        let len = u32::try_from(items.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "vec too long"))?;
        self.u32(len);
        for item in items {
            self.str(item)?;
        }
        Ok(())
    }

    fn port_forwards(&mut self, items: &[PortForward]) -> io::Result<()> {
        let len = u32::try_from(items.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "too many port forwards"))?;
        self.u32(len);
        for item in items {
            self.u16(item.host_port);
            self.u16(item.guest_port);
        }
        Ok(())
    }

    fn finish(self) -> io::Result<Vec<u8>> {
        if self.buf.len() > MAX_FRAME_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "encoded frame exceeds max size",
            ));
        }
        Ok(self.buf)
    }
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "offset overflow"))?;
        if end > self.data.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "truncated frame",
            ));
        }
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn i8(&mut self) -> io::Result<i8> {
        Ok(self.u8()? as i8)
    }

    fn bool(&mut self) -> io::Result<bool> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(io::Error::new(io::ErrorKind::InvalidData, "invalid bool")),
        }
    }

    fn win_size(&mut self) -> io::Result<WinSize> {
        Ok(WinSize {
            cols: self.u16()?,
            rows: self.u16()?,
            xpixel: self.u16()?,
            ypixel: self.u16()?,
        })
    }

    fn u16(&mut self) -> io::Result<u16> {
        let b: [u8; 2] = self.take(2)?.try_into().unwrap();
        Ok(u16::from_le_bytes(b))
    }

    fn u32(&mut self) -> io::Result<u32> {
        let b: [u8; 4] = self.take(4)?.try_into().unwrap();
        Ok(u32::from_le_bytes(b))
    }

    fn u64(&mut self) -> io::Result<u64> {
        let b: [u8; 8] = self.take(8)?.try_into().unwrap();
        Ok(u64::from_le_bytes(b))
    }

    fn i32(&mut self) -> io::Result<i32> {
        let b: [u8; 4] = self.take(4)?.try_into().unwrap();
        Ok(i32::from_le_bytes(b))
    }

    fn str(&mut self) -> io::Result<String> {
        let len = self.u32()? as usize;
        if len > MAX_FRAME_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "string exceeds max frame size",
            ));
        }
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid utf8"))
    }

    fn str_vec(&mut self) -> io::Result<Vec<String>> {
        let count = self.u32()? as usize;
        if count > 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "too many string items",
            ));
        }
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            out.push(self.str()?);
        }
        Ok(out)
    }

    fn port_forwards(&mut self) -> io::Result<Vec<PortForward>> {
        let count = self.u32()? as usize;
        if count > 32 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "too many port forwards",
            ));
        }
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            out.push(PortForward {
                host_port: self.u16()?,
                guest_port: self.u16()?,
            });
        }
        Ok(out)
    }

    fn ensure_eof(&self) -> io::Result<()> {
        if self.pos != self.data.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "trailing bytes in frame",
            ));
        }
        Ok(())
    }
}

pub fn encode_request(req: &Request) -> io::Result<Vec<u8>> {
    let mut w = Writer::new();
    match req {
        Request::Ping => w.u8(1),
        Request::Status => w.u8(2),
        Request::StartDistro {
            name,
            image_path,
            shared_dir,
        } => {
            w.u8(3);
            w.str(name)?;
            w.str(image_path)?;
            w.str(shared_dir)?;
        }
        Request::StopDistro { name } => {
            w.u8(4);
            w.str(name)?;
        }
        Request::OpenSession {
            distro,
            uid,
            gid,
            size,
            argv,
            env,
        } => {
            w.u8(5);
            w.str(distro)?;
            w.u32(*uid);
            w.u32(*gid);
            w.win_size(*size);
            w.str_vec(argv)?;
            w.str_vec(env)?;
        }
        Request::ResizePty { session_id, size } => {
            w.u8(6);
            w.u32(*session_id);
            w.win_size(*size);
        }
        Request::KillSession { session_id, signal } => {
            w.u8(7);
            w.u32(*session_id);
            w.i32(*signal);
        }
        Request::ListSessions => w.u8(8),
        Request::CreateTab { distro, uid, argv, env } => {
            w.u8(12);
            w.str(distro)?;
            w.u32(*uid);
            w.str_vec(argv)?;
            w.str_vec(env)?;
        }
        Request::AttachTab { session_id, size } => {
            w.u8(13);
            w.u32(*session_id);
            w.win_size(*size);
        }
        Request::ListTabs => w.u8(14),
        Request::ListImages => w.u8(9),
        Request::CreateDistro {
            name,
            size_gb,
            tarball_path,
            username,
            os_label,
        } => {
            w.u8(10);
            w.str(name)?;
            w.u32(*size_gb);
            w.str(tarball_path)?;
            w.str(username)?;
            w.str(os_label)?;
        }
        Request::ResizeDistro { name, size_gb } => {
            w.u8(11);
            w.str(name)?;
            w.u32(*size_gb);
        }
        Request::KernelReport => w.u8(15),
        Request::DeleteDistro { name } => {
            w.u8(16);
            w.str(name)?;
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
            w.u8(17);
            w.str(name)?;
            w.str(os_label)?;
            w.str(username)?;
            w.bool(*keep_awake);
            w.str(init)?;
            w.bool(*isolated_net);
            w.u16(*ssh_port);
            w.port_forwards(port_forwards)?;
        }
        Request::RenameImage { name, new_name } => {
            w.u8(18);
            w.str(name)?;
            w.str(new_name)?;
        }
        Request::ExportImage { name, dest_path } => {
            w.u8(19);
            w.str(name)?;
            w.str(dest_path)?;
        }
        Request::SetDaemonOptions {
            purge_on_uninstall,
            relaxed_sync,
        } => {
            w.u8(20);
            w.bool(*purge_on_uninstall);
            w.bool(*relaxed_sync);
        }
        Request::AttachDesktop { name } => {
            w.u8(23);
            w.str(name)?;
        }
        Request::StartScreen { name, display_id } => {
            w.u8(24);
            w.str(name)?;
            w.u32(*display_id);
        }
        Request::ConfigureGpu { name, enabled } => {
            w.u8(22);
            w.str(name)?;
            w.bool(*enabled);
        }
        Request::ReadLog { distro, name } => {
            w.u8(21);
            w.str(distro)?;
            w.str(name)?;
        }
    }
    w.finish()
}

pub fn decode_request(data: &[u8]) -> io::Result<Request> {
    if data.len() > MAX_FRAME_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame exceeds max size",
        ));
    }
    let mut r = Reader::new(data);
    let tag = r.u8()?;
    let req = match tag {
        1 => Request::Ping,
        2 => Request::Status,
        3 => Request::StartDistro {
            name: r.str()?,
            image_path: r.str()?,
            shared_dir: r.str()?,
        },
        4 => Request::StopDistro { name: r.str()? },
        5 => Request::OpenSession {
            distro: r.str()?,
            uid: r.u32()?,
            gid: r.u32()?,
            size: r.win_size()?,
            argv: r.str_vec()?,
            env: r.str_vec()?,
        },
        6 => Request::ResizePty {
            session_id: r.u32()?,
            size: r.win_size()?,
        },
        7 => Request::KillSession {
            session_id: r.u32()?,
            signal: r.i32()?,
        },
        8 => Request::ListSessions,
        9 => Request::ListImages,
        10 => Request::CreateDistro {
            name: r.str()?,
            size_gb: r.u32()?,
            tarball_path: r.str()?,
            username: r.str()?,
            os_label: r.str()?,
        },
        11 => Request::ResizeDistro {
            name: r.str()?,
            size_gb: r.u32()?,
        },
        12 => Request::CreateTab {
            distro: r.str()?,
            uid: r.u32()?,
            argv: r.str_vec()?,
            env: r.str_vec()?,
        },
        13 => Request::AttachTab { session_id: r.u32()?, size: r.win_size()? },
        14 => Request::ListTabs,
        15 => Request::KernelReport,
        16 => Request::DeleteDistro { name: r.str()? },
        17 => Request::ConfigureImage {
            name: r.str()?,
            os_label: r.str()?,
            username: r.str()?,
            keep_awake: r.bool()?,
            init: r.str()?,
            isolated_net: r.bool()?,
            ssh_port: r.u16()?,
            port_forwards: r.port_forwards()?,
        },
        18 => Request::RenameImage {
            name: r.str()?,
            new_name: r.str()?,
        },
        19 => Request::ExportImage {
            name: r.str()?,
            dest_path: r.str()?,
        },
        20 => Request::SetDaemonOptions {
            purge_on_uninstall: r.bool()?,
            relaxed_sync: r.bool()?,
        },
        21 => Request::ReadLog {
            distro: r.str()?,
            name: r.str()?,
        },
        23 => Request::AttachDesktop { name: r.str()? },
        24 => Request::StartScreen {
            name: r.str()?,
            display_id: r.u32()?,
        },
        22 => Request::ConfigureGpu {
            name: r.str()?,
            enabled: r.bool()?,
        },
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unknown request tag",
            ));
        }
    };
    r.ensure_eof()?;
    Ok(req)
}

fn write_session(w: &mut Writer, s: &SessionInfo) -> io::Result<()> {
    w.u32(s.session_id);
    w.str(&s.distro)?;
    w.i32(s.pid);
    w.u32(s.uid);
    Ok(())
}

fn read_session(r: &mut Reader<'_>) -> io::Result<SessionInfo> {
    Ok(SessionInfo {
        session_id: r.u32()?,
        distro: r.str()?,
        pid: r.i32()?,
        uid: r.u32()?,
    })
}

pub fn encode_response(resp: &Response) -> io::Result<Vec<u8>> {
    let mut w = Writer::new();
    match resp {
        Response::Ok => w.u8(1),
        Response::Pong {
            version,
            daemon_pid,
            selinux_enforcing,
        } => {
            w.u8(2);
            w.u32(*version);
            w.i32(*daemon_pid);
            w.i8(*selinux_enforcing);
        }
        Response::Status {
            kernel,
            distros,
            sessions,
            options,
        } => {
            w.u8(3);
            w.u32(kernel.page_size);
            w.i8(kernel.selinux_enforcing);
            w.bool(kernel.has_mnt_ns);
            w.bool(kernel.has_uts_ns);
            w.bool(kernel.has_pid_ns);
            w.bool(kernel.has_ipc_ns);
            w.bool(kernel.has_net_ns);
            w.bool(kernel.has_user_ns);
            w.bool(kernel.has_cgroup_ns);
            w.bool(kernel.isolated_network);
            w.u32(u32::try_from(distros.len()).unwrap_or(0));
            for d in distros {
                w.str(&d.name)?;
                w.str(&d.image_path)?;
                w.i32(d.holder_pid);
                w.i32(d.init_pid);
            }
            w.u32(u32::try_from(sessions.len()).unwrap_or(0));
            for s in sessions {
                write_session(&mut w, s)?;
            }
            w.bool(options.purge_on_uninstall);
            w.bool(options.relaxed_sync);
        }
        Response::Log(bytes) => {
            w.u8(9);
            w.buf.extend_from_slice(bytes);
        }
        Response::SessionOpened { session_id, pid } => {
            w.u8(4);
            w.u32(*session_id);
            w.i32(*pid);
        }
        Response::Sessions(sessions) => {
            w.u8(5);
            w.u32(u32::try_from(sessions.len()).unwrap_or(0));
            for s in sessions {
                write_session(&mut w, s)?;
            }
        }
        Response::TabAttached(output) => {
            w.u8(7);
            w.buf.extend_from_slice(output);
        }
        Response::Images(images) => {
            w.u8(6);
            w.u32(u32::try_from(images.len()).unwrap_or(0));
            for img in images {
                w.str(&img.name)?;
                w.str(&img.image_path)?;
                w.u64(img.size_bytes);
                w.u64(img.used_bytes);
                w.str(&img.os_label)?;
                w.str(&img.username)?;
                w.u64(img.fs_size_bytes);
                w.u64(img.fs_free_bytes);
                w.bool(img.keep_awake);
                w.str(&img.init)?;
                w.bool(img.isolated_net);
                w.u16(img.ssh_port);
                w.port_forwards(&img.port_forwards)?;
                w.str(&img.busy)?;
                w.bool(img.gpu_enabled);
                w.bool(img.gpu_available);
                w.bool(img.gpu_running);
            }
        }
        Response::KernelReport { release, checks } => {
            w.u8(8);
            w.str(release)?;
            w.u32(u32::try_from(checks.len()).unwrap_or(0));
            for check in checks {
                w.str(&check.id)?;
                w.u8(match check.state {
                    CheckState::Missing => 0,
                    CheckState::Present => 1,
                    CheckState::Unknown => 2,
                });
            }
        }
        Response::Error { code, message } => {
            w.u8(255);
            w.u32(*code);
            w.str(message)?;
        }
    }
    w.finish()
}

pub fn decode_response(data: &[u8]) -> io::Result<Response> {
    if data.len() > MAX_FRAME_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame exceeds max size",
        ));
    }
    let mut r = Reader::new(data);
    let tag = r.u8()?;
    let resp = match tag {
        1 => Response::Ok,
        2 => Response::Pong {
            version: r.u32()?,
            daemon_pid: r.i32()?,
            selinux_enforcing: r.i8()?,
        },
        3 => {
            let kernel = KernelInfo {
                page_size: r.u32()?,
                selinux_enforcing: r.i8()?,
                has_mnt_ns: r.bool()?,
                has_uts_ns: r.bool()?,
                has_pid_ns: r.bool()?,
                has_ipc_ns: r.bool()?,
                has_net_ns: r.bool()?,
                has_user_ns: r.bool()?,
                has_cgroup_ns: r.bool()?,
                isolated_network: r.bool()?,
            };
            let d_count = r.u32()? as usize;
            if d_count > 256 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "too many distros"));
            }
            let mut distros = Vec::with_capacity(d_count);
            for _ in 0..d_count {
                distros.push(DistroStatus {
                    name: r.str()?,
                    image_path: r.str()?,
                    holder_pid: r.i32()?,
                    init_pid: r.i32()?,
                });
            }
            let s_count = r.u32()? as usize;
            if s_count > 1024 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "too many sessions"));
            }
            let mut sessions = Vec::with_capacity(s_count);
            for _ in 0..s_count {
                sessions.push(read_session(&mut r)?);
            }
            let options = DaemonOptions {
                purge_on_uninstall: r.bool()?,
                relaxed_sync: r.bool()?,
            };
            Response::Status {
                kernel,
                distros,
                sessions,
                options,
            }
        }
        9 => Response::Log(r.take(data.len() - r.pos)?.to_vec()),
        4 => Response::SessionOpened {
            session_id: r.u32()?,
            pid: r.i32()?,
        },
        5 => {
            let s_count = r.u32()? as usize;
            if s_count > 1024 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "too many sessions"));
            }
            let mut sessions = Vec::with_capacity(s_count);
            for _ in 0..s_count {
                sessions.push(read_session(&mut r)?);
            }
            Response::Sessions(sessions)
        }
        6 => {
            let i_count = r.u32()? as usize;
            if i_count > 256 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "too many images"));
            }
            let mut images = Vec::with_capacity(i_count);
            for _ in 0..i_count {
                images.push(ImageInfo {
                    name: r.str()?,
                    image_path: r.str()?,
                    size_bytes: r.u64()?,
                    used_bytes: r.u64()?,
                    os_label: r.str()?,
                    username: r.str()?,
                    fs_size_bytes: r.u64()?,
                    fs_free_bytes: r.u64()?,
                    keep_awake: r.bool()?,
                    init: r.str()?,
                    isolated_net: r.bool()?,
                    ssh_port: r.u16()?,
                    port_forwards: r.port_forwards()?,
                    busy: r.str()?,
                    gpu_enabled: r.bool()?,
                    gpu_available: r.bool()?,
                    gpu_running: r.bool()?,
                });
            }
            Response::Images(images)
        }
        7 => Response::TabAttached(r.take(data.len() - r.pos)?.to_vec()),
        8 => {
            let release = r.str()?;
            let count = r.u32()? as usize;
            if count > 256 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "too many checks"));
            }
            let mut checks = Vec::with_capacity(count);
            for _ in 0..count {
                let id = r.str()?;
                let state = match r.u8()? {
                    0 => CheckState::Missing,
                    1 => CheckState::Present,
                    2 => CheckState::Unknown,
                    _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid check state")),
                };
                checks.push(KernelCheck { id, state });
            }
            Response::KernelReport { release, checks }
        }
        255 => Response::Error {
            code: r.u32()?,
            message: r.str()?,
        },
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unknown response tag",
            ));
        }
    };
    r.ensure_eof()?;
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_requests() {
        let cases = vec![
            Request::Ping,
            Request::Status,
            Request::StartDistro {
                name: "debian".into(),
                image_path: "/data/adb/bareboot/distros/debian/rootfs.img".into(),
                shared_dir: "/data/media/0/Linux".into(),
            },
            Request::StopDistro {
                name: "debian".into(),
            },
            Request::OpenSession {
                distro: "debian".into(),
                uid: 1000,
                gid: 1000,
                size: WinSize { cols: 120, rows: 40, xpixel: 960, ypixel: 800 },
                argv: vec!["/bin/bash".into(), "-l".into()],
                env: vec!["TERM=xterm-256color".into()],
            },
            Request::ResizePty {
                session_id: 7,
                size: WinSize { cols: 80, rows: 24, xpixel: 0, ypixel: 0 },
            },
            Request::KillSession {
                session_id: 7,
                signal: 15,
            },
            Request::ListSessions,
            Request::CreateTab {
                distro: "debian".into(),
                uid: 0,
                argv: vec!["/bin/sh".into(), "-c".into(), "true".into()],
                env: vec!["BAREBOOT_SSH_PORT=2222".into()],
            },
            Request::AttachTab { session_id: 7, size: WinSize { cols: 80, rows: 24, xpixel: 640, ypixel: 480 } },
            Request::ListTabs,
            Request::ListImages,
            Request::CreateDistro {
                name: "alpine".into(),
                size_gb: 16,
                tarball_path: "/data/local/tmp/alpine.tar.xz".into(),
                username: "alpine".into(),
                os_label: "Alpine 3.24".into(),
            },
            Request::ResizeDistro {
                name: "debian".into(),
                size_gb: 32,
            },
            Request::KernelReport,
            Request::DeleteDistro { name: "alpine".into() },
            Request::ConfigureImage {
                name: "arch".into(),
                os_label: "Arch Linux".into(),
                username: "alarm".into(),
                keep_awake: true,
                init: "systemd".into(),
                isolated_net: true,
                ssh_port: 2222,
                port_forwards: vec![
                    PortForward { host_port: 8080, guest_port: 80 },
                    PortForward { host_port: 3000, guest_port: 3000 },
                ],
            },
            Request::RenameImage {
                name: "arch".into(),
                new_name: "arch-old".into(),
            },
            Request::ExportImage {
                name: "debian".into(),
                dest_path: "/data/media/0/Download/debian.tar.gz".into(),
            },
            Request::SetDaemonOptions {
                purge_on_uninstall: true,
                relaxed_sync: true,
            },
            Request::ReadLog {
                distro: "debian".into(),
                name: "sshd".into(),
            },
            Request::ConfigureGpu { name: "debian".into(), enabled: true },
            Request::AttachDesktop { name: "debian".into() },
            Request::StartScreen { name: "debian".into(), display_id: 7 },
        ];
        for req in cases {
            let bytes = encode_request(&req).unwrap();
            let decoded = decode_request(&bytes).unwrap();
            assert_eq!(req, decoded);
        }
    }

    #[test]
    fn roundtrip_responses() {
        let cases = vec![
            Response::Ok,
            Response::Pong {
                version: 2,
                daemon_pid: 4242,
                selinux_enforcing: 1,
            },
            Response::Status {
                kernel: KernelInfo {
                    page_size: 4096,
                    selinux_enforcing: 1,
                    has_mnt_ns: true,
                    has_uts_ns: true,
                    has_pid_ns: false,
                    has_ipc_ns: false,
                    has_net_ns: true,
                    has_user_ns: false,
                    has_cgroup_ns: true,
                    isolated_network: true,
                },
                distros: vec![DistroStatus {
                    name: "debian".into(),
                    image_path: "/tmp/debian.img".into(),
                    holder_pid: 1234,
                    init_pid: 1240,
                }],
                sessions: vec![SessionInfo {
                    session_id: 1,
                    distro: "debian".into(),
                    pid: 5678,
                    uid: 1000,
                }],
                options: DaemonOptions {
                    purge_on_uninstall: false,
                    relaxed_sync: true,
                },
            },
            Response::Log(b"started sshd\n".to_vec()),
            Response::TabAttached(vec![0x1b, b'[', b'm', 0xff]),
            Response::SessionOpened {
                session_id: 9,
                pid: 9999,
            },
            Response::Sessions(vec![]),
            Response::Images(vec![ImageInfo {
                name: "debian".into(),
                image_path: "/data/adb/bareboot/distros/debian/rootfs.img".into(),
                size_bytes: 16 * 1024 * 1024 * 1024,
                used_bytes: 512 * 1024 * 1024,
                os_label: "Debian 13 (trixie)".into(),
                username: "debian".into(),
                fs_size_bytes: 15 * 1024 * 1024 * 1024,
                fs_free_bytes: 14 * 1024 * 1024 * 1024,
                keep_awake: true,
                init: "none".into(),
                isolated_net: true,
                ssh_port: 2222,
                port_forwards: vec![PortForward { host_port: 8080, guest_port: 80 }],
                busy: "export".into(),
                gpu_enabled: true,
                gpu_available: true,
                gpu_running: false,
            }]),
            Response::KernelReport {
                release: "6.12.81-android16".into(),
                checks: vec![
                    KernelCheck { id: "ns_pid".into(), state: CheckState::Missing },
                    KernelCheck { id: "loop_control".into(), state: CheckState::Present },
                    KernelCheck { id: "config_SYSVIPC".into(), state: CheckState::Unknown },
                ],
            },
            Response::Error {
                code: 13,
                message: "permission denied".into(),
            },
        ];
        for resp in cases {
            let bytes = encode_response(&resp).unwrap();
            let decoded = decode_response(&bytes).unwrap();
            assert_eq!(resp, decoded);
        }
    }

    #[test]
    fn rejects_truncated_and_trailing() {
        let mut bytes = encode_request(&Request::ResizePty {
            session_id: 1,
            size: WinSize { cols: 80, rows: 24, xpixel: 0, ypixel: 0 },
        })
        .unwrap();
        assert!(decode_request(&bytes[..bytes.len() - 1]).is_err());
        bytes.push(0);
        assert!(decode_request(&bytes).is_err());
    }
}
