use std::borrow::Cow;

use gpui_kit::{AssetSource, SharedString};

pub struct Assets;

const SVG_COOKIE9: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><path fill="currentColor" d="M 50.00,1.50 L 52.77,2.38 L 55.28,4.81 L 57.38,8.15 L 59.11,11.56 L 60.70,14.26 L 62.48,15.70 L 64.78,15.75 L 67.73,14.70 L 71.25,13.19 L 75.00,11.99 L 78.48,11.74 L 81.18,12.85 L 82.73,15.31 L 83.10,18.78 L 82.56,22.68 L 81.68,26.41 L 81.17,29.50 L 81.61,31.75 L 83.34,33.26 L 86.27,34.35 L 89.94,35.46 L 93.59,36.95 L 96.41,39.00 L 97.76,41.58 L 97.37,44.46 L 95.42,47.35 L 92.50,50.00 L 89.43,52.30 L 87.05,54.33 L 85.95,56.34 L 86.30,58.60 L 87.84,61.33 L 89.94,64.54 L 91.78,68.02 L 92.62,71.41 L 92.00,74.25 L 89.85,76.21 L 86.50,77.17 L 82.56,77.32 L 78.73,77.11 L 75.60,77.13 L 73.46,77.96 L 72.28,79.92 L 71.71,83.00 L 71.25,86.81 L 70.42,90.66 L 68.89,93.80 L 66.59,95.58 L 63.68,95.69 L 60.49,94.27 L 57.38,91.85 L 54.59,89.23 L 52.17,87.24 L 50.00,86.50 L 47.83,87.24 L 45.41,89.23 L 42.62,91.85 L 39.51,94.27 L 36.32,95.69 L 33.41,95.58 L 31.11,93.80 L 29.58,90.66 L 28.75,86.81 L 28.29,83.00 L 27.72,79.92 L 26.54,77.96 L 24.40,77.13 L 21.27,77.11 L 17.44,77.32 L 13.50,77.17 L 10.15,76.21 L 8.00,74.25 L 7.38,71.41 L 8.22,68.02 L 10.06,64.54 L 12.16,61.33 L 13.70,58.60 L 14.05,56.34 L 12.95,54.33 L 10.57,52.30 L 7.50,50.00 L 4.58,47.35 L 2.63,44.46 L 2.24,41.58 L 3.59,39.00 L 6.41,36.95 L 10.06,35.46 L 13.73,34.35 L 16.66,33.26 L 18.39,31.75 L 18.83,29.50 L 18.32,26.41 L 17.44,22.68 L 16.90,18.78 L 17.27,15.31 L 18.82,12.85 L 21.52,11.74 L 25.00,11.99 L 28.75,13.19 L 32.27,14.70 L 35.22,15.75 L 37.52,15.70 L 39.30,14.26 L 40.89,11.56 L 42.62,8.15 L 44.72,4.81 L 47.23,2.38 Z"/></svg>"##;
const SVG_BRAND: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="16" rx="3"/><path d="M7 9l3 3-3 3"/><path d="M13 15h4"/></svg>"##;
const SVG_APPS: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="7" height="7" rx="2"/><rect x="14" y="3" width="7" height="7" rx="2"/><rect x="3" y="14" width="7" height="7" rx="2"/><rect x="14" y="14" width="7" height="7" rx="2"/></svg>"##;
const SVG_LAYERS: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><polygon points="12 2 2 7 12 12 22 7 12 2"/><polyline points="2 17 12 22 22 17"/><polyline points="2 12 12 17 22 12"/></svg>"##;
const SVG_TERMINAL: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="4 17 10 11 4 5"/><line x1="12" y1="19" x2="20" y2="19"/></svg>"##;
const SVG_MONITOR: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><rect x="2" y="3" width="20" height="14" rx="2.5"/><line x1="8" y1="21" x2="16" y2="21"/><line x1="12" y1="17" x2="12" y2="21"/></svg>"##;
const SVG_NETWORK: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12h14"/><path d="M12 5l7 7-7 7"/><circle cx="4" cy="12" r="1.5"/></svg>"##;
const SVG_LOG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="8" y1="13" x2="16" y2="13"/><line x1="8" y1="17" x2="13" y2="17"/></svg>"##;
const SVG_SETTINGS: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9c.26.6.85 1 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>"##;
const SVG_PHONE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><rect x="5" y="2" width="14" height="20" rx="2.5"/><line x1="12" y1="18" x2="12.01" y2="18"/></svg>"##;
const SVG_USB: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><circle cx="10" cy="7" r="1"/><circle cx="4" cy="20" r="1"/><path d="M4.7 19.3 19 5"/><path d="m21 3-3 1 2 2Z"/><path d="M9.26 7.68 5 12l2 5"/><path d="m10 14 5 2 3.5-3.5"/><path d="m18 12 1-1 1 1-1 1Z"/></svg>"##;
const SVG_WIFI: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12.55a11 11 0 0 1 14.08 0"/><path d="M1.42 9a16 16 0 0 1 21.16 0"/><path d="M8.53 16.11a6 6 0 0 1 6.95 0"/><line x1="12" y1="20" x2="12.01" y2="20"/></svg>"##;
const SVG_PLAY: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polygon points="6 4 20 12 6 20 6 4"/></svg>"##;
const SVG_STOP: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="5" y="5" width="14" height="14" rx="2.5"/></svg>"##;
const SVG_REFRESH: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12a9 9 0 1 1-2.64-6.36"/><polyline points="21 3 21 9 15 9"/></svg>"##;
const SVG_SEARCH: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="7"/><line x1="21" y1="21" x2="16.65" y2="16.65"/></svg>"##;
const SVG_PIN: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><line x1="12" y1="17" x2="12" y2="22"/><path d="M5 17h14v-1.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V6h1a2 2 0 0 0 0-4H8a2 2 0 0 0 0 4h1v4.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24Z"/></svg>"##;
const SVG_PLUS: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/></svg>"##;
const SVG_TRASH: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/><path d="M10 11v6"/><path d="M14 11v6"/><path d="M9 6V4a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2"/></svg>"##;
const SVG_COPY: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>"##;
const SVG_EXTERNAL: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/><polyline points="15 3 21 3 21 9"/><line x1="10" y1="14" x2="21" y2="3"/></svg>"##;
const SVG_FOLDER: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>"##;
const SVG_GPU: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><rect x="4" y="4" width="16" height="16" rx="2"/><rect x="9" y="9" width="6" height="6"/><line x1="9" y1="1" x2="9" y2="4"/><line x1="15" y1="1" x2="15" y2="4"/><line x1="9" y1="20" x2="9" y2="23"/><line x1="15" y1="20" x2="15" y2="23"/><line x1="20" y1="9" x2="23" y2="9"/><line x1="20" y1="14" x2="23" y2="14"/><line x1="1" y1="9" x2="4" y2="9"/><line x1="1" y1="14" x2="4" y2="14"/></svg>"##;
const SVG_CLOSE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>"##;
const SVG_CHECK: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>"##;
const SVG_SLIDERS: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><line x1="4" y1="21" x2="4" y2="14"/><line x1="4" y1="10" x2="4" y2="3"/><line x1="12" y1="21" x2="12" y2="12"/><line x1="12" y1="8" x2="12" y2="3"/><line x1="20" y1="21" x2="20" y2="16"/><line x1="20" y1="12" x2="20" y2="3"/><line x1="1" y1="14" x2="7" y2="14"/><line x1="9" y1="8" x2="15" y2="8"/><line x1="17" y1="16" x2="23" y2="16"/></svg>"##;
const SVG_COMMAND: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M18 3a3 3 0 0 0-3 3v12a3 3 0 0 0 3 3 3 3 0 0 0 3-3 3 3 0 0 0-3-3H6a3 3 0 0 0-3 3 3 3 0 0 0 3 3 3 3 0 0 0 3-3V6a3 3 0 0 0-3-3 3 3 0 0 0-3 3 3 3 0 0 0 3 3h12a3 3 0 0 0 3-3 3 3 0 0 0-3-3z"/></svg>"##;
const SVG_EYE_OFF: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94"/><path d="M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19"/><line x1="1" y1="1" x2="23" y2="23"/></svg>"##;
const BRAND_PNG: &[u8] = include_bytes!("../../../../docs/images/icon.png");

