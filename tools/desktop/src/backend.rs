use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Child;
use tokio::sync::{Mutex, Notify, mpsc};

use crate::adb;
use crate::display::HostEvent;
use crate::model::{
    ConnectionStatus, DaemonSnapshot, DisplaySession, DisplayState, DistroImage, LogKind,
    RemoteWindow, Update,
};
use crate::prefs::{Prefs, TerminalApp, cache_dir};

#[derive(Clone, Debug)]
pub enum Command {
    RefreshNow,
    SelectDevice(String),
    StartDistro {
        name: String,
        image_path: String,
    },
    StopDistro {
        name: String,
    },
    ToggleGpu {
        name: String,
        enabled: bool,
    },
    ConfigureImage(DistroImage),
    ScanApps {
        distro: String,
    },
    ConnectDisplay {
        distro: String,
    },
    DisconnectDisplay {
        distro: String,
    },
    LaunchApp {
        distro: String,
        name: String,
        exec: String,
        terminal: bool,
    },
    OpenTerminal {
        distro: String,
        via_ssh: bool,
        ssh_port: u16,
        username: String,
        guest_cmd: Option<String>,
    },
    AddForward {
        local_port: u16,
        remote_port: u16,
    },
    RemoveForward {
        local: String,
    },
    ClearForwards,
    PushFile {
        path: PathBuf,
    },
    LoadLog {
        kind: LogKind,
        distro: Option<String>,
    },
    UpdateSettings {
        adb_path: Option<String>,
        terminal: TerminalApp,
        cmd_to_ctrl: bool,
        auto_reconnect: bool,
        auto_forward: bool,
    },
    Shutdown,
}

#[derive(Clone)]
pub struct Backend {
    tx: mpsc::UnboundedSender<Command>,
}

impl Backend {
    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }
}

struct ManagedDisplay {
    child: Child,
    session: DisplaySession,
    user_stopped: bool,
    forward_port: Option<u16>,
    connected_notify: Arc<Notify>,
}

struct WorkerState {
    adb_override: Option<String>,
    adb_path: Option<PathBuf>,
    preferred_serial: Option<String>,
    active_serial: Option<String>,
    terminal: TerminalApp,
    cmd_to_ctrl: bool,
    auto_reconnect: bool,
    auto_forward: bool,
    last_snapshot: Option<DaemonSnapshot>,
    scanned_distros: HashMap<String, bool>,
    displays: HashMap<String, ManagedDisplay>,
}

pub fn spawn(prefs: &Prefs) -> (Backend, UnboundedReceiver<Update>) {
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<Command>();
    let (upd_tx, upd_rx) = unbounded::<Update>();
    let initial = WorkerState {
        adb_override: prefs.adb_path.clone(),
        adb_path: adb::resolve_adb(prefs.adb_path.as_deref()),
        preferred_serial: prefs.preferred_serial.clone(),
        active_serial: None,
        terminal: prefs.terminal.clone(),
        cmd_to_ctrl: prefs.cmd_to_ctrl,
        auto_reconnect: prefs.auto_reconnect_display,
        auto_forward: prefs.auto_forward_ports,
        last_snapshot: None,
        scanned_distros: HashMap::new(),
        displays: HashMap::new(),
    };
    let state = Arc::new(Mutex::new(initial));
    let self_tx = cmd_tx.clone();

    std::thread::Builder::new()
        .name("bareboot-backend".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(async move {
                {
                    let st = state.lock().await;
                    let _ = upd_tx.unbounded_send(Update::AdbResolved(st.adb_path.clone()));
                }
                poll_once(&state, &upd_tx, &self_tx).await;
                let mut ticker = tokio::time::interval(Duration::from_secs(2));
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    tokio::select! {
                        _ = ticker.tick() => {
                            poll_once(&state, &upd_tx, &self_tx).await;
                        }
                        maybe = cmd_rx.recv() => {
                            let Some(cmd) = maybe else { break };
                            if matches!(cmd, Command::Shutdown) {
                                stop_all_displays(&state, &upd_tx).await;
                                break;
                            }
                            handle_command(cmd, &state, &upd_tx, &self_tx).await;
                        }
                    }
                }
            });
        })
        .expect("backend thread");

    (Backend { tx: cmd_tx }, upd_rx)
}

