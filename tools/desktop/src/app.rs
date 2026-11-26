use std::time::Duration;

use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::{
    Context, Entity, FocusHandle, Focusable, FontWeight, IntoElement, MouseButton, Render, Window,
    div, px,
};

use crate::backend::{Backend, Command};
use crate::design::{ButtonKind, ChipTone, FONT, Palette, Ui, eid};
use crate::dialogs::Dialog;
use crate::model::{ConnectionStatus, DisplayState, LogKind, Model, Update};
use crate::prefs::Prefs;

gpui_kit::actions!([FocusSearch, Refresh, QuickRun, Escape]);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Apps,
    Distros,
    Ports,
    Logs,
    Settings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppFilter {
    All,
    Pinned,
    Gui,
    Terminal,
    Hidden,
}

impl AppFilter {
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Pinned => "Pinned",
            Self::Gui => "GUI",
            Self::Terminal => "Terminal",
            Self::Hidden => "Hidden",
        }
    }
}

pub struct Toast {
    pub text: String,
    pub error: bool,
    pub id: u64,
}

pub struct AppView {
    pub model: Model,
    pub backend: Backend,
    pub prefs: Prefs,
    pub page: Page,
    pub app_filter: AppFilter,
    pub selected_distro: Option<String>,
    pub device_menu_open: bool,
    pub search_input: Entity<InputState>,
    pub fwd_local_input: Entity<InputState>,
    pub fwd_remote_input: Entity<InputState>,
    pub log_kind: LogKind,
    pub dialog: Option<Dialog>,
    pub toast: Option<Toast>,
    pub toast_counter: u64,
    pub focus: FocusHandle,
}

