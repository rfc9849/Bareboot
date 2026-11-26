use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use tokio::process::Command;

use crate::display::valid_distro_name;
use crate::model::{
    AdbForward, ConnectionStatus, DaemonSnapshot, DesktopApp, Device, DistroImage, KernelSummary,
    LogKind, PortForward, RunningDistro, SessionInfo, Transport,
};
use crate::prefs::TerminalApp;

const BAREBOOT_BIN: &str = "/data/adb/modules/bareboot/bin/bareboot";
const SHARED_DIR: &str = "/data/media/0/Linux";
const MARKER_IMAGES: &str = "___BAREBOOT_IMAGES___";

pub fn resolve_adb(override_path: Option<&str>) -> Option<PathBuf> {
    if let Some(custom) = override_path.filter(|s| !s.trim().is_empty()) {
        let p = PathBuf::from(custom.trim());
        if p.is_file() {
            return Some(p);
        }
    }
    if let Some(found) = find_in_path(adb_binary_name()) {
        return Some(found);
    }
    let home = dirs::home_dir();
    let mut candidates: Vec<PathBuf> = Vec::new();
    #[cfg(target_os = "macos")]
    {
        if let Some(h) = &home {
            candidates.push(h.join("Library/Android/sdk/platform-tools/adb"));
        }
        candidates.push(PathBuf::from("/opt/homebrew/bin/adb"));
        candidates.push(PathBuf::from("/opt/zerobrew/bin/adb"));
        candidates.push(PathBuf::from("/usr/local/bin/adb"));
        candidates.push(PathBuf::from("/opt/local/bin/adb"));
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(h) = &home {
            candidates.push(h.join("Android/Sdk/platform-tools/adb"));
            candidates.push(h.join(".local/bin/adb"));
        }
        candidates.push(PathBuf::from("/usr/bin/adb"));
        candidates.push(PathBuf::from("/usr/local/bin/adb"));
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
            candidates.push(local.join(r"Android\Sdk\platform-tools\adb.exe"));
        }
        if let Some(h) = &home {
            candidates.push(h.join(r"scoop\shims\adb.exe"));
        }
    }
    candidates.into_iter().find(|p| p.is_file())
}

fn adb_binary_name() -> &'static str {
    #[cfg(windows)]
    {
        "adb.exe"
    }
    #[cfg(not(windows))]
    {
        "adb"
    }
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn adb_cmd(adb: &Path, serial: Option<&str>) -> Command {
    let mut cmd = Command::new(adb);
    #[cfg(windows)]
    {
        cmd.creation_flags(0x0800_0000);
    }
    if let Some(s) = serial.filter(|s| !s.is_empty()) {
        cmd.arg("-s").arg(s);
    }
    cmd.stdin(Stdio::null());
    cmd
}

async fn run_adb(adb: &Path, serial: Option<&str>, args: &[&str]) -> io::Result<(bool, String, String)> {
    let output = adb_cmd(adb, serial).args(args).output().await?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    Ok((output.status.success(), stdout, stderr))
}

async fn run_root_shell(adb: &Path, serial: &str, remote_cmd: &str) -> io::Result<(bool, String, String)> {
    let wrapped = format!("su -c '{remote_cmd}'");
    run_adb(adb, Some(serial), &["shell", &wrapped]).await
}

pub async fn list_devices(adb: &Path) -> io::Result<Vec<Device>> {
    let (ok, stdout, stderr) = run_adb(adb, None, &["devices", "-l"]).await?;
    if !ok {
        return Err(io::Error::other(stderr.trim().to_owned()));
    }
    Ok(parse_devices(&stdout))
}

pub fn parse_devices(output: &str) -> Vec<Device> {
    let mut devices = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty()
            || line.starts_with("List of devices")
            || line.starts_with('*')
        {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(serial) = parts.next() else {
            continue;
        };
        let Some(state) = parts.next() else {
            continue;
        };
        let mut model = String::new();
        let mut product = String::new();
        let mut has_usb = false;
        for token in parts {
            if let Some(v) = token.strip_prefix("model:") {
                model = v.to_owned();
            } else if let Some(v) = token.strip_prefix("product:") {
                product = v.to_owned();
            } else if token.starts_with("usb:") {
                has_usb = true;
            }
        }
        let is_wifi = serial.contains("._adb-tls-connect._tcp")
            || serial.contains(':')
            || (!has_usb && serial.starts_with("adb-"));
        let transport = if has_usb && !is_wifi {
            Transport::Usb
        } else if is_wifi {
            Transport::Wifi
        } else {
            Transport::Usb
        };
        devices.push(Device {
            serial: serial.to_owned(),
            state: state.to_owned(),
            model,
            product,
            transport,
        });
    }
    devices.sort_by_key(|d| (d.transport != Transport::Usb, d.serial.clone()));
    devices
}