async fn poll_once(
    state: &Arc<Mutex<WorkerState>>,
    upd: &UnboundedSender<Update>,
    self_tx: &mpsc::UnboundedSender<Command>,
) {
    let (adb_path, preferred, current_serial, auto_forward) = {
        let mut st = state.lock().await;
        if st.adb_path.is_none() {
            st.adb_path = adb::resolve_adb(st.adb_override.as_deref());
            let _ = upd.unbounded_send(Update::AdbResolved(st.adb_path.clone()));
        }
        (
            st.adb_path.clone(),
            st.preferred_serial.clone(),
            st.active_serial.clone(),
            st.auto_forward,
        )
    };
    let Some(adb_path) = adb_path else {
        let _ = upd.unbounded_send(Update::Snapshot {
            status: ConnectionStatus::AdbMissing,
            snapshot: None,
        });
        return;
    };

    let devices = adb::list_devices(&adb_path).await.unwrap_or_default();

    let chosen = current_serial
        .filter(|s| devices.iter().any(|d| &d.serial == s && d.is_ready()))
        .or_else(|| {
            preferred.filter(|s| devices.iter().any(|d| &d.serial == s && d.is_ready()))
        })
        .or_else(|| devices.iter().find(|d| d.is_ready()).map(|d| d.serial.clone()))
        .or_else(|| devices.first().map(|d| d.serial.clone()));

    {
        let mut st = state.lock().await;
        if st.active_serial != chosen {
            st.scanned_distros.clear();
        }
        st.active_serial = chosen.clone();
    }

    let _ = upd.unbounded_send(Update::DevicesChanged {
        devices: devices.clone(),
        active_serial: chosen.clone(),
    });

    let Some(serial) = chosen else {
        let _ = upd.unbounded_send(Update::Snapshot {
            status: ConnectionStatus::NoDevice,
            snapshot: None,
        });
        return;
    };

    if let Some(dev) = devices.iter().find(|d| d.serial == serial)
        && !dev.is_ready()
    {
        let _ = upd.unbounded_send(Update::Snapshot {
            status: ConnectionStatus::Unauthorized(dev.state.clone()),
            snapshot: None,
        });
        return;
    }

    let (status, snapshot) = adb::poll_snapshot(&adb_path, &serial).await;
    if let Some(snap) = &snapshot {
        let mut to_scan = Vec::new();
        {
            let mut st = state.lock().await;
            st.last_snapshot = Some(snap.clone());
            for running in &snap.running {
                if !st.scanned_distros.contains_key(&running.name) {
                    st.scanned_distros.insert(running.name.clone(), true);
                    to_scan.push(running.name.clone());
                }
            }
            let running_names: Vec<String> = snap.running.iter().map(|r| r.name.clone()).collect();
            st.scanned_distros.retain(|k, _| running_names.contains(k));
        }

        for distro in to_scan {
            let _ = self_tx.send(Command::ScanApps { distro });
        }

        if auto_forward {
            for running in &snap.running {
                if let Some(img) = snap.images.iter().find(|i| i.name == running.name) {
                    if img.ssh_port > 0 {
                        let _ = adb::add_forward(&adb_path, &serial, img.ssh_port, img.ssh_port).await;
                    }
                    for pf in &img.forwards {
                        let _ = adb::add_forward(&adb_path, &serial, pf.host_port, pf.host_port).await;
                    }
                }
            }
        }
    }

    if let Ok(fwds) = adb::list_forwards(&adb_path, &serial).await {
        let _ = upd.unbounded_send(Update::ForwardsChanged(fwds));
    }

    let _ = upd.unbounded_send(Update::Snapshot { status, snapshot });
}

