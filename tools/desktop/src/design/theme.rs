use gpui_kit::{Rgba, WindowAppearance, rgb, rgba};
use serde::{Deserialize, Serialize};

#[cfg(target_os = "macos")]
pub const FONT: &str = ".SystemUIFont";
#[cfg(target_os = "windows")]
pub const FONT: &str = "Segoe UI";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub const FONT: &str = "sans-serif";

#[cfg(target_os = "macos")]
pub const MONO: &str = "Menlo";
#[cfg(target_os = "windows")]
pub const MONO: &str = "Consolas";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub const MONO: &str = "monospace";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    pub fn resolve(self, appearance: WindowAppearance) -> Scheme {
        match self {
            Self::Light => Scheme::Light,
            Self::Dark => Scheme::Dark,
            Self::System => match appearance {
                WindowAppearance::Dark | WindowAppearance::VibrantDark => Scheme::Dark,
                _ => Scheme::Light,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    Light,
    Dark,
}

#[derive(Clone, Copy)]
pub struct Palette {
    pub scheme: Scheme,
    pub surface: Rgba,
    pub surface_low: Rgba,
    pub surface_container: Rgba,
    pub surface_high: Rgba,
    pub surface_highest: Rgba,
    pub primary: Rgba,
    pub primary_hover: Rgba,
    pub on_primary: Rgba,
    pub primary_container: Rgba,
    pub on_primary_container: Rgba,
    pub secondary_container: Rgba,
    pub secondary_container_hover: Rgba,
    pub on_secondary_container: Rgba,
    pub tertiary: Rgba,
    pub tertiary_container: Rgba,
    pub on_tertiary_container: Rgba,
    pub on_surface: Rgba,
    pub on_surface_variant: Rgba,
    pub outline_variant: Rgba,
    pub error: Rgba,
    pub error_container: Rgba,
    pub on_error_container: Rgba,
    pub amber: Rgba,
    pub amber_container: Rgba,
    pub scrim: Rgba,
}

impl Palette {
    pub fn of(scheme: Scheme) -> Self {
        match scheme {
            Scheme::Light => Self {
                scheme,
                surface: rgb(0xfaf8f4),
                surface_low: rgb(0xf3f0ea),
                surface_container: rgb(0xedeae2),
                surface_high: rgb(0xe7e3da),
                surface_highest: rgb(0xe0dcd2),
                primary: rgb(0x5e5946),
                primary_hover: rgb(0x4c4737),
                on_primary: rgb(0xffffff),
                primary_container: rgb(0xe5dfc9),
                on_primary_container: rgb(0x1e1b11),
                secondary_container: rgb(0xe2ded3),
                secondary_container_hover: rgb(0xd6d1c5),
                on_secondary_container: rgb(0x1f1d17),
                tertiary: rgb(0x535e54),
                tertiary_container: rgb(0xd7e1d7),
                on_tertiary_container: rgb(0x141e16),
                on_surface: rgb(0x1c1b18),
                on_surface_variant: rgb(0x4b4840),
                outline_variant: rgb(0xcac5ba),
                error: rgb(0xba1a1a),
                error_container: rgb(0xffdad6),
                on_error_container: rgb(0x410002),
                amber: rgb(0x8a5b14),
                amber_container: rgb(0xf9e8c8),
                scrim: rgba(0x12100c4d),
            },
            Scheme::Dark => Self {
                scheme,
                surface: rgb(0x12110f),
                surface_low: rgb(0x171614),
                surface_container: rgb(0x1e1d1b),
                surface_high: rgb(0x282724),
                surface_highest: rgb(0x34322e),
                primary: rgb(0xd1cbb9),
                primary_hover: rgb(0xdfd9c8),
                on_primary: rgb(0x2c291f),
                primary_container: rgb(0x4a4538),
                on_primary_container: rgb(0xeee9dc),
                secondary_container: rgb(0x3c3a32),
                secondary_container_hover: rgb(0x49463d),
                on_secondary_container: rgb(0xe5e0d4),
                tertiary: rgb(0xb8c2b6),
                tertiary_container: rgb(0x343a33),
                on_tertiary_container: rgb(0xdce4da),
                on_surface: rgb(0xe7e3db),
                on_surface_variant: rgb(0xa5a096),
                outline_variant: rgb(0x47443d),
                error: rgb(0xf2b8b5),
                error_container: rgb(0x5c2624),
                on_error_container: rgb(0xf9dedc),
                amber: rgb(0xe5c17d),
                amber_container: rgb(0x3e331e),
                scrim: rgba(0x090807b3),
            },
        }
    }
}
