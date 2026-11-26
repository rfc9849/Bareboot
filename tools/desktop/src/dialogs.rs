use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::base::{FocusTrapElement as _, Input};
use gpui_kit::prelude::*;
use gpui_kit::{Context, Entity, Focusable as _, FontWeight, MouseButton, Window, div, px};

use crate::adb::{format_port_forwards, parse_port_forwards};
use crate::app::AppView;
use crate::backend::Command;
use crate::design::{ButtonKind, MONO, Ui, eid};
use crate::model::DistroImage;
use crate::prefs::CustomShortcut;

pub struct QuickRunDialog {
    pub distro: String,
    pub command: Entity<InputState>,
    pub terminal: bool,
}

pub struct CustomAppDialog {
    pub distro: String,
    pub name: Entity<InputState>,
    pub exec: Entity<InputState>,
    pub terminal: bool,
    pub error: Option<String>,
}

pub struct ConfigureDistroDialog {
    pub original: DistroImage,
    pub is_running: bool,
    pub os_label: Entity<InputState>,
    pub username: Entity<InputState>,
    pub ssh_port: Entity<InputState>,
    pub forwards: Entity<InputState>,
    pub keep_awake: bool,
    pub init_system: bool,
    pub isolated_net: bool,
    pub gpu_enabled: bool,
    pub error: Option<String>,
}

pub enum Dialog {
    QuickRun(QuickRunDialog),
    AddCustomApp(CustomAppDialog),
    ConfigureDistro(Box<ConfigureDistroDialog>),
}