pub async fn poll_snapshot(adb: &Path, serial: &str) -> (ConnectionStatus, Option<DaemonSnapshot>) {
    let cmd = format!(
        "{BAREBOOT_BIN} status && echo {MARKER_IMAGES} && {BAREBOOT_BIN} images"
    );
    let Ok((ok, stdout, stderr)) = run_root_shell(adb, serial, &cmd).await else {
        return (
            ConnectionStatus::DaemonUnavailable("failed to run adb shell".into()),
            None,
        );
    };
    let clean_out = stdout.replace('\r', "");
    let clean_err = stderr.replace('\r', "");
    if !ok {
        let msg = if !clean_err.trim().is_empty() {
            clean_err.trim().to_owned()
        } else if !clean_out.trim().is_empty() {
            clean_out.trim().to_owned()
        } else {
            "bareboot daemon did not respond (grant root to ADB Shell in KernelSU/Magisk)".into()
        };
        return (ConnectionStatus::DaemonUnavailable(msg), None);
    }
    let Some((status_part, images_part)) = clean_out.split_once(MARKER_IMAGES) else {
        let detail = clean_out.trim();
        return (
            ConnectionStatus::DaemonUnavailable(if detail.is_empty() {
                "unexpected output from bareboot".into()
            } else {
                detail.to_owned()
            }),
            None,
        );
    };
    let mut snapshot = parse_status(status_part);
    snapshot.images = parse_images(images_part);
    (ConnectionStatus::Ready, Some(snapshot))
}

pub fn parse_status(text: &str) -> DaemonSnapshot {
    let mut snap = DaemonSnapshot::default();
    let mut section = "";
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("kernel:") {
            snap.kernel = parse_kernel_line(rest);
            section = "";
        } else if line.starts_with("distros:") {
            section = "distros";
        } else if line.starts_with("sessions:") {
            section = "sessions";
        } else if line.starts_with("options:") {
            section = "";
        } else if section == "distros" {
            let mut parts = line.split_whitespace();
            let Some(name) = parts.next() else { continue };
            let mut holder_pid = 0;
            let mut init_pid = 0;
            let mut image_path = String::new();
            for tok in parts {
                if let Some(v) = tok.strip_prefix("holder_pid=") {
                    holder_pid = v.parse().unwrap_or(0);
                } else if let Some(v) = tok.strip_prefix("init_pid=") {
                    init_pid = v.parse().unwrap_or(0);
                } else if let Some(v) = tok.strip_prefix("image=") {
                    image_path = v.to_owned();
                }
            }
            snap.running.push(RunningDistro {
                name: name.to_owned(),
                holder_pid,
                init_pid,
                image_path,
            });
        } else if section == "sessions" {
            let mut parts = line.split_whitespace();
            let Some(id_tok) = parts.next().and_then(|s| s.strip_prefix('#')) else {
                continue;
            };
            let id = id_tok.parse().unwrap_or(0);
            let mut distro = String::new();
            let mut pid = 0;
            let mut uid = 0;
            for tok in parts {
                if let Some(v) = tok.strip_prefix("distro=") {
                    distro = v.to_owned();
                } else if let Some(v) = tok.strip_prefix("pid=") {
                    pid = v.parse().unwrap_or(0);
                } else if let Some(v) = tok.strip_prefix("uid=") {
                    uid = v.parse().unwrap_or(0);
                }
            }
            snap.sessions.push(SessionInfo {
                id,
                distro,
                pid,
                uid,
            });
        }
    }
    snap
}

fn parse_kernel_line(rest: &str) -> KernelSummary {
    let mut k = KernelSummary::default();
    let cleaned = rest.replace(['[', ']'], " ");
    for tok in cleaned.split_whitespace() {
        if let Some(v) = tok.strip_prefix("page=") {
            k.page_size = v.parse().unwrap_or(0);
        } else if let Some(v) = tok.strip_prefix("selinux=") {
            k.selinux = v.parse().unwrap_or(0);
        } else if let Some(v) = tok.strip_prefix("mnt:") {
            k.mnt = v == "true";
        } else if let Some(v) = tok.strip_prefix("uts:") {
            k.uts = v == "true";
        } else if let Some(v) = tok.strip_prefix("pid:") {
            k.pid = v == "true";
        } else if let Some(v) = tok.strip_prefix("ipc:") {
            k.ipc = v == "true";
        } else if let Some(v) = tok.strip_prefix("net:") {
            k.net = v == "true";
        } else if let Some(v) = tok.strip_prefix("user:") {
            k.user = v == "true";
        } else if let Some(v) = tok.strip_prefix("cgroup:") {
            k.cgroup = v == "true";
        }
    }
    k
}