impl AppView {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        backend: Backend,
        mut updates: UnboundedReceiver<Update>,
        prefs: Prefs,
    ) -> Self {
        let search_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Search apps or commands… (Cmd+K)")
        });
        cx.subscribe_in(
            &search_input,
            window,
            |_this, _, event: &InputEvent, _window, cx| match event {
                InputEvent::Change | InputEvent::Focus | InputEvent::Blur => cx.notify(),
                InputEvent::PressEnter { .. } => {}
            },
        )
        .detach();

        let fwd_local_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Local port (e.g. 8080)"));
        let fwd_remote_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Phone port (e.g. 8080)"));

        cx.spawn(async move |this, cx| {
            while let Some(update) = updates.next().await {
                if this
                    .update(cx, |this, cx| this.on_update(update, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        let selected_distro = prefs.selected_distro.clone();
        Self {
            model: Model::default(),
            backend,
            prefs,
            page: Page::Apps,
            app_filter: AppFilter::All,
            selected_distro,
            device_menu_open: false,
            search_input,
            fwd_local_input,
            fwd_remote_input,
            log_kind: LogKind::Daemon,
            dialog: None,
            toast: None,
            toast_counter: 0,
            focus: cx.focus_handle(),
        }
    }

    pub fn active_distro(&self) -> Option<String> {
        if let Some(sel) = &self.selected_distro
            && self.model.snapshot.images.iter().any(|i| &i.name == sel)
        {
            return Some(sel.clone());
        }
        if let Some(running) = self.model.snapshot.running.first() {
            return Some(running.name.clone());
        }
        self.model
            .snapshot
            .images
            .first()
            .map(|i| i.name.clone())
    }

    pub fn select_distro(&mut self, name: String, cx: &mut Context<Self>) {
        self.selected_distro = Some(name.clone());
        self.prefs.selected_distro = Some(name.clone());
        self.prefs.save();
        if self.model.is_running(&name) && !self.model.apps.contains_key(&name) {
            self.backend.send(Command::ScanApps {
                distro: name.clone(),
            });
        }
        if self.page == Page::Logs && self.log_kind != LogKind::Daemon {
            self.load_current_log(cx);
        }
        cx.notify();
    }

    pub fn show_toast(&mut self, text: impl Into<String>, error: bool, cx: &mut Context<Self>) {
        self.toast_counter += 1;
        let id = self.toast_counter;
        self.toast = Some(Toast {
            text: text.into(),
            error,
            id,
        });
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(4)).await;
            let _ = this.update(cx, |this, cx| {
                if this.toast.as_ref().is_some_and(|t| t.id == id) {
                    this.toast = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn load_current_log(&mut self, cx: &mut Context<Self>) {
        self.model.log_loading = true;
        self.backend.send(Command::LoadLog {
            kind: self.log_kind,
            distro: self.active_distro(),
        });
        cx.notify();
    }

    pub fn open_terminal_for(&mut self, distro: &str, cx: &mut Context<Self>) {
        let Some(img) = self.model.image(distro).cloned() else {
            return;
        };
        if !self.model.is_running(distro) {
            self.show_toast(format!("Starting {distro} first…"), false, cx);
            self.backend.send(Command::StartDistro {
                name: img.name.clone(),
                image_path: img.path.clone(),
            });
            return;
        }
        self.backend.send(Command::OpenTerminal {
            distro: img.name,
            via_ssh: self.prefs.terminal_via_ssh,
            ssh_port: img.ssh_port,
            username: img.username,
            guest_cmd: None,
        });
    }

    pub fn sync_backend_settings(&self) {
        self.backend.send(Command::UpdateSettings {
            adb_path: self.prefs.adb_path.clone(),
            terminal: self.prefs.terminal.clone(),
            cmd_to_ctrl: self.prefs.cmd_to_ctrl,
            auto_reconnect: self.prefs.auto_reconnect_display,
            auto_forward: self.prefs.auto_forward_ports,
        });
    }

    pub fn pick_file_to_push(&mut self, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        cx.spawn(async move |_this, cx| {
            let picked = cx
                .background_executor()
                .spawn(async {
                    rfd::FileDialog::new()
                        .set_title("Send file to /mnt/android")
                        .pick_file()
                })
                .await;
            if let Some(path) = picked {
                backend.send(Command::PushFile { path });
            }
        })
        .detach();
    }

    pub fn pick_adb_binary(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let picked = cx
                .background_executor()
                .spawn(async {
                    rfd::FileDialog::new()
                        .set_title("Locate adb executable")
                        .pick_file()
                })
                .await;
            if let Some(path) = picked {
                let _ = this.update(cx, |this, cx| {
                    this.prefs.adb_path = Some(path.to_string_lossy().into_owned());
                    this.prefs.save();
                    this.sync_backend_settings();
                    this.show_toast("Updated ADB path", false, cx);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn on_update(&mut self, update: Update, cx: &mut Context<Self>) {
        match update {
            Update::AdbResolved(path) => {
                self.model.adb_path = path;
            }
            Update::DevicesChanged {
                devices,
                active_serial,
            } => {
                self.model.devices = devices;
                self.model.active_serial = active_serial;
            }
            Update::Snapshot { status, snapshot } => {
                self.model.status = status;
                if let Some(snap) = snapshot {
                    self.model.snapshot = snap;
                    if self.selected_distro.is_none() {
                        self.selected_distro = self.active_distro();
                    }
                }
            }
            Update::AppsLoaded { distro, apps } => {
                self.model.apps.insert(distro, apps);
            }
            Update::AppsScanning { distro, scanning } => {
                self.model.scanning_apps.insert(distro, scanning);
            }
            Update::DisplayChanged { distro, session } => {
                self.model.displays.insert(distro, session);
            }
            Update::ForwardsChanged(fwds) => {
                self.model.forwards = fwds;
            }
            Update::LogLoaded { text } => {
                self.model.log_text = text;
                self.model.log_loading = false;
            }
            Update::DistroBusy { distro, action } => {
                if let Some(label) = action {
                    self.model.busy_distros.insert(distro, label);
                } else {
                    self.model.busy_distros.remove(&distro);
                }
            }
            Update::Toast { message, error } => {
                self.show_toast(message, error, cx);
            }
        }
        cx.notify();
    }

    fn sidebar(&mut self, ui: &Ui, _window: &mut Window, cx: &mut Context<Self>) -> gpui_kit::Div {
        let p = ui.p;
        let running_count = self.model.snapshot.running.len();
        let fwd_count = self.model.forwards.len();
        let active_dev = self.model.active_device().cloned();
        let devices = self.model.devices.clone();

        let (dot_color, dev_title, dev_sub) = match &self.model.status {
            ConnectionStatus::Ready => {
                let d = active_dev.as_ref();
                (
                    p.primary,
                    d.map_or_else(|| "Connected".into(), |d| d.display_name()),
                    d.map_or_else(
                        String::new,
                        |d| format!("{} · {}", d.transport.label(), short_serial(&d.serial)),
                    ),
                )
            }
            ConnectionStatus::AdbMissing => {
                (p.error, "ADB not found".into(), "Set path in Settings".into())
            }
            ConnectionStatus::NoDevice => (
                p.on_surface_variant,
                "No phone".into(),
                "Plug USB or Wi-Fi ADB".into(),
            ),
            ConnectionStatus::Unauthorized(st) => (p.amber, "Phone locked".into(), st.clone()),
            ConnectionStatus::DaemonUnavailable(_) => {
                let name = active_dev
                    .as_ref()
                    .map_or_else(|| "Phone".into(), |d| d.display_name());
                (p.amber, name, "Daemon unavailable".into())
            }
        };

        let nav_items: &[(Page, &str, &str, Option<String>)] = &[
            (Page::Apps, "Apps", "apps", None),
            (
                Page::Distros,
                "Distros",
                "layers",
                (running_count > 0).then(|| running_count.to_string()),
            ),
            (
                Page::Ports,
                "Ports & SSH",
                "network",
                (fwd_count > 0).then(|| fwd_count.to_string()),
            ),
            (Page::Logs, "Logs", "log", None),
            (Page::Settings, "Settings", "settings", None),
        ];

        let active_distro = self.active_distro();
        let active_display = active_distro
            .as_deref()
            .map(|d| self.model.display(d))
            .unwrap_or_default();
        let win_total = self.model.total_windows();
        let connected_count = self.model.connected_displays();

        let top_pad = if cfg!(target_os = "macos") {
            px(44.)
        } else {
            px(16.)
        };

        let mut top = div()
            .flex()
            .flex_col()
            .px(px(14.))
            .pt(top_pad)
            .gap(px(14.));

        top = top.child(
            div()
                .px(px(6.))
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(ui.shape_icon(true, "terminal", 30., p.primary, p.on_primary))
                        .child(
                            div()
                                .text_size(px(16.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(p.on_surface)
                                .child("Bareboot"),
                        ),
                )
                .child(
                    ui.icon_button("sb-refresh", "refresh", "Refresh")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.backend.send(Command::RefreshNow);
                            if let Some(d) = this.active_distro()
                                && this.model.is_running(&d)
                            {
                                this.backend.send(Command::ScanApps { distro: d });
                            }
                            cx.notify();
                        })),
                ),
        );

        let multi = devices.len() > 1;
        let mut device_box = div()
            .id("device-switcher")
            .p(px(12.))
            .rounded(px(20.))
            .bg(p.surface_container)
            .flex()
            .flex_col()
            .gap(px(8.));
        if multi {
            device_box = device_box
                .cursor_pointer()
                .hover(move |s| s.bg(p.surface_high))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.device_menu_open = !this.device_menu_open;
                    cx.notify();
                }));
        }
        device_box = device_box.child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(ui.dot(dot_color))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(p.on_surface)
                                .truncate()
                                .child(dev_title),
                        )
                        .child(
                            div()
                                .text_size(px(10.5))
                                .text_color(p.on_surface_variant)
                                .truncate()
                                .child(dev_sub),
                        ),
                )
                .when(multi, |this| {
                    this.child(ui.chip(&format!("{}", devices.len()), ChipTone::Surface))
                }),
        );

        if self.device_menu_open && devices.len() > 1 {
            let mut list = div().pt(px(4.)).flex().flex_col().gap(px(3.));
            for dev in devices {
                let serial = dev.serial.clone();
                let active = self.model.active_serial.as_deref() == Some(serial.as_str());
                let icon_name = match dev.transport {
                    crate::model::Transport::Usb => "usb",
                    crate::model::Transport::Wifi => "wifi",
                };
                list = list.child(
                    div()
                        .id(eid(format!("dev-{serial}")))
                        .px(px(10.))
                        .py(px(7.))
                        .rounded_full()
                        .cursor_pointer()
                        .bg(if active {
                            p.secondary_container
                        } else {
                            p.surface_high
                        })
                        .hover(move |s| s.bg(p.surface_highest))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(6.))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.device_menu_open = false;
                            this.prefs.preferred_serial = Some(serial.clone());
                            this.prefs.save();
                            this.backend.send(Command::SelectDevice(serial.clone()));
                            cx.notify();
                        }))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(7.))
                                .min_w_0()
                                .child(ui.icon(
                                    icon_name,
                                    13.,
                                    if active {
                                        p.on_secondary_container
                                    } else {
                                        p.on_surface_variant
                                    },
                                ))
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .font_weight(if active {
                                            FontWeight::SEMIBOLD
                                        } else {
                                            FontWeight::NORMAL
                                        })
                                        .text_color(if active {
                                            p.on_secondary_container
                                        } else {
                                            p.on_surface
                                        })
                                        .truncate()
                                        .child(format!(
                                            "{} ({})",
                                            dev.display_name(),
                                            dev.transport.label()
                                        )),
                                ),
                        )
                        .when(active, |s| {
                            s.child(ui.icon("check", 13., p.on_secondary_container))
                        }),
                );
            }
            device_box = device_box.child(list);
        }
        top = top.child(device_box);

        let mut nav = div().flex().flex_col().gap(px(4.)).pt(px(2.));
        for (page, label, icon_name, badge) in nav_items {
            let page = *page;
            let active = self.page == page;
            let fg = if active {
                p.on_secondary_container
            } else {
                p.on_surface_variant
            };
            nav = nav.child(
                div()
                    .id(eid(format!("nav-{label}")))
                    .h(px(44.))
                    .px(px(14.))
                    .rounded_full()
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_between()
                    .when(active, |s| s.bg(p.secondary_container))
                    .when(!active, |s| s.hover(move |h| h.bg(p.surface_container)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.page = page;
                        if page == Page::Logs && this.model.log_text.is_empty() {
                            this.load_current_log(cx);
                        }
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(11.))
                            .child(ui.icon(icon_name, 17., fg))
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(if active {
                                        FontWeight::SEMIBOLD
                                    } else {
                                        FontWeight::MEDIUM
                                    })
                                    .text_color(fg)
                                    .child(*label),
                            ),
                    )
                    .when_some(badge.clone(), |s, b| {
                        s.child(ui.chip(
                            &b,
                            if active {
                                ChipTone::Primary
                            } else {
                                ChipTone::Surface
                            },
                        ))
                    }),
            );
        }
        top = top.child(nav);

        let (disp_dot, disp_label) = if connected_count > 0 {
            (
                p.primary,
                if win_total == 0 {
                    "Display ready".into()
                } else if win_total == 1 {
                    "1 window open".into()
                } else {
                    format!("{win_total} windows open")
                },
            )
        } else {
            match &active_display.state {
                DisplayState::Connecting => (p.amber, "Connecting…".into()),
                DisplayState::Reconnecting => (p.amber, "Reconnecting…".into()),
                DisplayState::Error(_) => (p.error, "Display error".into()),
                _ => (p.on_surface_variant, "Display idle".into()),
            }
        };

        let mut disp_card = div()
            .p(px(14.))
            .rounded(px(22.))
            .bg(p.surface_container)
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(ui.dot(disp_dot))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(p.on_surface)
                                    .child(disp_label),
                            ),
                    )
                    .child(ui.icon("monitor", 15., p.on_surface_variant)),
            );

        if let Some(distro) = active_distro {
            let is_running = self.model.is_running(&distro);
            let is_connected = matches!(
                active_display.state,
                DisplayState::Connected | DisplayState::Connecting | DisplayState::Reconnecting
            );
            let distro_btn = distro.clone();
            disp_card = disp_card.child(
                ui.compact_button(
                    "sb-display-toggle",
                    if is_connected {
                        "Disconnect"
                    } else {
                        "Connect Display"
                    },
                    Some("monitor"),
                    if is_connected {
                        ButtonKind::Tonal
                    } else {
                        ButtonKind::Primary
                    },
                )
                .w_full()
                .disabled(!is_running && !is_connected)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if is_connected {
                        this.backend.send(Command::DisconnectDisplay {
                            distro: distro_btn.clone(),
                        });
                    } else {
                        this.backend.send(Command::ConnectDisplay {
                            distro: distro_btn.clone(),
                        });
                    }
                    cx.notify();
                })),
            );
        }

        div()
            .w(px(244.))
            .flex_none()
            .h_full()
            .bg(p.surface_low)
            .flex()
            .flex_col()
            .justify_between()
            .child(top)
            .child(div().p(px(14.)).child(disp_card))
    }
}