async fn handle_command(
    cmd: Command,
    state: &Arc<Mutex<WorkerState>>,
    upd: &UnboundedSender<Update>,
    self_tx: &mpsc::UnboundedSender<Command>,
) {
    match cmd {
        Command::RefreshNow => {
            poll_once(state, upd, self_tx).await;
        }
        Command::SelectDevice(serial) => {
            stop_all_displays(state, upd).await;
            {
                let mut st = state.lock().await;
                st.preferred_serial = Some(serial.clone());
                st.active_serial = Some(serial);
                st.scanned_distros.clear();
            }
            poll_once(state, upd, self_tx).await;
        }
        Command::StartDistro { name, image_path } => {
            let Some((adb_path, serial)) = get_adb_and_serial(state).await else {
                return;
            };
            let upd = upd.clone();
            let self_tx = self_tx.clone();
            tokio::spawn(async move {
                let _ = upd.unbounded_send(Update::DistroBusy {
                    distro: name.clone(),
                    action: Some("Starting…".into()),
                });
                let res = adb::start_distro(&adb_path, &serial, &name, &image_path).await;
                let _ = upd.unbounded_send(Update::DistroBusy {
                    distro: name.clone(),
                    action: None,
                });
                match res {
                    Ok(()) => {
                        let _ = upd.unbounded_send(Update::Toast {
                            message: format!("Started {name}"),
                            error: false,
                        });
                        let _ = self_tx.send(Command::RefreshNow);
                    }
                    Err(e) => {
                        let _ = upd.unbounded_send(Update::Toast {
                            message: format!("Failed to start {name}: {e}"),
                            error: true,
                        });
                    }
                }
            });
        }
        Command::StopDistro { name } => {
            disconnect_display(state, upd, &name).await;
            let Some((adb_path, serial)) = get_adb_and_serial(state).await else {
                return;
            };
            let upd = upd.clone();
            let self_tx = self_tx.clone();
            tokio::spawn(async move {
                let _ = upd.unbounded_send(Update::DistroBusy {
                    distro: name.clone(),
                    action: Some("Stopping…".into()),
                });
                let res = adb::stop_distro(&adb_path, &serial, &name).await;
                let _ = upd.unbounded_send(Update::DistroBusy {
                    distro: name.clone(),
                    action: None,
                });
                match res {
                    Ok(()) => {
                        let _ = upd.unbounded_send(Update::Toast {
                            message: format!("Stopped {name}"),
                            error: false,
                        });
                        let _ = self_tx.send(Command::RefreshNow);
                    }
                    Err(e) => {
                        let _ = upd.unbounded_send(Update::Toast {
                            message: format!("Failed to stop {name}: {e}"),
                            error: true,
                        });
                    }
                }
            });
        }
        Command::ToggleGpu { name, enabled } => {
            let Some((adb_path, serial)) = get_adb_and_serial(state).await else {
                return;
            };
            let upd = upd.clone();
            let self_tx = self_tx.clone();
            tokio::spawn(async move {
                match adb::configure_gpu(&adb_path, &serial, &name, enabled).await {
                    Ok(()) => {
                        let _ = upd.unbounded_send(Update::Toast {
                            message: format!(
                                "GPU {} for {name}",
                                if enabled { "enabled" } else { "disabled" }
                            ),
                            error: false,
                        });
                        let _ = self_tx.send(Command::RefreshNow);
                    }
                    Err(e) => {
                        let _ = upd.unbounded_send(Update::Toast {
                            message: format!("GPU setting failed: {e}"),
                            error: true,
                        });
                    }
                }
            });
        }
        Command::ConfigureImage(img) => {
            let Some((adb_path, serial)) = get_adb_and_serial(state).await else {
                return;
            };
            let upd = upd.clone();
            let self_tx = self_tx.clone();
            tokio::spawn(async move {
                match adb::configure_image(&adb_path, &serial, &img).await {
                    Ok(()) => {
                        let _ = upd.unbounded_send(Update::Toast {
                            message: format!("Saved settings for {}", img.name),
                            error: false,
                        });
                        let _ = self_tx.send(Command::RefreshNow);
                    }
                    Err(e) => {
                        let _ = upd.unbounded_send(Update::Toast {
                            message: format!("Failed to configure {}: {e}", img.name),
                            error: true,
                        });
                    }
                }
            });
        }
        Command::ScanApps { distro } => {
            let Some((adb_path, serial)) = get_adb_and_serial(state).await else {
                return;
            };
            let upd = upd.clone();
            tokio::spawn(async move {
                let _ = upd.unbounded_send(Update::AppsScanning {
                    distro: distro.clone(),
                    scanning: true,
                });
                let icon_dir = cache_dir().join("icons");
                match adb::scan_desktop_apps(&adb_path, &serial, &distro, &icon_dir).await {
                    Ok(apps) => {
                        let _ = upd.unbounded_send(Update::AppsLoaded {
                            distro: distro.clone(),
                            apps,
                        });
                    }
                    Err(e) => {
                        let _ = upd.unbounded_send(Update::Toast {
                            message: format!("Could not scan apps in {distro}: {e}"),
                            error: true,
                        });
                    }
                }
                let _ = upd.unbounded_send(Update::AppsScanning {
                    distro,
                    scanning: false,
                });
            });
        }
        Command::ConnectDisplay { distro } => {
            let _ = ensure_display_connected(state, upd, self_tx, &distro).await;
        }
        Command::DisconnectDisplay { distro } => {
            disconnect_display(state, upd, &distro).await;
        }
        Command::LaunchApp {
            distro,
            name,
            exec,
            terminal,
        } => {
            let state = state.clone();
            let upd = upd.clone();
            let self_tx = self_tx.clone();
            tokio::spawn(async move {
                let Some((adb_path, serial)) = get_adb_and_serial(&state).await else {
                    let _ = upd.unbounded_send(Update::Toast {
                        message: "No device connected".into(),
                        error: true,
                    });
                    return;
                };
                let (is_running, image_path, term_app, ssh_port, username) = {
                    let st = state.lock().await;
                    let running = st
                        .last_snapshot
                        .as_ref()
                        .is_some_and(|s| s.running.iter().any(|r| r.name == distro));
                    let img = st
                        .last_snapshot
                        .as_ref()
                        .and_then(|s| s.images.iter().find(|i| i.name == distro).cloned());
                    (
                        running,
                        img.as_ref().map(|i| i.path.clone()),
                        st.terminal.clone(),
                        img.as_ref().map_or(0, |i| i.ssh_port),
                        img.as_ref().map(|i| i.username.clone()).unwrap_or_default(),
                    )
                };

                if !is_running {
                    let Some(path) = image_path else {
                        let _ = upd.unbounded_send(Update::Toast {
                            message: format!("Distro {distro} not found"),
                            error: true,
                        });
                        return;
                    };
                    let _ = upd.unbounded_send(Update::DistroBusy {
                        distro: distro.clone(),
                        action: Some("Starting…".into()),
                    });
                    let started = adb::start_distro(&adb_path, &serial, &distro, &path).await;
                    let _ = upd.unbounded_send(Update::DistroBusy {
                        distro: distro.clone(),
                        action: None,
                    });
                    if let Err(e) = started {
                        let _ = upd.unbounded_send(Update::Toast {
                            message: format!("Failed to start {distro}: {e}"),
                            error: true,
                        });
                        return;
                    }
                    let _ = self_tx.send(Command::RefreshNow);
                    tokio::time::sleep(Duration::from_millis(350)).await;
                }

                if terminal {
                    match adb::open_external_terminal(&adb::TerminalLaunch {
                        terminal: &term_app,
                        adb: &adb_path,
                        serial: &serial,
                        distro: &distro,
                        via_ssh: false,
                        ssh_port,
                        username: &username,
                        guest_cmd: Some(&exec),
                    }) {
                        Ok(()) => {
                            let _ = upd.unbounded_send(Update::Toast {
                                message: format!("Opened {name} in terminal"),
                                error: false,
                            });
                        }
                        Err(e) => {
                            let _ = upd.unbounded_send(Update::Toast {
                                message: format!("Could not open terminal: {e}"),
                                error: true,
                            });
                        }
                    }
                    return;
                }

                if let Err(e) = ensure_display_connected(&state, &upd, &self_tx, &distro).await {
                    let _ = upd.unbounded_send(Update::Toast {
                        message: format!("Display failed for {distro}: {e}"),
                        error: true,
                    });
                    return;
                }

                match adb::launch_guest_app(&adb_path, &serial, &distro, &exec).await {
                    Ok(()) => {
                        let _ = upd.unbounded_send(Update::Toast {
                            message: format!("Launched {name}"),
                            error: false,
                        });
                    }
                    Err(e) => {
                        let _ = upd.unbounded_send(Update::Toast {
                            message: format!("Could not launch {name}: {e}"),
                            error: true,
                        });
                    }
                }
            });
        }
        Command::OpenTerminal {
            distro,
            via_ssh,
            ssh_port,
            username,
            guest_cmd,
        } => {
            let state = state.clone();
            let upd = upd.clone();
            tokio::spawn(async move {
                let Some((adb_path, serial)) = get_adb_and_serial(&state).await else {
                    return;
                };
                let term = state.lock().await.terminal.clone();
                if via_ssh && ssh_port > 0 {
                    let _ = adb::add_forward(&adb_path, &serial, ssh_port, ssh_port).await;
                }
                match adb::open_external_terminal(&adb::TerminalLaunch {
                    terminal: &term,
                    adb: &adb_path,
                    serial: &serial,
                    distro: &distro,
                    via_ssh,
                    ssh_port,
                    username: &username,
                    guest_cmd: guest_cmd.as_deref(),
                }) {
                    Ok(()) => {
                        let _ = upd.unbounded_send(Update::Toast {
                            message: format!("Opened terminal for {distro}"),
                            error: false,
                        });
                    }
                    Err(e) => {
                        let _ = upd.unbounded_send(Update::Toast {
                            message: format!("Could not open terminal: {e}"),
                            error: true,
                        });
                    }
                }
            });
        }
        Command::AddForward {
            local_port,
            remote_port,
        } => {
            let Some((adb_path, serial)) = get_adb_and_serial(state).await else {
                return;
            };
            let upd = upd.clone();
            tokio::spawn(async move {
                match adb::add_forward(&adb_path, &serial, local_port, remote_port).await {
                    Ok(()) => {
                        if let Ok(fwds) = adb::list_forwards(&adb_path, &serial).await {
                            let _ = upd.unbounded_send(Update::ForwardsChanged(fwds));
                        }
                        let _ = upd.unbounded_send(Update::Toast {
                            message: format!("Forwarded 127.0.0.1:{local_port} -> :{remote_port}"),
                            error: false,
                        });
                    }
                    Err(e) => {
                        let _ = upd.unbounded_send(Update::Toast {
                            message: format!("Port forward failed: {e}"),
                            error: true,
                        });
                    }
                }
            });
        }
        Command::RemoveForward { local } => {
            let Some((adb_path, serial)) = get_adb_and_serial(state).await else {
                return;
            };
            let upd = upd.clone();
            tokio::spawn(async move {
                match adb::remove_forward(&adb_path, &serial, &local).await {
                    Ok(()) => {
                        if let Ok(fwds) = adb::list_forwards(&adb_path, &serial).await {
                            let _ = upd.unbounded_send(Update::ForwardsChanged(fwds));
                        }
                    }
                    Err(e) => {
                        let _ = upd.unbounded_send(Update::Toast {
                            message: format!("Remove forward failed: {e}"),
                            error: true,
                        });
                    }
                }
            });
        }
        Command::ClearForwards => {
            let Some((adb_path, serial)) = get_adb_and_serial(state).await else {
                return;
            };
            let upd = upd.clone();
            let self_tx = self_tx.clone();
            tokio::spawn(async move {
                match adb::remove_all_forwards(&adb_path, &serial).await {
                    Ok(()) => {
                        let _ = upd.unbounded_send(Update::ForwardsChanged(Vec::new()));
                        let _ = upd.unbounded_send(Update::Toast {
                            message: "Cleared ADB port forwards".into(),
                            error: false,
                        });
                        let _ = self_tx.send(Command::RefreshNow);
                    }
                    Err(e) => {
                        let _ = upd.unbounded_send(Update::Toast {
                            message: format!("Clear forwards failed: {e}"),
                            error: true,
                        });
                    }
                }
            });
        }
        Command::PushFile { path } => {
            let Some((adb_path, serial)) = get_adb_and_serial(state).await else {
                return;
            };
            let upd = upd.clone();
            tokio::spawn(async move {
                match adb::push_to_shared(&adb_path, &serial, &path).await {
                    Ok(guest_path) => {
                        let _ = upd.unbounded_send(Update::Toast {
                            message: format!("Copied to {guest_path}"),
                            error: false,
                        });
                    }
                    Err(e) => {
                        let _ = upd.unbounded_send(Update::Toast {
                            message: format!("File copy failed: {e}"),
                            error: true,
                        });
                    }
                }
            });
        }
        Command::LoadLog { kind, distro } => {
            let Some((adb_path, serial)) = get_adb_and_serial(state).await else {
                return;
            };
            let upd = upd.clone();
            tokio::spawn(async move {
                let text = match adb::read_log(&adb_path, &serial, kind, distro.as_deref()).await {
                    Ok(t) if !t.trim().is_empty() => t,
                    Ok(_) => "(log is empty)".into(),
                    Err(e) => format!("Error reading log: {e}"),
                };
                let _ = upd.unbounded_send(Update::LogLoaded { text });
            });
        }
        Command::UpdateSettings {
            adb_path,
            terminal,
            cmd_to_ctrl,
            auto_reconnect,
            auto_forward,
        } => {
            let resolved = adb::resolve_adb(adb_path.as_deref());
            {
                let mut st = state.lock().await;
                st.adb_override = adb_path;
                st.adb_path = resolved.clone();
                st.terminal = terminal;
                st.cmd_to_ctrl = cmd_to_ctrl;
                st.auto_reconnect = auto_reconnect;
                st.auto_forward = auto_forward;
            }
            let _ = upd.unbounded_send(Update::AdbResolved(resolved));
            poll_once(state, upd, self_tx).await;
        }
        Command::Shutdown => {}
    }
}