pub fn parse_images(text: &str) -> Vec<DistroImage> {
    let mut images = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let tokens = split_key_value_tokens(line);
        let Some(name) = tokens.first() else {
            continue;
        };
        if !valid_distro_name(name) {
            continue;
        }
        let mut img = DistroImage {
            name: name.clone(),
            used_mb: 0,
            size_gb: 0,
            free_mb: 0,
            path: String::new(),
            os_label: String::new(),
            username: String::new(),
            keep_awake: false,
            init: "none".into(),
            isolated_net: false,
            ssh_port: 0,
            forwards: Vec::new(),
            busy: String::new(),
            gpu_enabled: false,
            gpu_available: false,
            gpu_running: false,
        };
        for tok in tokens.iter().skip(1) {
            let Some((k, v)) = tok.split_once('=') else {
                continue;
            };
            let v = unquote_rust_debug(v);
            match k {
                "used" => img.used_mb = v.trim_end_matches("MB").parse().unwrap_or(0),
                "size" => img.size_gb = v.trim_end_matches("GB").parse().unwrap_or(0),
                "free" => img.free_mb = v.trim_end_matches("MB").parse().unwrap_or(0),
                "path" => img.path = v,
                "os" => img.os_label = v,
                "user" => img.username = v,
                "keep_awake" => img.keep_awake = v == "true",
                "init" => img.init = v,
                "isolated_net" => img.isolated_net = v == "true",
                "ssh_port" => img.ssh_port = v.parse().unwrap_or(0),
                "forwards" => img.forwards = parse_port_forwards(&v),
                "busy" => img.busy = v,
                "gpu_enabled" => img.gpu_enabled = v == "true",
                "gpu_available" => img.gpu_available = v == "true",
                "gpu_running" => img.gpu_running = v == "true",
                _ => {}
            }
        }
        images.push(img);
    }
    images
}