fn short_serial(serial: &str) -> String {
    if serial.len() > 16 {
        format!("{}…", &serial[..14])
    } else {
        serial.to_owned()
    }
}

impl Drop for AppView {
    fn drop(&mut self) {
        self.backend.send(Command::Shutdown);
    }
}

impl Focusable for AppView {
    fn focus_handle(&self, _: &gpui_kit::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scheme = self.prefs.theme.resolve(window.appearance());
        let p = Palette::of(scheme);
        let ui = Ui::new(p);

        let page_body = match self.page {
            Page::Apps => self.page_apps(&ui, window, cx),
            Page::Distros => self.page_distros(&ui, window, cx),
            Page::Ports => self.page_ports(&ui, window, cx),
            Page::Logs => self.page_logs(&ui, window, cx),
            Page::Settings => self.page_settings(&ui, window, cx),
        };

        let mut main_area = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(p.surface);

        #[cfg(not(target_os = "macos"))]
        {
            let bar = div().h(px(36.)).w_full().flex_none().bg(p.surface);
            main_area = main_area.child(crate::chrome::caption_bar(bar, &ui, window));
        }

        #[cfg(target_os = "macos")]
        {
            main_area = main_area.child(
                div()
                    .h(px(18.))
                    .w_full()
                    .flex_none()
                    .window_control_area(gpui_kit::WindowControlArea::Drag)
                    .on_mouse_down(MouseButton::Left, |event, window, _| {
                        if event.click_count == 2 {
                            window.zoom_window();
                        } else {
                            window.start_window_move();
                        }
                    }),
            );
        }

        main_area = main_area.child(
            div()
                .id("main-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px(px(28.))
                .pt(px(12.))
                .pb(px(32.))
                .child(page_body),
        );

        let toast_el = self.toast.as_ref().map(|toast| {
            let bg = if toast.error {
                p.error_container
            } else {
                p.secondary_container
            };
            let fg = if toast.error {
                p.on_error_container
            } else {
                p.on_secondary_container
            };
            div()
                .absolute()
                .right(px(24.))
                .bottom(px(24.))
                .px(px(18.))
                .py(px(12.))
                .rounded_full()
                .bg(bg)
                .text_size(px(12.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(fg)
                .child(toast.text.clone())
        });

        let dialog_el = self.render_dialog(&ui, window, cx);

        div()
            .size_full()
            .relative()
            .font_family(FONT)
            .text_color(p.on_surface)
            .bg(p.surface)
            .flex()
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                this.page = Page::Apps;
                let fh = this.search_input.read(cx).focus_handle(cx);
                fh.focus(window, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Refresh, _, cx| {
                this.backend.send(Command::RefreshNow);
                if let Some(d) = this.active_distro()
                    && this.model.is_running(&d)
                {
                    this.backend.send(Command::ScanApps { distro: d });
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &QuickRun, window, cx| {
                if let Some(d) = this.active_distro() {
                    this.open_quick_run(d, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Escape, _, cx| {
                if this.dialog.is_some() {
                    this.close_dialog(cx);
                } else if this.device_menu_open {
                    this.device_menu_open = false;
                    cx.notify();
                }
            }))
            .child(self.sidebar(&ui, window, cx))
            .child(main_area)
            .when_some(toast_el, |this, el| this.child(el))
            .when_some(dialog_el, |this, el| this.child(el))
    }
}