async fn get_adb_and_serial(state: &Arc<Mutex<WorkerState>>) -> Option<(PathBuf, String)> {
    let st = state.lock().await;
    Some((st.adb_path.clone()?, st.active_serial.clone()?))
}

async fn ensure_display_connected(
    state: &Arc<Mutex<WorkerState>>,
    upd: &UnboundedSender<Update>,
    self_tx: &mpsc::UnboundedSender<Command>,
    distro: &str,
) -> std::io::Result<()> {
    let wait_notify = {
        let st = state.lock().await;
        if let Some(existing) = st.displays.get(distro) {
            match existing.session.state {
                DisplayState::Connected => return Ok(()),
                DisplayState::Connecting | DisplayState::Reconnecting => {
                    Some(existing.connected_notify.clone())
                }
                _ => None,
            }
        } else {
            None
        }
    };
    if let Some(notify) = wait_notify {
        let _ = tokio::time::timeout(Duration::from_secs(10), notify.notified()).await;
        let st = state.lock().await;
        if st
            .displays
            .get(distro)
            .is_some_and(|d| matches!(d.session.state, DisplayState::Connected))
        {
            return Ok(());
        }
        return Err(std::io::Error::other("display did not connect in time"));
    }

    let (adb_path, serial, cmd_to_ctrl) = {
        let st = state.lock().await;
        let adb_path = st
            .adb_path
            .clone()
            .ok_or_else(|| std::io::Error::other("adb not found"))?;
        let serial = st
            .active_serial
            .clone()
            .ok_or_else(|| std::io::Error::other("no device connected"))?;
        (adb_path, serial, st.cmd_to_ctrl)
    };

    let exe = std::env::current_exe()?;
    let mut cmd = tokio::process::Command::new(exe);
    #[cfg(windows)]
    {
        cmd.creation_flags(0x0800_0000);
    }
    cmd.arg("--display-host")
        .arg(distro)
        .arg("--serial")
        .arg(&serial)
        .arg("--adb")
        .arg(&adb_path)
        .arg("--events");
    if cmd_to_ctrl {
        cmd.arg("--cmd-to-ctrl");
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take().unwrap();
    let notify = Arc::new(Notify::new());

    let session = DisplaySession {
        state: DisplayState::Connecting,
        windows: Vec::new(),
    };
    {
        let mut st = state.lock().await;
        if let Some(mut old) = st.displays.remove(distro) {
            old.user_stopped = true;
            let _ = old.child.kill().await;
        }
        st.displays.insert(
            distro.to_owned(),
            ManagedDisplay {
                child,
                session: session.clone(),
                user_stopped: false,
                forward_port: None,
                connected_notify: notify.clone(),
            },
        );
    }
    let _ = upd.unbounded_send(Update::DisplayChanged {
        distro: distro.to_owned(),
        session,
    });

    let state_bg = state.clone();
    let upd_bg = upd.clone();
    let self_tx_bg = self_tx.clone();
    let distro_bg = distro.to_owned();
    let notify_bg = notify.clone();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        let mut last_reason = String::new();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(ev) = serde_json::from_str::<HostEvent>(&line) else {
                continue;
            };
            let mut st = state_bg.lock().await;
            let Some(entry) = st.displays.get_mut(&distro_bg) else {
                break;
            };
            match ev {
                HostEvent::Connected { forward_port } => {
                    entry.forward_port = Some(forward_port);
                    entry.session.state = DisplayState::Connected;
                    notify_bg.notify_waiters();
                }
                HostEvent::WindowOpen { id } => {
                    if !entry.session.windows.iter().any(|w| w.id == id) {
                        entry.session.windows.push(RemoteWindow {
                            id,
                            title: "Bareboot".into(),
                        });
                    }
                }
                HostEvent::WindowTitle { id, title } => {
                    if let Some(w) = entry.session.windows.iter_mut().find(|w| w.id == id) {
                        w.title = title;
                    }
                }
                HostEvent::WindowClose { id } => {
                    entry.session.windows.retain(|w| w.id != id);
                }
                HostEvent::Disconnected { reason } => {
                    last_reason = reason;
                }
            }
            let _ = upd_bg.unbounded_send(Update::DisplayChanged {
                distro: distro_bg.clone(),
                session: entry.session.clone(),
            });
        }

        let (should_reconnect, cleanup_fwd) = {
            let mut st = state_bg.lock().await;
            let auto = st.auto_reconnect;
            let adb_pair = st.adb_path.clone().zip(st.active_serial.clone());
            let still_running = st
                .last_snapshot
                .as_ref()
                .is_some_and(|s| s.running.iter().any(|r| r.name == distro_bg));
            if let Some(mut entry) = st.displays.remove(&distro_bg) {
                let _ = entry.child.wait().await;
                let fwd = entry.forward_port.zip(adb_pair);
                let stopped = entry.user_stopped;
                let reconnect = if stopped {
                    let _ = upd_bg.unbounded_send(Update::DisplayChanged {
                        distro: distro_bg.clone(),
                        session: DisplaySession::default(),
                    });
                    false
                } else if auto && still_running {
                    let _ = upd_bg.unbounded_send(Update::DisplayChanged {
                        distro: distro_bg.clone(),
                        session: DisplaySession {
                            state: DisplayState::Reconnecting,
                            windows: Vec::new(),
                        },
                    });
                    true
                } else {
                    let _ = upd_bg.unbounded_send(Update::DisplayChanged {
                        distro: distro_bg.clone(),
                        session: DisplaySession {
                            state: if last_reason.is_empty() {
                                DisplayState::Disconnected
                            } else {
                                DisplayState::Error(last_reason)
                            },
                            windows: Vec::new(),
                        },
                    });
                    false
                };
                (reconnect, fwd)
            } else {
                (false, None)
            }
        };
        if let Some((port, (adb_path, serial))) = cleanup_fwd {
            let _ = adb::remove_forward(&adb_path, &serial, &format!("tcp:{port}")).await;
        }
        notify_bg.notify_waiters();

        if should_reconnect {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let _ = self_tx_bg.send(Command::ConnectDisplay { distro: distro_bg });
        }
    });

    let _ = tokio::time::timeout(Duration::from_secs(10), notify.notified()).await;
    let st = state.lock().await;
    if st
        .displays
        .get(distro)
        .is_some_and(|d| matches!(d.session.state, DisplayState::Connected))
    {
        Ok(())
    } else {
        Err(std::io::Error::other("display connection failed"))
    }
}

async fn disconnect_display(
    state: &Arc<Mutex<WorkerState>>,
    upd: &UnboundedSender<Update>,
    distro: &str,
) {
    let (mut entry, adb_pair) = {
        let mut st = state.lock().await;
        if let Some(e) = st.displays.get_mut(distro) {
            e.user_stopped = true;
        }
        let pair = st.adb_path.clone().zip(st.active_serial.clone());
        (st.displays.remove(distro), pair)
    };
    if let Some(ref mut e) = entry {
        let _ = e.child.kill().await;
        let _ = e.child.wait().await;
        if let (Some(port), Some((adb_path, serial))) = (e.forward_port, adb_pair) {
            let _ = adb::remove_forward(&adb_path, &serial, &format!("tcp:{port}")).await;
        }
    }
    let _ = upd.unbounded_send(Update::DisplayChanged {
        distro: distro.to_owned(),
        session: DisplaySession::default(),
    });
}

async fn stop_all_displays(state: &Arc<Mutex<WorkerState>>, upd: &UnboundedSender<Update>) {
    let names: Vec<String> = {
        let st = state.lock().await;
        st.displays.keys().cloned().collect()
    };
    for name in names {
        disconnect_display(state, upd, &name).await;
    }
}