fn split_key_value_tokens(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut escaped = false;
    for ch in line.chars() {
        if escaped {
            cur.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' && in_quotes {
            cur.push(ch);
            escaped = true;
            continue;
        }
        if ch == '"' {
            in_quotes = !in_quotes;
            cur.push(ch);
            continue;
        }
        if ch.is_whitespace() && !in_quotes {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(ch);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn unquote_rust_debug(s: &str) -> String {
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        serde_json::from_str::<String>(s).unwrap_or_else(|_| s[1..s.len() - 1].to_owned())
    } else {
        s.to_owned()
    }
}

pub fn parse_port_forwards(s: &str) -> Vec<PortForward> {
    if s.is_empty() || s == "-" {
        return Vec::new();
    }
    let mut out = Vec::new();
    for item in s.split(',') {
        let item = item.trim();
        if let Some((h, g)) = item.split_once(':') {
            if let (Ok(host_port), Ok(guest_port)) = (h.parse::<u16>(), g.parse::<u16>()) {
                out.push(PortForward {
                    host_port,
                    guest_port,
                });
            }
        } else if let Ok(p) = item.parse::<u16>() {
            out.push(PortForward {
                host_port: p,
                guest_port: p,
            });
        }
    }
    out
}

pub fn format_port_forwards(items: &[PortForward]) -> String {
    items
        .iter()
        .map(|p| {
            if p.host_port == p.guest_port {
                p.host_port.to_string()
            } else {
                format!("{}:{}", p.host_port, p.guest_port)
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

pub async fn start_distro(adb: &Path, serial: &str, name: &str, image_path: &str) -> io::Result<()> {
    if !valid_distro_name(name) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid distro name"));
    }
    let cmd = format!("{BAREBOOT_BIN} start {name} {image_path}");
    let (ok, stdout, stderr) = run_root_shell(adb, serial, &cmd).await?;
    if ok {
        Ok(())
    } else {
        Err(io::Error::other(best_err(&stdout, &stderr, "failed to start distro")))
    }
}

pub async fn stop_distro(adb: &Path, serial: &str, name: &str) -> io::Result<()> {
    if !valid_distro_name(name) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid distro name"));
    }
    let cmd = format!("{BAREBOOT_BIN} stop {name}");
    let (ok, stdout, stderr) = run_root_shell(adb, serial, &cmd).await?;
    if ok {
        Ok(())
    } else {
        Err(io::Error::other(best_err(&stdout, &stderr, "failed to stop distro")))
    }
}

pub async fn configure_gpu(adb: &Path, serial: &str, name: &str, enabled: bool) -> io::Result<()> {
    if !valid_distro_name(name) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid distro name"));
    }
    let val = if enabled { "on" } else { "off" };
    let cmd = format!("{BAREBOOT_BIN} gpu {name} {val}");
    let (ok, stdout, stderr) = run_root_shell(adb, serial, &cmd).await?;
    if ok {
        Ok(())
    } else {
        Err(io::Error::other(best_err(&stdout, &stderr, "failed to change GPU setting")))
    }
}

pub async fn configure_image(adb: &Path, serial: &str, img: &DistroImage) -> io::Result<()> {
    if !valid_distro_name(&img.name) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid distro name"));
    }
    let os_b64 = BASE64.encode(img.os_label.as_bytes());
    let user_b64 = BASE64.encode(img.username.as_bytes());
    let init = if img.init == "system" { "system" } else { "none" };
    let keep = if img.keep_awake { "1" } else { "0" };
    let iso = if img.isolated_net { "1" } else { "0" };
    let forwards = format_port_forwards(&img.forwards);
    let forwards_b64 = BASE64.encode(forwards.as_bytes());
    let cmd = format!(
        "OS=$(printf %s {os_b64} | base64 -d); USR=$(printf %s {user_b64} | base64 -d); FWD=$(printf %s {forwards_b64} | base64 -d); {BAREBOOT_BIN} configure {} \"$OS\" \"$USR\" {keep} {init} {iso} {} \"$FWD\"",
        img.name, img.ssh_port
    );
    let (ok, stdout, stderr) = run_root_shell(adb, serial, &cmd).await?;
    if ok {
        Ok(())
    } else {
        Err(io::Error::other(best_err(&stdout, &stderr, "failed to configure image")))
    }
}

pub async fn read_log(adb: &Path, serial: &str, kind: LogKind, distro: Option<&str>) -> io::Result<String> {
    let cmd = match kind {
        LogKind::Daemon => format!("{BAREBOOT_BIN} log daemon"),
        LogKind::Autostart | LogKind::Sshd => {
            let Some(name) = distro.filter(|d| valid_distro_name(d)) else {
                return Ok(String::new());
            };
            format!("{BAREBOOT_BIN} log {} {name}", kind.arg())
        }
    };
    let (ok, stdout, stderr) = run_root_shell(adb, serial, &cmd).await?;
    let clean = stdout.replace('\r', "");
    if ok {
        Ok(clean)
    } else {
        Err(io::Error::other(best_err(&clean, &stderr, "failed to read log")))
    }
}

async fn run_guest_script(adb: &Path, serial: &str, distro: &str, script: &str) -> io::Result<String> {
    if !valid_distro_name(distro) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid distro name"));
    }
    let b64 = BASE64.encode(script.as_bytes());
    let inner = format!(
        "{BAREBOOT_BIN} shell {distro} -- /bin/sh -c 'printf %s {b64} | base64 -d | /bin/sh'"
    );
    let wrapped = format!("su -c \"{inner}\"");
    let (ok, stdout, stderr) = run_adb(adb, Some(serial), &["shell", &wrapped]).await?;
    let clean = stdout.replace('\r', "");
    if ok {
        Ok(clean)
    } else {
        Err(io::Error::other(best_err(&clean, &stderr, "guest command failed")))
    }
}

pub async fn scan_desktop_apps(
    adb: &Path,
    serial: &str,
    distro: &str,
    icon_cache_dir: &Path,
) -> io::Result<Vec<DesktopApp>> {
    let script = r#"
for d in /usr/share/applications "$HOME/.local/share/applications"; do
  [ -d "$d" ] || continue
  for f in "$d"/*.desktop; do
    [ -f "$f" ] || continue
    awk '
      /^\[Desktop Entry\]/ { in_entry=1; next }
      /^\[/ { in_entry=0; next }
      in_entry {
        if ($0 ~ /^(Name|GenericName|Comment|Exec|Icon|Categories|Terminal|NoDisplay|Hidden|Type)=/) {
          print FILENAME "\t" $0
        }
      }
    ' "$f"
  done
done
"#;
    let out = run_guest_script(adb, serial, distro, script).await?;
    let mut apps = parse_desktop_entries(&out);

    let distro_cache = icon_cache_dir.join(distro);
    let _ = std::fs::create_dir_all(&distro_cache);
    let mut missing_icons: Vec<String> = Vec::new();
    for app in &mut apps {
        if app.icon_name.is_empty() {
            continue;
        }
        let safe = sanitize_icon_filename(&app.icon_name);
        let cached = distro_cache.join(format!("{safe}.png"));
        if cached.is_file() {
            app.icon_path = Some(cached);
        } else if !missing_icons.contains(&app.icon_name) {
            missing_icons.push(app.icon_name.clone());
        }
    }

    if !missing_icons.is_empty()
        && let Ok(fetched) = fetch_guest_icons(adb, serial, distro, &missing_icons, &distro_cache).await
    {
        for app in &mut apps {
            if app.icon_path.is_none()
                && let Some(path) = fetched.get(&app.icon_name)
            {
                app.icon_path = Some(path.clone());
            }
        }
    }

    Ok(apps)
}

async fn fetch_guest_icons(
    adb: &Path,
    serial: &str,
    distro: &str,
    icons: &[String],
    cache_dir: &Path,
) -> io::Result<HashMap<String, PathBuf>> {
    let mut lines = String::new();
    for icon in icons.iter().take(48) {
        if icon.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_./".contains(&b)) {
            lines.push_str(icon);
            lines.push('\n');
        }
    }
    if lines.is_empty() {
        return Ok(HashMap::new());
    }
    let script = format!(
        r#"
find_icon() {{
  name="$1"
  if [ -f "$name" ]; then
    case "$name" in *.png) printf '%s' "$name"; return ;; esac
  fi
  for p in \
    "/usr/share/icons/hicolor/128x128/apps/$name.png" \
    "/usr/share/icons/hicolor/64x64/apps/$name.png" \
    "/usr/share/icons/hicolor/48x48/apps/$name.png" \
    "/usr/share/icons/hicolor/256x256/apps/$name.png" \
    "/usr/share/pixmaps/$name.png" \
    "/usr/share/icons/Adwaita/64x64/apps/$name.png" \
    "$HOME/.local/share/icons/hicolor/128x128/apps/$name.png"; do
    if [ -f "$p" ]; then
      printf '%s' "$p"
      return
    fi
  done
}}
while IFS= read -r name; do
  [ -n "$name" ] || continue
  f="$(find_icon "$name")"
  if [ -n "$f" ] && [ -r "$f" ]; then
    sz=$(wc -c < "$f" 2>/dev/null || echo 999999)
    if [ "$sz" -lt 131072 ]; then
      printf 'ICON\t%s\t' "$name"
      base64 "$f" | tr -d '\n'
      printf '\n'
    fi
  fi
done <<'EOF'
{lines}EOF
"#
    );
    let out = run_guest_script(adb, serial, distro, &script).await?;
    let mut result = HashMap::new();
    for line in out.lines() {
        let Some(rest) = line.strip_prefix("ICON\t") else {
            continue;
        };
        let Some((name, b64)) = rest.split_once('\t') else {
            continue;
        };
        if let Ok(bytes) = BASE64.decode(b64.trim())
            && bytes.len() >= 8
            && &bytes[..8] == b"\x89PNG\r\n\x1a\n"
        {
            let path = cache_dir.join(format!("{}.png", sanitize_icon_filename(name)));
            if std::fs::write(&path, &bytes).is_ok() {
                result.insert(name.to_owned(), path);
            }
        }
    }
    Ok(result)
}

fn sanitize_icon_filename(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

pub fn parse_desktop_entries(raw: &str) -> Vec<DesktopApp> {
    #[derive(Default)]
    struct RawEntry {
        name: String,
        generic_name: String,
        comment: String,
        exec: String,
        icon: String,
        categories: Vec<String>,
        terminal: bool,
        no_display: bool,
        hidden: bool,
        entry_type: String,
    }
    let mut entries: BTreeMap<String, RawEntry> = BTreeMap::new();
    for line in raw.lines() {
        let Some((path, kv)) = line.split_once('\t') else {
            continue;
        };
        let Some((key, val)) = kv.split_once('=') else {
            continue;
        };
        let id = Path::new(path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(path)
            .to_owned();
        let entry = entries.entry(id).or_default();
        match key {
            "Name" => {
                if entry.name.is_empty() {
                    entry.name = val.trim().to_owned();
                }
            }
            "GenericName" => entry.generic_name = val.trim().to_owned(),
            "Comment" => entry.comment = val.trim().to_owned(),
            "Exec" => entry.exec = val.trim().to_owned(),
            "Icon" => entry.icon = val.trim().to_owned(),
            "Categories" => {
                entry.categories = val
                    .split(';')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect();
            }
            "Terminal" => entry.terminal = val.trim().eq_ignore_ascii_case("true"),
            "NoDisplay" => entry.no_display = val.trim().eq_ignore_ascii_case("true"),
            "Hidden" => entry.hidden = val.trim().eq_ignore_ascii_case("true"),
            "Type" => entry.entry_type = val.trim().to_owned(),
            _ => {}
        }
    }
    let mut apps = Vec::new();
    for (id, e) in entries {
        if e.no_display || e.hidden || e.name.is_empty() || e.exec.is_empty() {
            continue;
        }
        if !e.entry_type.is_empty() && e.entry_type != "Application" {
            continue;
        }
        let cleaned_exec = clean_exec_line(&e.exec);
        if cleaned_exec.is_empty() {
            continue;
        }
        let comment = if !e.comment.is_empty() {
            e.comment
        } else {
            e.generic_name
        };
        apps.push(DesktopApp {
            id,
            name: e.name,
            comment,
            exec: cleaned_exec,
            icon_name: e.icon,
            icon_path: None,
            categories: e.categories,
            terminal: e.terminal,
            is_custom: false,
        });
    }
    apps.sort_by_key(|a| a.name.to_lowercase());
    apps
}

pub fn clean_exec_line(exec: &str) -> String {
    let mut out = Vec::new();
    for tok in exec.split_whitespace() {
        if matches!(
            tok,
            "%f" | "%F" | "%u" | "%U" | "%d" | "%D" | "%n" | "%N" | "%i" | "%c" | "%k" | "%v" | "%m"
        ) {
            continue;
        }
        out.push(tok.replace("%%", "%"));
    }
    out.join(" ")
}

pub async fn launch_guest_app(adb: &Path, serial: &str, distro: &str, exec_cmd: &str) -> io::Result<()> {
    let script = format!(
        r#"
if setsid -f /bin/sh -c '{escaped}' </dev/null >/dev/null 2>&1; then
  exit 0
fi
if command -v setsid >/dev/null 2>&1; then
  setsid /bin/sh -c '{escaped}' </dev/null >/dev/null 2>&1 &
  sleep 0.25
  exit 0
fi
nohup /bin/sh -c '{escaped}' </dev/null >/dev/null 2>&1 &
sleep 0.25
"#,
        escaped = shell_single_quote_escape(exec_cmd),
    );
    run_guest_script(adb, serial, distro, &script).await?;
    Ok(())
}

fn shell_single_quote_escape(s: &str) -> String {
    s.replace('\'', "'\\''")
}

pub async fn list_forwards(adb: &Path, serial: &str) -> io::Result<Vec<AdbForward>> {
    let (ok, stdout, stderr) = run_adb(adb, Some(serial), &["forward", "--list"]).await?;
    if !ok {
        return Err(io::Error::other(stderr.trim().to_owned()));
    }
    let mut out = Vec::new();
    for line in stdout.lines() {
        let mut parts = line.split_whitespace();
        let (Some(dev), Some(local), Some(remote)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        if dev == serial {
            out.push(AdbForward {
                serial: dev.to_owned(),
                local: local.to_owned(),
                remote: remote.to_owned(),
            });
        }
    }
    Ok(out)
}

pub async fn add_forward(adb: &Path, serial: &str, local_port: u16, remote_port: u16) -> io::Result<()> {
    let local = format!("tcp:{local_port}");
    let remote = format!("tcp:{remote_port}");
    let (ok, stdout, stderr) = run_adb(adb, Some(serial), &["forward", &local, &remote]).await?;
    if ok {
        Ok(())
    } else {
        Err(io::Error::other(best_err(&stdout, &stderr, "adb forward failed")))
    }
}

pub async fn remove_forward(adb: &Path, serial: &str, local: &str) -> io::Result<()> {
    let (ok, stdout, stderr) = run_adb(adb, Some(serial), &["forward", "--remove", local]).await?;
    if ok {
        Ok(())
    } else {
        Err(io::Error::other(best_err(&stdout, &stderr, "adb forward --remove failed")))
    }
}

pub async fn remove_all_forwards(adb: &Path, serial: &str) -> io::Result<()> {
    let (ok, stdout, stderr) = run_adb(adb, Some(serial), &["forward", "--remove-all"]).await?;
    if ok {
        Ok(())
    } else {
        Err(io::Error::other(best_err(&stdout, &stderr, "adb forward --remove-all failed")))
    }
}

pub async fn push_to_shared(adb: &Path, serial: &str, local_path: &Path) -> io::Result<String> {
    let name = local_path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid filename"))?;
    let remote = format!("{SHARED_DIR}/{name}");
    let local_str = local_path.to_string_lossy().into_owned();
    let (ok, stdout, stderr) = run_adb(adb, Some(serial), &["push", &local_str, &remote]).await?;
    if ok {
        Ok(format!("/mnt/android/{name}"))
    } else {
        Err(io::Error::other(best_err(&stdout, &stderr, "adb push failed")))
    }
}

pub struct TerminalLaunch<'a> {
    pub terminal: &'a TerminalApp,
    pub adb: &'a Path,
    pub serial: &'a str,
    pub distro: &'a str,
    pub via_ssh: bool,
    pub ssh_port: u16,
    pub username: &'a str,
    pub guest_cmd: Option<&'a str>,
}

pub fn open_external_terminal(req: &TerminalLaunch<'_>) -> io::Result<()> {
    let (terminal, adb, serial, distro, via_ssh, ssh_port, username, guest_cmd) = (
        req.terminal,
        req.adb,
        req.serial,
        req.distro,
        req.via_ssh,
        req.ssh_port,
        req.username,
        req.guest_cmd,
    );
    let argv: Vec<String> = if via_ssh && ssh_port > 0 && guest_cmd.is_none() {
        let user = if username.is_empty() { distro } else { username };
        vec![
            "ssh".into(),
            "-p".into(),
            ssh_port.to_string(),
            "-o".into(),
            "StrictHostKeyChecking=no".into(),
            "-o".into(),
            "UserKnownHostsFile=/dev/null".into(),
            "-o".into(),
            "LogLevel=ERROR".into(),
            format!("{user}@127.0.0.1"),
        ]
    } else {
        let remote = match guest_cmd {
            Some(cmd) => {
                let b64 = BASE64.encode(cmd.as_bytes());
                format!(
                    "su -c \"{BAREBOOT_BIN} shell {distro} -- /bin/sh -c 'printf %s {b64} | base64 -d | /bin/sh'\""
                )
            }
            None => format!("su -c '{BAREBOOT_BIN} shell {distro}'"),
        };
        vec![
            adb.to_string_lossy().into_owned(),
            "-s".into(),
            serial.to_owned(),
            "shell".into(),
            "-t".into(),
            remote,
        ]
    };
    spawn_in_terminal(terminal, &argv)
}

fn spawn_in_terminal(choice: &TerminalApp, argv: &[String]) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        spawn_macos_terminal(choice, argv)
    }
    #[cfg(target_os = "linux")]
    {
        spawn_linux_terminal(choice, argv)
    }
    #[cfg(target_os = "windows")]
    {
        spawn_windows_terminal(choice, argv)
    }
}

#[cfg(target_os = "macos")]
fn spawn_macos_terminal(choice: &TerminalApp, argv: &[String]) -> io::Result<()> {
    let resolved = match choice {
        TerminalApp::Auto => detect_macos_terminal(),
        other => other.clone(),
    };
    match resolved {
        TerminalApp::Ghostty => {
            std::process::Command::new("open")
                .args(["-na", "Ghostty", "--args", "-e"])
                .args(argv)
                .spawn()?;
        }
        TerminalApp::Kitty => {
            std::process::Command::new("open")
                .args(["-na", "kitty", "--args"])
                .args(argv)
                .spawn()?;
        }
        TerminalApp::Alacritty => {
            std::process::Command::new("open")
                .args(["-na", "Alacritty", "--args", "-e"])
                .args(argv)
                .spawn()?;
        }
        TerminalApp::WezTerm => {
            std::process::Command::new("open")
                .args(["-na", "WezTerm", "--args", "start", "--"])
                .args(argv)
                .spawn()?;
        }
        TerminalApp::ITerm2 => {
            let line = shell_join(argv);
            let script = format!(
                "tell application \"iTerm\"\nactivate\ncreate window with default profile command \"{}\"\nend tell",
                line.replace('\\', "\\\\").replace('"', "\\\"")
            );
            std::process::Command::new("osascript")
                .args(["-e", &script])
                .spawn()?;
        }
        TerminalApp::Warp => {
            let line = shell_join(argv);
            let tmp = std::env::temp_dir().join(format!("bareboot-term-{}.command", std::process::id()));
            std::fs::write(&tmp, format!("#!/bin/sh\nexec {line}\n"))?;
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755));
            std::process::Command::new("open")
                .args(["-a", "Warp"])
                .arg(&tmp)
                .spawn()?;
        }
        TerminalApp::Custom(bin) => {
            std::process::Command::new(bin)
                .arg("-e")
                .args(argv)
                .spawn()?;
        }
        _ => {
            let line = shell_join(argv);
            let script = format!(
                "tell application \"Terminal\"\nactivate\ndo script \"{}\"\nend tell",
                line.replace('\\', "\\\\").replace('"', "\\\"")
            );
            std::process::Command::new("osascript")
                .args(["-e", &script])
                .spawn()?;
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn detect_macos_terminal() -> TerminalApp {
    let checks = [
        ("/Applications/Ghostty.app", TerminalApp::Ghostty),
        ("/Applications/iTerm.app", TerminalApp::ITerm2),
        ("/Applications/kitty.app", TerminalApp::Kitty),
        ("/Applications/Alacritty.app", TerminalApp::Alacritty),
        ("/Applications/WezTerm.app", TerminalApp::WezTerm),
        ("/Applications/Warp.app", TerminalApp::Warp),
    ];
    for (path, app) in checks {
        if Path::new(path).exists() {
            return app;
        }
    }
    TerminalApp::TerminalApp
}

#[cfg(target_os = "linux")]
fn spawn_linux_terminal(choice: &TerminalApp, argv: &[String]) -> io::Result<()> {
    let resolved = match choice {
        TerminalApp::Auto => detect_linux_terminal(),
        other => other.clone(),
    };
    match resolved {
        TerminalApp::Ghostty => {
            std::process::Command::new("ghostty").arg("-e").args(argv).spawn()?;
        }
        TerminalApp::Kitty => {
            std::process::Command::new("kitty").args(argv).spawn()?;
        }
        TerminalApp::Alacritty => {
            std::process::Command::new("alacritty").arg("-e").args(argv).spawn()?;
        }
        TerminalApp::WezTerm => {
            std::process::Command::new("wezterm").args(["start", "--"]).args(argv).spawn()?;
        }
        TerminalApp::Foot => {
            std::process::Command::new("foot").args(argv).spawn()?;
        }
        TerminalApp::GnomeTerminal => {
            std::process::Command::new("gnome-terminal").arg("--").args(argv).spawn()?;
        }
        TerminalApp::Konsole => {
            std::process::Command::new("konsole").arg("-e").args(argv).spawn()?;
        }
        TerminalApp::Custom(bin) => {
            std::process::Command::new(bin).arg("-e").args(argv).spawn()?;
        }
        _ => {
            std::process::Command::new("x-terminal-emulator").arg("-e").args(argv).spawn()?;
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn detect_linux_terminal() -> TerminalApp {
    let checks = [
        ("ghostty", TerminalApp::Ghostty),
        ("kitty", TerminalApp::Kitty),
        ("alacritty", TerminalApp::Alacritty),
        ("wezterm", TerminalApp::WezTerm),
        ("foot", TerminalApp::Foot),
        ("gnome-terminal", TerminalApp::GnomeTerminal),
        ("konsole", TerminalApp::Konsole),
    ];
    for (bin, app) in checks {
        if find_in_path(bin).is_some() {
            return app;
        }
    }
    TerminalApp::GnomeTerminal
}

#[cfg(target_os = "windows")]
fn spawn_windows_terminal(choice: &TerminalApp, argv: &[String]) -> io::Result<()> {
    let resolved = match choice {
        TerminalApp::Auto => {
            if find_in_path("wt.exe").is_some() {
                TerminalApp::WindowsTerminal
            } else {
                TerminalApp::Custom("cmd.exe".into())
            }
        }
        other => other.clone(),
    };
    match resolved {
        TerminalApp::WindowsTerminal => {
            std::process::Command::new("wt.exe").args(argv).spawn()?;
        }
        TerminalApp::Alacritty => {
            std::process::Command::new("alacritty.exe").arg("-e").args(argv).spawn()?;
        }
        TerminalApp::WezTerm => {
            std::process::Command::new("wezterm.exe").args(["start", "--"]).args(argv).spawn()?;
        }
        TerminalApp::Custom(bin) => {
            std::process::Command::new(bin).args(argv).spawn()?;
        }
        _ => {
            std::process::Command::new("cmd.exe")
                .args(["/C", "start"])
                .args(argv)
                .spawn()?;
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn shell_join(args: &[String]) -> String {
    args.iter()
        .map(|a| format!("'{}'", shell_single_quote_escape(a)))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn open_url(url: &str) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd").args(["/C", "start", "", url]).spawn();
}

fn best_err(stdout: &str, stderr: &str, fallback: &str) -> String {
    if !stderr.trim().is_empty() {
        stderr.trim().to_owned()
    } else if !stdout.trim().is_empty() {
        stdout.trim().to_owned()
    } else {
        fallback.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_usb_and_wifi_devices() {
        let sample = "\
List of devices attached
0123456789ABCDEF       device usb:1-1 product:device_beta model:Pixel_10_Pro device:phone transport_id:1
adb-0123456789ABCDEF-abc123._adb-tls-connect._tcp device product:device_beta model:Pixel_10_Pro device:phone transport_id:2
";
        let devs = parse_devices(sample);
        assert_eq!(devs.len(), 2);
        assert_eq!(devs[0].transport, Transport::Usb);
        assert_eq!(devs[0].display_name(), "Pixel 10 Pro");
        assert_eq!(devs[1].transport, Transport::Wifi);
    }

    #[test]
    fn parses_status_and_images() {
        let status = "\
kernel: page=4096 selinux=1 ns=[mnt:true uts:true pid:true ipc:true net:true user:false cgroup:true]
options: purge_on_uninstall=false relaxed_sync=false
distros: 1
  arch holder_pid=25235 init_pid=25245 image=/data/adb/bareboot/distros/arch/rootfs.img
sessions: 1
  #49 distro=arch pid=7606 uid=1000
";
        let snap = parse_status(status);
        assert_eq!(snap.kernel.page_size, 4096);
        assert!(snap.kernel.pid);
        assert!(!snap.kernel.user);
        assert_eq!(snap.running.len(), 1);
        assert_eq!(snap.running[0].name, "arch");
        assert_eq!(snap.sessions.len(), 1);

        let imgs = parse_images(
            r#"arch used=3981MB size=8GB free=4037MB path=/data/adb/bareboot/distros/arch/rootfs.img os="Arch Linux" user="arch" keep_awake=false init=system isolated_net=true ssh_port=2222 forwards=8080:80,3000 busy="" gpu_enabled=true gpu_available=true gpu_running=true"#,
        );
        assert_eq!(imgs.len(), 1);
        assert_eq!(imgs[0].os_label, "Arch Linux");
        assert_eq!(imgs[0].used_mb, 3981);
        assert_eq!(imgs[0].forwards.len(), 2);
        assert_eq!(imgs[0].forwards[0], PortForward { host_port: 8080, guest_port: 80 });
        assert_eq!(imgs[0].forwards[1], PortForward { host_port: 3000, guest_port: 3000 });
    }

    #[test]
    fn parses_desktop_entries_and_filters_hidden() {
        let raw = "\
/usr/share/applications/firefox.desktop\tName=Firefox
/usr/share/applications/firefox.desktop\tExec=/usr/lib/firefox/firefox %u
/usr/share/applications/firefox.desktop\tIcon=firefox
/usr/share/applications/firefox.desktop\tComment=Fast browser
/usr/share/applications/firefox.desktop\tTerminal=false
/usr/share/applications/firefox.desktop\tType=Application
/usr/share/applications/pinentry.desktop\tName=Pinentry
/usr/share/applications/pinentry.desktop\tExec=/usr/bin/pinentry
/usr/share/applications/pinentry.desktop\tNoDisplay=true
";
        let apps = parse_desktop_entries(raw);
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].name, "Firefox");
        assert_eq!(apps[0].exec, "/usr/lib/firefox/firefox");
        assert!(!apps[0].terminal);
    }
}