fn find(path: &str) -> Option<&'static [u8]> {
    Some(match path {
        "shapes/cookie9.svg" => SVG_COOKIE9,
        "icons/brand.svg" => SVG_BRAND,
        "icons/apps.svg" => SVG_APPS,
        "icons/layers.svg" => SVG_LAYERS,
        "icons/terminal.svg" => SVG_TERMINAL,
        "icons/monitor.svg" => SVG_MONITOR,
        "icons/network.svg" => SVG_NETWORK,
        "icons/log.svg" => SVG_LOG,
        "icons/settings.svg" => SVG_SETTINGS,
        "icons/phone.svg" => SVG_PHONE,
        "icons/usb.svg" => SVG_USB,
        "icons/wifi.svg" => SVG_WIFI,
        "icons/play.svg" => SVG_PLAY,
        "icons/stop.svg" => SVG_STOP,
        "icons/refresh.svg" => SVG_REFRESH,
        "icons/search.svg" => SVG_SEARCH,
        "icons/pin.svg" => SVG_PIN,
        "icons/plus.svg" => SVG_PLUS,
        "icons/trash.svg" => SVG_TRASH,
        "icons/copy.svg" => SVG_COPY,
        "icons/external.svg" => SVG_EXTERNAL,
        "icons/folder.svg" => SVG_FOLDER,
        "icons/gpu.svg" => SVG_GPU,
        "icons/close.svg" => SVG_CLOSE,
        "icons/check.svg" => SVG_CHECK,
        "icons/sliders.svg" => SVG_SLIDERS,
        "icons/command.svg" => SVG_COMMAND,
        "icons/eye-off.svg" => SVG_EYE_OFF,
        "brand/icon.png" => BRAND_PNG,
        _ => return None,
    })
}

const ICONS: &[&str] = &[
    "shapes/cookie9.svg",
    "icons/brand.svg",
    "icons/apps.svg",
    "icons/layers.svg",
    "icons/terminal.svg",
    "icons/monitor.svg",
    "icons/network.svg",
    "icons/log.svg",
    "icons/settings.svg",
    "icons/phone.svg",
    "icons/usb.svg",
    "icons/wifi.svg",
    "icons/play.svg",
    "icons/stop.svg",
    "icons/refresh.svg",
    "icons/search.svg",
    "icons/pin.svg",
    "icons/plus.svg",
    "icons/trash.svg",
    "icons/copy.svg",
    "icons/external.svg",
    "icons/folder.svg",
    "icons/gpu.svg",
    "icons/close.svg",
    "icons/check.svg",
    "icons/sliders.svg",
    "icons/command.svg",
    "icons/eye-off.svg",
    "brand/icon.png",
];

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui_kit::Result<Option<Cow<'static, [u8]>>> {
        Ok(find(path).map(Cow::Borrowed))
    }

    fn list(&self, path: &str) -> gpui_kit::Result<Vec<SharedString>> {
        Ok(ICONS
            .iter()
            .filter(|icon| icon.starts_with(path))
            .map(|icon| SharedString::from(*icon))
            .collect())
    }
}
