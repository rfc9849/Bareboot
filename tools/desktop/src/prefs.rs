use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::design::ThemeMode;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum TerminalApp {
    #[default]
    Auto,
    Ghostty,
    ITerm2,
    TerminalApp,
    Kitty,
    Alacritty,
    WezTerm,
    Warp,
    Foot,
    GnomeTerminal,
    Konsole,
    WindowsTerminal,
    Custom(String),
}

impl TerminalApp {
    pub fn label(&self) -> String {
        match self {
            Self::Auto => "Auto-detect".into(),
            Self::Ghostty => "Ghostty".into(),
            Self::ITerm2 => "iTerm2".into(),
            Self::TerminalApp => "Terminal.app".into(),
            Self::Kitty => "Kitty".into(),
            Self::Alacritty => "Alacritty".into(),
            Self::WezTerm => "WezTerm".into(),
            Self::Warp => "Warp".into(),
            Self::Foot => "Foot".into(),
            Self::GnomeTerminal => "GNOME Terminal".into(),
            Self::Konsole => "Konsole".into(),
            Self::WindowsTerminal => "Windows Terminal".into(),
            Self::Custom(cmd) => format!("Custom ({cmd})"),
        }
    }

    pub fn platform_choices() -> Vec<Self> {
        #[cfg(target_os = "macos")]
        {
            vec![
                Self::Auto,
                Self::Ghostty,
                Self::ITerm2,
                Self::Kitty,
                Self::Alacritty,
                Self::WezTerm,
                Self::Warp,
                Self::TerminalApp,
            ]
        }
        #[cfg(target_os = "linux")]
        {
            vec![
                Self::Auto,
                Self::Ghostty,
                Self::Kitty,
                Self::Alacritty,
                Self::WezTerm,
                Self::Foot,
                Self::GnomeTerminal,
                Self::Konsole,
            ]
        }
        #[cfg(target_os = "windows")]
        {
            vec![
                Self::Auto,
                Self::WindowsTerminal,
                Self::Alacritty,
                Self::WezTerm,
            ]
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomShortcut {
    pub id: String,
    pub distro: String,
    pub name: String,
    pub exec: String,
    pub terminal: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub theme: ThemeMode,
    pub preferred_serial: Option<String>,
    pub selected_distro: Option<String>,
    pub adb_path: Option<String>,
    pub terminal: TerminalApp,
    pub terminal_via_ssh: bool,
    pub cmd_to_ctrl: bool,
    pub auto_reconnect_display: bool,
    pub auto_forward_ports: bool,
    pub pinned_apps: Vec<String>,
    pub hidden_apps: Vec<String>,
    pub custom_apps: Vec<CustomShortcut>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            theme: ThemeMode::System,
            preferred_serial: None,
            selected_distro: None,
            adb_path: None,
            terminal: TerminalApp::Auto,
            terminal_via_ssh: false,
            cmd_to_ctrl: cfg!(target_os = "macos"),
            auto_reconnect_display: true,
            auto_forward_ports: true,
            pinned_apps: Vec::new(),
            hidden_apps: Vec::new(),
            custom_apps: Vec::new(),
        }
    }
}

pub fn config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("bareboot-desktop"))
}

pub fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("bareboot-desktop")
}

impl Prefs {
    pub fn load() -> Self {
        let Some(path) = config_dir().map(|d| d.join("prefs.json")) else {
            return Self::default();
        };
        let Ok(bytes) = std::fs::read(&path) else {
            return Self::default();
        };
        serde_json::from_slice(&bytes).unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(dir) = config_dir() else {
            return;
        };
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let path = dir.join("prefs.json");
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(path, json);
        }
    }

    pub fn pin_key(distro: &str, app_id: &str) -> String {
        format!("{distro}::{app_id}")
    }

    pub fn is_pinned(&self, distro: &str, app_id: &str) -> bool {
        let key = Self::pin_key(distro, app_id);
        self.pinned_apps.iter().any(|k| k == &key)
    }

    pub fn toggle_pin(&mut self, distro: &str, app_id: &str) -> bool {
        let key = Self::pin_key(distro, app_id);
        if let Some(pos) = self.pinned_apps.iter().position(|k| k == &key) {
            self.pinned_apps.remove(pos);
            self.save();
            false
        } else {
            self.pinned_apps.push(key);
            self.save();
            true
        }
    }

    pub fn is_hidden(&self, distro: &str, app_id: &str) -> bool {
        let key = Self::pin_key(distro, app_id);
        self.hidden_apps.iter().any(|k| k == &key)
    }

    pub fn toggle_hidden(&mut self, distro: &str, app_id: &str) -> bool {
        let key = Self::pin_key(distro, app_id);
        if let Some(pos) = self.hidden_apps.iter().position(|k| k == &key) {
            self.hidden_apps.remove(pos);
            self.save();
            false
        } else {
            self.hidden_apps.push(key);
            self.save();
            true
        }
    }
}