impl AppView {
    pub fn make_input(
        &self,
        value: &str,
        placeholder: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        let value = value.to_owned();
        let placeholder = placeholder.to_owned();
        let state = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .default_value(value)
        });
        cx.subscribe_in(
            &state,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.submit_dialog(window, cx),
                InputEvent::Change | InputEvent::Focus | InputEvent::Blur => cx.notify(),
            },
        )
        .detach();
        state
    }

    pub fn open_quick_run(&mut self, distro: String, window: &mut Window, cx: &mut Context<Self>) {
        let command = self.make_input("", "e.g. firefox, vkcube-wayland, alacritty", window, cx);
        let focus = command.read(cx).focus_handle(cx);
        self.dialog = Some(Dialog::QuickRun(QuickRunDialog {
            distro,
            command,
            terminal: false,
        }));
        focus.focus(window, cx);
        cx.notify();
    }

    pub fn open_add_custom_app(
        &mut self,
        distro: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = self.make_input("", "e.g. VS Code", window, cx);
        let exec = self.make_input("", "e.g. code --ozone-platform=wayland", window, cx);
        let focus = name.read(cx).focus_handle(cx);
        self.dialog = Some(Dialog::AddCustomApp(CustomAppDialog {
            distro,
            name,
            exec,
            terminal: false,
            error: None,
        }));
        focus.focus(window, cx);
        cx.notify();
    }

    pub fn open_configure_distro(
        &mut self,
        img: DistroImage,
        is_running: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let os_label = self.make_input(&img.os_label, "Debian / Arch Linux", window, cx);
        let username = self.make_input(&img.username, "user", window, cx);
        let ssh_str = if img.ssh_port > 0 {
            img.ssh_port.to_string()
        } else {
            String::new()
        };
        let ssh_port = self.make_input(&ssh_str, "2222 (0 = off)", window, cx);
        let fwd_str = format_port_forwards(&img.forwards);
        let forwards = self.make_input(&fwd_str, "8080:80, 3000", window, cx);
        self.dialog = Some(Dialog::ConfigureDistro(Box::new(ConfigureDistroDialog {
            keep_awake: img.keep_awake,
            init_system: img.init == "system",
            isolated_net: img.isolated_net,
            gpu_enabled: img.gpu_enabled,
            original: img,
            is_running,
            os_label,
            username,
            ssh_port,
            forwards,
            error: None,
        })));
        cx.notify();
    }

    pub fn close_dialog(&mut self, cx: &mut Context<Self>) {
        self.dialog = None;
        cx.notify();
    }

    pub fn submit_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.dialog.as_mut() else {
            return;
        };
        match dialog {
            Dialog::QuickRun(d) => {
                let cmd = d.command.read(cx).value().trim().to_owned();
                if cmd.is_empty() {
                    return;
                }
                let distro = d.distro.clone();
                let terminal = d.terminal;
                self.dialog = None;
                self.backend.send(Command::LaunchApp {
                    distro,
                    name: cmd.split_whitespace().next().unwrap_or(&cmd).to_owned(),
                    exec: cmd,
                    terminal,
                });
                cx.notify();
            }
            Dialog::AddCustomApp(d) => {
                let name = d.name.read(cx).value().trim().to_owned();
                let exec = d.exec.read(cx).value().trim().to_owned();
                if name.is_empty() || exec.is_empty() {
                    d.error = Some("Both name and command are required.".into());
                    cx.notify();
                    return;
                }
                let id = format!(
                    "custom-{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |t| t.as_millis())
                );
                let shortcut = CustomShortcut {
                    id,
                    distro: d.distro.clone(),
                    name: name.clone(),
                    exec,
                    terminal: d.terminal,
                };
                self.prefs.custom_apps.push(shortcut);
                self.prefs.save();
                self.dialog = None;
                self.show_toast(format!("Added shortcut \"{name}\""), false, cx);
                cx.notify();
            }
            Dialog::ConfigureDistro(d) => {
                if d.is_running {
                    d.error = Some("Stop the distribution first to change startup settings.".into());
                    cx.notify();
                    return;
                }
                let ssh_raw = d.ssh_port.read(cx).value().trim().to_owned();
                let ssh_port = if ssh_raw.is_empty() {
                    0
                } else {
                    match ssh_raw.parse::<u16>() {
                        Ok(p) => p,
                        Err(_) => {
                            d.error = Some("Invalid SSH port (use 0 or 1024..65535).".into());
                            cx.notify();
                            return;
                        }
                    }
                };
                let mut updated = d.original.clone();
                updated.os_label = d.os_label.read(cx).value().trim().to_owned();
                updated.username = d.username.read(cx).value().trim().to_owned();
                updated.keep_awake = d.keep_awake;
                updated.init = if d.init_system { "system" } else { "none" }.into();
                updated.isolated_net = d.isolated_net;
                updated.ssh_port = ssh_port;
                updated.forwards = parse_port_forwards(&d.forwards.read(cx).value());
                let gpu_changed = d.gpu_enabled != d.original.gpu_enabled;
                let gpu_enabled = d.gpu_enabled;
                let distro_name = updated.name.clone();
                self.dialog = None;
                self.backend.send(Command::ConfigureImage(updated));
                if gpu_changed {
                    self.backend.send(Command::ToggleGpu {
                        name: distro_name,
                        enabled: gpu_enabled,
                    });
                }
                cx.notify();
            }
        }
    }

    pub fn render_dialog(
        &mut self,
        ui: &Ui,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui_kit::Stateful<gpui_kit::Div>> {
        let dialog = self.dialog.as_ref()?;
        let p = ui.p;

        let content = match dialog {
            Dialog::QuickRun(d) => {
                let distro = d.distro.clone();
                let terminal = d.terminal;
                let cmd_input = d.command.clone();
                div()
                    .flex()
                    .flex_col()
                    .gap(px(16.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(18.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(p.on_surface)
                                    .child(format!("Run in {distro}")),
                            )
                            .child(ui.icon_button("dlg-close", "close", "Close").on_click(
                                cx.listener(|this, _, _, cx| this.close_dialog(cx)),
                            )),
                    )
                    .child(self.labeled_input(ui, "Command", &cmd_input, true))
                    .child(
                        ui.segmented_row(0, 1).px(px(14.)).py(px(12.)).child(
                            self.toggle_row(
                                ui,
                                "dlg-run-term",
                                "Run in host terminal",
                                "Off = launch as Wayland GUI app with auto-display",
                                terminal,
                                cx,
                                |this, checked, cx| {
                                    if let Some(Dialog::QuickRun(d)) = &mut this.dialog {
                                        d.terminal = checked;
                                        cx.notify();
                                    }
                                },
                            ),
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.))
                            .pt(px(4.))
                            .child(
                                ui.button("dlg-cancel", "Cancel", None, ButtonKind::Ghost)
                                    .on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))),
                            )
                            .child(
                                ui.button("dlg-submit", "Run", Some("play"), ButtonKind::Primary)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.submit_dialog(window, cx)
                                    })),
                            ),
                    )
            }
            Dialog::AddCustomApp(d) => {
                let distro = d.distro.clone();
                let terminal = d.terminal;
                let name_input = d.name.clone();
                let exec_input = d.exec.clone();
                let err = d.error.clone();
                div()
                    .flex()
                    .flex_col()
                    .gap(px(14.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(18.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(p.on_surface)
                                    .child(format!("Add shortcut · {distro}")),
                            )
                            .child(ui.icon_button("dlg-close", "close", "Close").on_click(
                                cx.listener(|this, _, _, cx| this.close_dialog(cx)),
                            )),
                    )
                    .child(self.labeled_input(ui, "Display name", &name_input, false))
                    .child(self.labeled_input(ui, "Command", &exec_input, true))
                    .child(
                        ui.segmented_row(0, 1).px(px(14.)).py(px(12.)).child(
                            self.toggle_row(
                                ui,
                                "dlg-custom-term",
                                "Open in host terminal",
                                "For CLI / TUI tools instead of Wayland GUI apps",
                                terminal,
                                cx,
                                |this, checked, cx| {
                                    if let Some(Dialog::AddCustomApp(d)) = &mut this.dialog {
                                        d.terminal = checked;
                                        cx.notify();
                                    }
                                },
                            ),
                        ),
                    )
                    .when_some(err, |this, msg| {
                        this.child(div().text_size(px(12.)).text_color(p.error).child(msg))
                    })
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.))
                            .pt(px(4.))
                            .child(
                                ui.button("dlg-cancel", "Cancel", None, ButtonKind::Ghost)
                                    .on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))),
                            )
                            .child(
                                ui.button(
                                    "dlg-save",
                                    "Add shortcut",
                                    Some("plus"),
                                    ButtonKind::Primary,
                                )
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.submit_dialog(window, cx)
                                })),
                            ),
                    )
            }
            Dialog::ConfigureDistro(d) => {
                let name = d.original.name.clone();
                let is_running = d.is_running;
                let os_input = d.os_label.clone();
                let user_input = d.username.clone();
                let ssh_input = d.ssh_port.clone();
                let fwd_input = d.forwards.clone();
                let keep_awake = d.keep_awake;
                let init_system = d.init_system;
                let isolated_net = d.isolated_net;
                let gpu_enabled = d.gpu_enabled;
                let gpu_avail = d.original.gpu_available;
                let err = d.error.clone();
                div()
                    .flex()
                    .flex_col()
                    .gap(px(14.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(18.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(p.on_surface)
                                    .child(format!("Settings · {name}")),
                            )
                            .child(ui.icon_button("dlg-close", "close", "Close").on_click(
                                cx.listener(|this, _, _, cx| this.close_dialog(cx)),
                            )),
                    )
                    .when(is_running, |this| {
                        this.child(
                            div()
                                .px(px(14.))
                                .py(px(10.))
                                .rounded(px(16.))
                                .bg(p.amber_container)
                                .text_color(p.amber)
                                .text_size(px(12.))
                                .child("Stop the distribution first to change startup settings."),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .gap(px(10.))
                            .child(div().flex_1().child(self.labeled_input(
                                ui,
                                "OS label",
                                &os_input,
                                false,
                            )))
                            .child(div().flex_1().child(self.labeled_input(
                                ui,
                                "Default user",
                                &user_input,
                                true,
                            ))),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(10.))
                            .child(div().w(px(130.)).child(self.labeled_input(
                                ui,
                                "SSH port",
                                &ssh_input,
                                true,
                            )))
                            .child(div().flex_1().child(self.labeled_input(
                                ui,
                                "Port forwards",
                                &fwd_input,
                                true,
                            ))),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .child(ui.segmented_row(0, 4).px(px(14.)).py(px(10.)).child(
                                self.toggle_row(
                                    ui,
                                    "cfg-gpu",
                                    "GPU acceleration",
                                    if gpu_avail {
                                        "Vulkan & OpenGL on the phone GPU"
                                    } else {
                                        "GPU bundle unavailable on phone"
                                    },
                                    gpu_enabled,
                                    cx,
                                    |this, checked, cx| {
                                        if let Some(Dialog::ConfigureDistro(d)) = &mut this.dialog {
                                            d.gpu_enabled = checked;
                                            cx.notify();
                                        }
                                    },
                                ),
                            ))
                            .child(ui.segmented_row(1, 4).px(px(14.)).py(px(10.)).child(
                                self.toggle_row(
                                    ui,
                                    "cfg-init",
                                    "Boot distribution init",
                                    "Run systemd or OpenRC as PID 1",
                                    init_system,
                                    cx,
                                    |this, checked, cx| {
                                        if let Some(Dialog::ConfigureDistro(d)) = &mut this.dialog {
                                            d.init_system = checked;
                                            cx.notify();
                                        }
                                    },
                                ),
                            ))
                            .child(ui.segmented_row(2, 4).px(px(14.)).py(px(10.)).child(
                                self.toggle_row(
                                    ui,
                                    "cfg-net",
                                    "Isolated network",
                                    "Private IPv4 uplink via slirp4netns",
                                    isolated_net,
                                    cx,
                                    |this, checked, cx| {
                                        if let Some(Dialog::ConfigureDistro(d)) = &mut this.dialog {
                                            d.isolated_net = checked;
                                            cx.notify();
                                        }
                                    },
                                ),
                            ))
                            .child(ui.segmented_row(3, 4).px(px(14.)).py(px(10.)).child(
                                self.toggle_row(
                                    ui,
                                    "cfg-awake",
                                    "Keep phone awake",
                                    "Hold a wakelock while running",
                                    keep_awake,
                                    cx,
                                    |this, checked, cx| {
                                        if let Some(Dialog::ConfigureDistro(d)) = &mut this.dialog {
                                            d.keep_awake = checked;
                                            cx.notify();
                                        }
                                    },
                                ),
                            )),
                    )
                    .when_some(err, |this, msg| {
                        this.child(div().text_size(px(12.)).text_color(p.error).child(msg))
                    })
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.))
                            .pt(px(4.))
                            .child(
                                ui.button("dlg-cancel", "Cancel", None, ButtonKind::Ghost)
                                    .on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))),
                            )
                            .child(
                                ui.button(
                                    "dlg-save",
                                    "Save",
                                    Some("check"),
                                    ButtonKind::Primary,
                                )
                                .disabled(is_running)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.submit_dialog(window, cx)
                                })),
                            ),
                    )
            }
        };

        let modal = div()
            .id("dialog-scrim")
            .absolute()
            .inset_0()
            .bg(p.scrim)
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                this.close_dialog(cx);
            }))
            .child(
                div()
                    .id("dialog-card")
                    .w(px(480.))
                    .max_w(px(540.))
                    .p(px(24.))
                    .rounded(px(28.))
                    .bg(p.surface_high)
                    .focus_trap("dialog-trap", &self.focus)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(content),
            );

        Some(modal)
    }

    pub fn labeled_input(
        &self,
        ui: &Ui,
        label: &str,
        state: &Entity<InputState>,
        mono: bool,
    ) -> gpui_kit::Div {
        let p = ui.p;
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .px(px(4.))
                    .text_size(px(11.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(p.on_surface_variant)
                    .child(label.to_owned()),
            )
            .child(
                div()
                    .h(px(40.))
                    .px(px(14.))
                    .rounded(px(14.))
                    .bg(p.surface_container)
                    .flex()
                    .items_center()
                    .text_size(px(12.5))
                    .text_color(p.on_surface)
                    .when(mono, |this| this.font_family(MONO))
                    .child(div().w_full().child(Input::new(state))),
            )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn toggle_row(
        &self,
        ui: &Ui,
        id: &str,
        title: &str,
        subtitle: &str,
        checked: bool,
        cx: &mut Context<Self>,
        on_change: impl Fn(&mut Self, bool, &mut Context<Self>) + 'static,
    ) -> gpui_kit::Div {
        let p = ui.p;
        let entity = cx.entity().downgrade();
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(14.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(p.on_surface)
                            .child(title.to_owned()),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(p.on_surface_variant)
                            .child(subtitle.to_owned()),
                    ),
            )
            .child(ui.switch(eid(id), checked, title).on_change(
                move |val, _, _, cx| {
                    let _ = entity.update(cx, |this, cx| on_change(this, val, cx));
                },
            ))
    }
}
