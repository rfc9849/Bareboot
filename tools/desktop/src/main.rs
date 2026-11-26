#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::path::PathBuf;

use bareboot_desktop::adb::resolve_adb;
use bareboot_desktop::app::{Escape, FocusSearch, QuickRun, Refresh};
use bareboot_desktop::backend::Command;
use bareboot_desktop::display::{HostConfig, run_host, valid_distro_name};
use bareboot_desktop::single::{Message, Role, acquire};
use bareboot_desktop::{AppView, Assets, Prefs, backend, config_dir};
use futures::StreamExt;
use gpui_kit::prelude::*;
use gpui_kit::{
    Bounds, KeyBinding, Menu, MenuItem, TitlebarOptions, WindowBounds, WindowDecorations,
    WindowOptions, point, px, size,
};

gpui_kit::actions!([Quit]);

enum LaunchMode {
    Companion,
    DisplayHost(HostConfig),
}

fn parse_args() -> Result<LaunchMode, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        return Ok(LaunchMode::Companion);
    }

    if args[0] == "--display-host" {
        let Some(distro) = args.get(1).cloned() else {
            return Err("Usage: bareboot-desktop --display-host DISTRO [--serial SERIAL] [--adb PATH] [--events] [--cmd-to-ctrl]".into());
        };
        let mut serial = None;
        let mut adb_path: Option<PathBuf> = None;
        let mut emit_events = false;
        let mut cmd_to_ctrl = false;
        let mut i = 2;
        while i < args.len() {
            match args[i].as_str() {
                "--serial" => {
                    serial = args.get(i + 1).cloned();
                    i += 2;
                }
                "--adb" => {
                    adb_path = args.get(i + 1).map(PathBuf::from);
                    i += 2;
                }
                "--events" => {
                    emit_events = true;
                    i += 1;
                }
                "--cmd-to-ctrl" => {
                    cmd_to_ctrl = true;
                    i += 1;
                }
                other => return Err(format!("Unknown argument: {other}")),
            }
        }
        let adb = adb_path
            .or_else(|| resolve_adb(None))
            .unwrap_or_else(|| PathBuf::from("adb"));
        return Ok(LaunchMode::DisplayHost(HostConfig {
            distro,
            serial,
            adb_path: adb,
            emit_events,
            cmd_to_ctrl,
        }));
    }

    if args.len() == 1 && !args[0].starts_with('-') && valid_distro_name(&args[0]) {
        let adb = resolve_adb(None).unwrap_or_else(|| PathBuf::from("adb"));
        return Ok(LaunchMode::DisplayHost(HostConfig {
            distro: args[0].clone(),
            serial: None,
            adb_path: adb,
            emit_events: false,
            cmd_to_ctrl: cfg!(target_os = "macos"),
        }));
    }

    Ok(LaunchMode::Companion)
}

fn main() {
    match parse_args() {
        Ok(LaunchMode::DisplayHost(cfg)) => {
            std::process::exit(run_host(cfg));
        }
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(2);
        }
        Ok(LaunchMode::Companion) => {}
    }

    let lock = config_dir()
        .map(|dir| dir.join("desktop.sock"))
        .unwrap_or_else(|| std::env::temp_dir().join("bareboot-desktop.sock"));
    let Role::Primary(mut messages) = acquire(&lock, &Message::Show) else {
        return;
    };

    let prefs = Prefs::load();
    let application = gpui_kit::application().with_assets(Assets);
    application.run(move |cx| {
        gpui_kit::init(cx);
        let (backend_handle, updates) = backend::spawn(&prefs);
        let shutdown_backend = backend_handle.clone();
        cx.on_action(move |_: &Quit, cx| {
            shutdown_backend.send(Command::Shutdown);
            cx.quit();
        });
        cx.bind_keys([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("ctrl-q", Quit, None),
            KeyBinding::new("cmd-k", FocusSearch, None),
            KeyBinding::new("ctrl-k", FocusSearch, None),
            KeyBinding::new("cmd-r", Refresh, None),
            KeyBinding::new("f5", Refresh, None),
            KeyBinding::new("cmd-shift-r", QuickRun, None),
            KeyBinding::new("ctrl-shift-r", QuickRun, None),
            KeyBinding::new("escape", Escape, None),
        ]);
        cx.set_menus([Menu::new("Bareboot").items([MenuItem::action("Quit Bareboot", Quit)])]);

        let bounds = Bounds::centered(None, size(px(1040.), px(700.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("Bareboot".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(15.), px(14.))),
            }),
            window_min_size: Some(size(px(760.), px(520.))),
            window_decorations: Some(WindowDecorations::Client),
            app_id: Some("ch.ausfuhrli.bareboot.desktop".into()),
            ..Default::default()
        };

        let close_backend = backend_handle.clone();
        cx.on_window_closed(move |cx, _| {
            if cx.windows().is_empty() {
                close_backend.send(Command::Shutdown);
                cx.quit();
            }
        })
        .detach();

        let window = cx
            .open_window(options, |window, cx| {
                cx.new(|cx| AppView::new(window, cx, backend_handle, updates, prefs))
            })
            .expect("cannot open the window");

        cx.spawn(async move |cx| {
            while let Some(msg) = messages.next().await {
                if msg == Message::Show {
                    let _ = window.update(cx, |_, window, cx| {
                        cx.activate(true);
                        window.activate_window();
                    });
                }
            }
        })
        .detach();
        cx.activate(true);
    });
}
