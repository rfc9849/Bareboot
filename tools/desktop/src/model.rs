use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    Usb,
    Wifi,
}

impl Transport {
    pub fn label(self) -> &'static str {
        match self {
            Self::Usb => "USB",
            Self::Wifi => "Wi-Fi",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub serial: String,
    pub state: String,
    pub model: String,
    pub product: String,
    pub transport: Transport,
}

impl Device {
    pub fn display_name(&self) -> String {
        if !self.model.is_empty() {
            self.model.replace('_', " ")
        } else {
            self.serial.clone()
        }
    }

    pub fn is_ready(&self) -> bool {
        self.state == "device"
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct KernelSummary {
    pub page_size: u32,
    pub selinux: i8,
    pub mnt: bool,
    pub uts: bool,
    pub pid: bool,
    pub ipc: bool,
    pub net: bool,
    pub user: bool,
    pub cgroup: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunningDistro {
    pub name: String,
    pub holder_pid: i32,
    pub init_pid: i32,
    pub image_path: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionInfo {
    pub id: u32,
    pub distro: String,
    pub pid: i32,
    pub uid: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PortForward {
    pub host_port: u16,
    pub guest_port: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DistroImage {
    pub name: String,
    pub used_mb: u64,
    pub size_gb: u64,
    pub free_mb: u64,
    pub path: String,
    pub os_label: String,
    pub username: String,
    pub keep_awake: bool,
    pub init: String,
    pub isolated_net: bool,
    pub ssh_port: u16,
    pub forwards: Vec<PortForward>,
    pub busy: String,
    pub gpu_enabled: bool,
    pub gpu_available: bool,
    pub gpu_running: bool,
}

impl DistroImage {
    pub fn display_os(&self) -> &str {
        if self.os_label.is_empty() {
            &self.name
        } else {
            &self.os_label
        }
    }

    pub fn used_ratio(&self) -> f32 {
        let total_mb = self.size_gb.saturating_mul(1024).max(1);
        (self.used_mb as f32 / total_mb as f32).clamp(0.0, 1.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesktopApp {
    pub id: String,
    pub name: String,
    pub comment: String,
    pub exec: String,
    pub icon_name: String,
    pub icon_path: Option<PathBuf>,
    pub categories: Vec<String>,
    pub terminal: bool,
    pub is_custom: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteWindow {
    pub id: u32,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisplayState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplaySession {
    pub state: DisplayState,
    pub windows: Vec<RemoteWindow>,
}

impl Default for DisplaySession {
    fn default() -> Self {
        Self {
            state: DisplayState::Disconnected,
            windows: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdbForward {
    pub serial: String,
    pub local: String,
    pub remote: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogKind {
    Daemon,
    Autostart,
    Sshd,
}

impl LogKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Daemon => "Daemon",
            Self::Autostart => "Autostart",
            Self::Sshd => "SSHD",
        }
    }

    pub fn arg(self) -> &'static str {
        match self {
            Self::Daemon => "daemon",
            Self::Autostart => "autostart",
            Self::Sshd => "sshd",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct DaemonSnapshot {
    pub kernel: KernelSummary,
    pub running: Vec<RunningDistro>,
    pub sessions: Vec<SessionInfo>,
    pub images: Vec<DistroImage>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum ConnectionStatus {
    AdbMissing,
    #[default]
    NoDevice,
    Unauthorized(String),
    DaemonUnavailable(String),
    Ready,
}

#[derive(Default)]
pub struct Model {
    pub adb_path: Option<PathBuf>,
    pub devices: Vec<Device>,
    pub active_serial: Option<String>,
    pub status: ConnectionStatus,
    pub snapshot: DaemonSnapshot,
    pub apps: HashMap<String, Vec<DesktopApp>>,
    pub scanning_apps: HashMap<String, bool>,
    pub displays: HashMap<String, DisplaySession>,
    pub forwards: Vec<AdbForward>,
    pub log_text: String,
    pub log_loading: bool,
    pub busy_distros: HashMap<String, String>,
}

impl Model {
    pub fn active_device(&self) -> Option<&Device> {
        let serial = self.active_serial.as_deref()?;
        self.devices.iter().find(|d| d.serial == serial)
    }

    pub fn is_running(&self, name: &str) -> bool {
        self.snapshot.running.iter().any(|r| r.name == name)
    }

    pub fn running_info(&self, name: &str) -> Option<&RunningDistro> {
        self.snapshot.running.iter().find(|r| r.name == name)
    }

    pub fn image(&self, name: &str) -> Option<&DistroImage> {
        self.snapshot.images.iter().find(|i| i.name == name)
    }

    pub fn display(&self, name: &str) -> DisplaySession {
        self.displays.get(name).cloned().unwrap_or_default()
    }

    pub fn total_windows(&self) -> usize {
        self.displays.values().map(|d| d.windows.len()).sum()
    }

    pub fn connected_displays(&self) -> usize {
        self.displays
            .values()
            .filter(|d| matches!(d.state, DisplayState::Connected))
            .count()
    }
}

#[derive(Clone, Debug)]
pub enum Update {
    AdbResolved(Option<PathBuf>),
    DevicesChanged {
        devices: Vec<Device>,
        active_serial: Option<String>,
    },
    Snapshot {
        status: ConnectionStatus,
        snapshot: Option<DaemonSnapshot>,
    },
    AppsLoaded {
        distro: String,
        apps: Vec<DesktopApp>,
    },
    AppsScanning {
        distro: String,
        scanning: bool,
    },
    DisplayChanged {
        distro: String,
        session: DisplaySession,
    },
    ForwardsChanged(Vec<AdbForward>),
    LogLoaded {
        text: String,
    },
    DistroBusy {
        distro: String,
        action: Option<String>,
    },
    Toast {
        message: String,
        error: bool,
    },
}
