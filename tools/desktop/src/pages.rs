use gpui_kit::base::Input;
use gpui_kit::prelude::*;
use gpui_kit::{ClipboardItem, Context, FontWeight, MouseButton, Window, div, img, px, rgb};

use crate::adb::open_url;
use crate::app::{AppFilter, AppView, Page};
use crate::backend::Command;
use crate::design::{ButtonKind, ChipTone, MONO, ThemeMode, Ui, eid};
use crate::model::{ConnectionStatus, DesktopApp, DisplayState, LogKind};
use crate::prefs::{Prefs, TerminalApp};

impl AppView {
    pub fn page_apps(
        &mut self,
        ui: &Ui,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let p = ui.p;
        if !matches!(self.model.status, ConnectionStatus::Ready) {
            return self.connection_empty_state(ui, cx);
        }

        let images = self.model.snapshot.images.clone();
        if images.is_empty() {
            return ui
                .card()
                .p(px(28.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(
                    div()
                        .text_size(px(16.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(p.on_surface)
                        .child("No Linux distributions installed"),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(p.on_surface_variant)
                        .child("Install a distribution in the Bareboot Android app first, then it will appear here automatically."),
                );
        }

        let distro = self
            .active_distro()
            .unwrap_or_else(|| images[0].name.clone());
        let is_running = self.model.is_running(&distro);
        let scanning = self
            .model
            .scanning_apps
            .get(&distro)
            .copied()
            .unwrap_or(false);
        let busy_label = self.model.busy_distros.get(&distro).cloned();
        let display_session = self.model.display(&distro);
        let img_info = self.model.image(&distro).cloned();

        let active_os = img_info
            .as_ref()
            .map(|i| i.display_os().to_owned())
            .unwrap_or_else(|| distro.clone());
        let active_user = img_info
            .as_ref()
            .map(|i| i.username.clone())
            .unwrap_or_default();
        let active_gpu = img_info.as_ref().is_some_and(|i| i.gpu_running);
        let active_ssh = img_info.as_ref().map_or(0, |i| i.ssh_port);
        let scanned_count = self.model.apps.get(&distro).map_or(0, Vec::len);

        let mut root = div().flex().flex_col().gap(px(16.));

        let d_run = distro.clone();
        let d_add = distro.clone();
        let d_term = distro.clone();
        root = root.child(
            div()
                .flex()
                .items_end()
                .justify_between()
                .gap(px(12.))
                .child(ui.page_header(
                    "Apps",
                    Some(if is_running {
                        format!("{active_os} · Running · {scanned_count} installed")
                    } else {
                        format!("{active_os} · Stopped")
                    }),
                ))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            ui.compact_button(
                                "btn-quick-run",
                                "Run…",
                                Some("command"),
                                ButtonKind::Surface,
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_quick_run(d_run.clone(), window, cx);
                            })),
                        )
                        .child(
                            ui.compact_button(
                                "btn-add-custom",
                                "Shortcut",
                                Some("plus"),
                                ButtonKind::Surface,
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_add_custom_app(d_add.clone(), window, cx);
                            })),
                        )
                        .child(
                            ui.compact_button(
                                "btn-open-term",
                                "Terminal",
                                Some("terminal"),
                                ButtonKind::Primary,
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_terminal_for(&d_term, cx);
                            })),
                        ),
                ),
        );

        let hero_fg = if is_running {
            p.on_primary_container
        } else {
            p.on_surface
        };
        let hero_sub = if is_running {
            p.on_primary_container
        } else {
            p.on_surface_variant
        };
        let stop_d = distro.clone();
        let start_d = distro.clone();
        let start_p = img_info.as_ref().map(|i| i.path.clone()).unwrap_or_default();
        let busy_strip = busy_label.clone();

        let mut hero = ui
            .hero_card(is_running)
            .p(px(18.))
            .flex()
            .flex_col()
            .gap(px(12.));

        let hero_top = div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(14.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .child(ui.shape_icon(
                        is_running,
                        "terminal",
                        50.,
                        if is_running {
                            p.primary
                        } else {
                            p.secondary_container
                        },
                        if is_running {
                            p.on_primary
                        } else {
                            p.on_secondary_container
                        },
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(
                                        div()
                                            .text_size(px(18.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(hero_fg)
                                            .child(distro.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.5))
                                            .text_color(hero_sub)
                                            .opacity(0.85)
                                            .child(active_os),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(hero_sub)
                                            .opacity(0.8)
                                            .child(if active_user.is_empty() {
                                                if is_running {
                                                    "Running".into()
                                                } else {
                                                    "Stopped".into()
                                                }
                                            } else {
                                                format!(
                                                    "{active_user}@{distro} · {}",
                                                    if is_running { "Running" } else { "Stopped" }
                                                )
                                            }),
                                    )
                                    .when(active_gpu, |s| s.child(ui.chip("GPU", ChipTone::Tertiary)))
                                    .when(active_ssh > 0, |s| {
                                        s.child(ui.chip(
                                            &format!("SSH :{active_ssh}"),
                                            ChipTone::Secondary,
                                        ))
                                    }),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .when(is_running, |s| {
                        s.child(
                            ui.compact_button(
                                "strip-stop-distro",
                                busy_strip.as_deref().unwrap_or("Stop"),
                                Some("stop"),
                                ButtonKind::Tonal,
                            )
                            .disabled(busy_strip.is_some())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.backend.send(Command::StopDistro {
                                    name: stop_d.clone(),
                                });
                                cx.notify();
                            })),
                        )
                    })
                    .when(!is_running, |s| {
                        s.child(
                            ui.compact_button(
                                "strip-start-distro",
                                busy_strip.as_deref().unwrap_or("Start"),
                                Some("play"),
                                ButtonKind::Primary,
                            )
                            .disabled(busy_strip.is_some())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.backend.send(Command::StartDistro {
                                    name: start_d.clone(),
                                    image_path: start_p.clone(),
                                });
                                cx.notify();
                            })),
                        )
                    }),
            );

        if images.len() > 1 {
            let mut switcher = div().flex().items_center().flex_wrap().gap(px(6.));
            for img in &images {
                let name = img.name.clone();
                let selected = name == distro;
                let running = self.model.is_running(&name);
                switcher = switcher.child(
                    div()
                        .id(eid(format!("distro-pill-{name}")))
                        .px(px(12.))
                        .py(px(6.))
                        .rounded_full()
                        .cursor_pointer()
                        .bg(if selected {
                            p.primary
                        } else {
                            p.surface_container
                        })
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.select_distro(name.clone(), cx);
                        }))
                        .child(ui.dot(if selected {
                            p.on_primary
                        } else if running {
                            p.primary
                        } else {
                            p.on_surface_variant
                        }))
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(if selected { p.on_primary } else { p.on_surface })
                                .child(format!("{} ({})", img.display_os(), img.name)),
                        ),
                );
            }
            hero = hero.child(hero_top).child(switcher);
        } else {
            hero = hero.child(hero_top);
        }
        root = root.child(hero);

        let d_scan = distro.clone();
        let mut filter_bar = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .flex_1()
                    .h(px(42.))
                    .px(px(16.))
                    .rounded_full()
                    .bg(p.surface_container)
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(ui.icon("search", 16., p.on_surface_variant))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(13.))
                            .text_color(p.on_surface)
                            .child(Input::new(&self.search_input)),
                    ),
            );

        let has_hidden = !self.prefs.hidden_apps.is_empty();
        let mut filters = vec![
            AppFilter::All,
            AppFilter::Pinned,
            AppFilter::Gui,
            AppFilter::Terminal,
        ];
        if has_hidden || self.app_filter == AppFilter::Hidden {
            filters.push(AppFilter::Hidden);
        }
        for f in filters {
            let active = self.app_filter == f;
            filter_bar = filter_bar.child(
                div()
                    .id(eid(format!("filter-{}", f.label())))
                    .h(px(36.))
                    .px(px(14.))
                    .rounded_full()
                    .cursor_pointer()
                    .bg(if active {
                        p.secondary_container
                    } else {
                        p.surface_container
                    })
                    .hover(move |s| {
                        s.bg(if active {
                            p.secondary_container_hover
                        } else {
                            p.surface_high
                        })
                    })
                    .flex()
                    .items_center()
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(if active {
                        p.on_secondary_container
                    } else {
                        p.on_surface_variant
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.app_filter = f;
                        cx.notify();
                    }))
                    .child(f.label()),
            );
        }

        filter_bar = filter_bar.child(
            ui.icon_button("btn-rescan-apps", "refresh", "Rescan apps")
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.backend.send(Command::ScanApps {
                        distro: d_scan.clone(),
                    });
                    cx.notify();
                })),
        );
        root = root.child(filter_bar);

        if !display_session.windows.is_empty() {
            let d_disc = distro.clone();
            let mut wins = div().flex().flex_wrap().gap(px(6.));
            for w in &display_session.windows {
                wins = wins.child(ui.chip(&w.title, ChipTone::Primary));
            }
            root = root.child(
                ui.card()
                    .p(px(14.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .flex_wrap()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(p.primary)
                                    .child("Open windows"),
                            )
                            .child(wins),
                    )
                    .child(
                        ui.compact_button(
                            "btn-close-disp",
                            "Disconnect",
                            Some("stop"),
                            ButtonKind::Tonal,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.backend.send(Command::DisconnectDisplay {
                                distro: d_disc.clone(),
                            });
                            cx.notify();
                        })),
                    ),
            );
        }

        let query = self.search_input.read(cx).value().trim().to_lowercase();
        let mut combined: Vec<DesktopApp> = self
            .prefs
            .custom_apps
            .iter()
            .filter(|c| c.distro == distro)
            .map(|c| DesktopApp {
                id: c.id.clone(),
                name: c.name.clone(),
                comment: c.exec.clone(),
                exec: c.exec.clone(),
                icon_name: String::new(),
                icon_path: None,
                categories: vec!["Custom".into()],
                terminal: c.terminal,
                is_custom: true,
            })
            .collect();

        if let Some(scanned) = self.model.apps.get(&distro) {
            combined.extend(scanned.iter().cloned());
        }

        combined.retain(|app| {
            let pinned = self.prefs.is_pinned(&distro, &app.id);
            let hidden = self.prefs.is_hidden(&distro, &app.id);
            let filter_ok = match self.app_filter {
                AppFilter::All => !hidden,
                AppFilter::Pinned => pinned && !hidden,
                AppFilter::Gui => !app.terminal && !hidden,
                AppFilter::Terminal => app.terminal && !hidden,
                AppFilter::Hidden => hidden,
            };
            if !filter_ok {
                return false;
            }
            if query.is_empty() {
                return true;
            }
            app.name.to_lowercase().contains(&query)
                || app.exec.to_lowercase().contains(&query)
                || app.comment.to_lowercase().contains(&query)
        });

        combined.sort_by(|a, b| {
            let pa = self.prefs.is_pinned(&distro, &a.id);
            let pb = self.prefs.is_pinned(&distro, &b.id);
            pb.cmp(&pa)
                .then_with(|| b.is_custom.cmp(&a.is_custom))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });

        if combined.is_empty() {
            let msg = if scanning {
                format!("Scanning installed applications in {distro}…")
            } else if !query.is_empty() {
                format!(
                    "No applications match \"{query}\". Press Cmd+Shift+R to run any command directly."
                )
            } else if self.app_filter == AppFilter::Pinned {
                "No pinned apps yet. Click the pin icon on any app card to pin it here.".into()
            } else if self.app_filter == AppFilter::Hidden {
                "No hidden apps.".into()
            } else if !is_running {
                "Start the distribution above to load installed apps, or add a custom shortcut."
                    .into()
            } else {
                format!(
                    "No .desktop applications found in {distro}. Install GUI apps or use Shortcut / Run…"
                )
            };
            root = root.child(
                ui.card()
                    .p(px(32.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(13.))
                    .text_color(p.on_surface_variant)
                    .child(msg),
            );
            return root;
        }

        let mut grid = div().flex().flex_wrap().gap(px(12.));
        for app in combined {
            grid = grid.child(self.render_app_card(ui, &distro, &app, cx));
        }

        root = root.child(grid);
        root
    }

    fn render_app_card(
        &self,
        ui: &Ui,
        distro: &str,
        app: &DesktopApp,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let p = ui.p;
        let pinned = self.prefs.is_pinned(distro, &app.id);
        let hidden = self.prefs.is_hidden(distro, &app.id);
        let launch_distro = distro.to_owned();
        let launch_name = app.name.clone();
        let launch_exec = app.exec.clone();
        let launch_term = app.terminal;
        let pin_distro = distro.to_owned();
        let pin_id = app.id.clone();
        let hide_distro = distro.to_owned();
        let hide_id = app.id.clone();
        let del_id = app.id.clone();
        let is_custom = app.is_custom;

        let icon_box = if let Some(path) = &app.icon_path {
            div()
                .size(px(44.))
                .rounded(px(14.))
                .bg(p.surface_high)
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .overflow_hidden()
                .child(img(path.clone()).size(px(32.)))
        } else {
            let initial = app
                .name
                .chars()
                .next()
                .unwrap_or('?')
                .to_uppercase()
                .to_string();
            let badge_bg = avatar_color(&app.name);
            div()
                .size(px(44.))
                .rounded(px(14.))
                .bg(badge_bg)
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(18.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(0xffffff))
                .child(initial)
        };

        let subtitle = if !app.comment.is_empty() {
            app.comment.clone()
        } else {
            app.exec.clone()
        };

        let card_bg = if pinned {
            p.surface_high
        } else {
            p.surface_container
        };
        let card_hover = if pinned {
            p.surface_highest
        } else {
            p.surface_high
        };

        div()
            .id(eid(format!("app-card-{distro}-{}", app.id)))
            .w(px(238.))
            .p(px(14.))
            .rounded(px(22.))
            .bg(card_bg)
            .cursor_pointer()
            .hover(move |s| s.bg(card_hover))
            .flex()
            .flex_col()
            .gap(px(12.))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.backend.send(Command::LaunchApp {
                    distro: launch_distro.clone(),
                    name: launch_name.clone(),
                    exec: launch_exec.clone(),
                    terminal: launch_term,
                });
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(icon_box)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(13.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(p.on_surface)
                                    .truncate()
                                    .child(app.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(p.on_surface_variant)
                                    .truncate()
                                    .child(subtitle),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(5.))
                            .child(ui.chip(
                                if app.terminal { "Terminal" } else { "Wayland" },
                                if app.terminal {
                                    ChipTone::Secondary
                                } else {
                                    ChipTone::Tertiary
                                },
                            ))
                            .when(is_custom, |s| s.child(ui.chip("Custom", ChipTone::Amber))),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .when(is_custom, |this| {
                                let del_id = del_id.clone();
                                this.child(
                                    div()
                                        .id(eid(format!("del-custom-{del_id}")))
                                        .size(px(28.))
                                        .rounded_full()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .hover(move |s| s.bg(p.error_container))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.prefs.custom_apps.retain(|c| c.id != del_id);
                                            this.prefs.save();
                                            cx.notify();
                                        }))
                                        .child(ui.icon("trash", 14., p.error)),
                                )
                            })
                            .when(!is_custom, |this| {
                                let hide_distro = hide_distro.clone();
                                let hide_id = hide_id.clone();
                                this.child(
                                    div()
                                        .id(eid(format!("hide-app-{distro}-{}", app.id)))
                                        .size(px(28.))
                                        .rounded_full()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .hover(move |s| s.bg(p.surface_highest))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.prefs.toggle_hidden(&hide_distro, &hide_id);
                                            cx.notify();
                                        }))
                                        .child(ui.icon(
                                            "eye-off",
                                            14.,
                                            if hidden {
                                                p.amber
                                            } else {
                                                p.on_surface_variant
                                            },
                                        )),
                                )
                            })
                            .child(
                                div()
                                    .id(eid(format!("pin-app-{distro}-{}", app.id)))
                                    .size(px(28.))
                                    .rounded_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .when(pinned, |s| s.bg(p.primary_container))
                                    .hover(move |s| s.bg(p.surface_highest))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.prefs.toggle_pin(&pin_distro, &pin_id);
                                        cx.notify();
                                    }))
                                    .child(ui.icon(
                                        "pin",
                                        14.,
                                        if pinned {
                                            p.on_primary_container
                                        } else {
                                            p.on_surface_variant
                                        },
                                    )),
                            ),
                    ),
            )
    }

    pub fn page_distros(
        &mut self,
        ui: &Ui,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let p = ui.p;
        if !matches!(self.model.status, ConnectionStatus::Ready) {
            return self.connection_empty_state(ui, cx);
        }

        let images = self.model.snapshot.images.clone();
        let running_count = self.model.snapshot.running.len();
        let sub = if running_count == 0 {
            "None running".into()
        } else {
            format!("{running_count} running")
        };

        let mut root = div().flex().flex_col().gap(px(16.)).child(
            div()
                .flex()
                .items_end()
                .justify_between()
                .child(ui.page_header("Distributions", Some(sub)))
                .child(
                    ui.compact_button(
                        "btn-send-file",
                        "Send file to /mnt/android",
                        Some("folder"),
                        ButtonKind::Tonal,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.pick_file_to_push(cx))),
                ),
        );

        if images.is_empty() {
            return root.child(
                ui.card()
                    .p(px(28.))
                    .text_size(px(13.))
                    .text_color(p.on_surface_variant)
                    .child("No distributions found on this device."),
            );
        }

        for img in images {
            let name = img.name.clone();
            let running = self.model.is_running(&name);
            let disp = self.model.display(&name);
            let disp_active = matches!(
                disp.state,
                DisplayState::Connected | DisplayState::Connecting | DisplayState::Reconnecting
            );
            let busy = self.model.busy_distros.get(&name).cloned();

            let start_name = name.clone();
            let start_path = img.path.clone();
            let stop_name = name.clone();
            let disp_name = name.clone();
            let term_name = name.clone();
            let apps_name = name.clone();
            let cfg_img = img.clone();

            let used_gb = img.used_mb as f64 / 1024.0;
            let total_gb = img.size_gb as f64;
            let low_space = img.free_mb < 1024;
            let fg = if running {
                p.on_primary_container
            } else {
                p.on_surface
            };
            let sub_fg = if running {
                p.on_primary_container
            } else {
                p.on_surface_variant
            };
            let track_bg = if running {
                p.secondary_container
            } else {
                p.surface_highest
            };
            let bar_color = if low_space { p.error } else { p.primary };

            let mut meta = vec![
                img.display_os().to_owned(),
                if running { "Running" } else { "Stopped" }.to_owned(),
            ];
            if img.gpu_running {
                meta.push("GPU".into());
            }
            if img.init == "system" {
                meta.push("System init".into());
            }
            if img.isolated_net {
                meta.push("Isolated net".into());
            }
            if img.ssh_port > 0 {
                meta.push(format!("SSH :{}", img.ssh_port));
            }

            let card = ui
                .hero_card(running)
                .p(px(22.))
                .flex()
                .flex_col()
                .gap(px(16.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(16.))
                        .child(ui.shape_icon(
                            running,
                            "terminal",
                            56.,
                            if running {
                                p.primary
                            } else {
                                p.secondary_container
                            },
                            if running {
                                p.on_primary
                            } else {
                                p.on_secondary_container
                            },
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(3.))
                                .child(
                                    div()
                                        .text_size(px(21.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(fg)
                                        .child(img.name.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .text_color(sub_fg)
                                        .opacity(0.85)
                                        .child(meta.join(" · ")),
                                ),
                        )
                        .child(
                            ui.icon_button(
                                eid(format!("cfg-{}", img.name)),
                                "settings",
                                "Configure image",
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_configure_distro(cfg_img.clone(), running, window, cx);
                            })),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(7.))
                        .child(ui.m3_progress(img.used_ratio(), bar_color, track_bg))
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(if low_space { p.error } else { sub_fg })
                                .opacity(if low_space { 1.0 } else { 0.8 })
                                .child(format!("{:.1} GB of {:.1} GB used", used_gb, total_gb)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .when(running, |this| {
                            this.child(
                                ui.button(
                                    eid(format!("term-{term_name}")),
                                    "Open terminal",
                                    Some("terminal"),
                                    ButtonKind::Primary,
                                )
                                .flex_1()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.open_terminal_for(&term_name, cx);
                                })),
                            )
                            .child(
                                ui.button(
                                    eid(format!("disp-{disp_name}")),
                                    if disp_active {
                                        "Disconnect display"
                                    } else {
                                        "Display"
                                    },
                                    Some("monitor"),
                                    ButtonKind::Tonal,
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if disp_active {
                                        this.backend.send(Command::DisconnectDisplay {
                                            distro: disp_name.clone(),
                                        });
                                    } else {
                                        this.backend.send(Command::ConnectDisplay {
                                            distro: disp_name.clone(),
                                        });
                                    }
                                    cx.notify();
                                })),
                            )
                            .child(
                                ui.button(
                                    eid(format!("apps-{apps_name}")),
                                    "Apps",
                                    Some("apps"),
                                    ButtonKind::Tonal,
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.select_distro(apps_name.clone(), cx);
                                    this.page = Page::Apps;
                                    cx.notify();
                                })),
                            )
                            .child(
                                ui.button(
                                    eid(format!("stop-{stop_name}")),
                                    busy.as_deref().unwrap_or("Stop"),
                                    None,
                                    ButtonKind::Tonal,
                                )
                                .disabled(busy.is_some())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.backend.send(Command::StopDistro {
                                        name: stop_name.clone(),
                                    });
                                    cx.notify();
                                })),
                            )
                        })
                        .when(!running, |this| {
                            this.child(
                                ui.button(
                                    eid(format!("start-{start_name}")),
                                    busy.as_deref().unwrap_or("Start"),
                                    Some("play"),
                                    ButtonKind::Primary,
                                )
                                .flex_1()
                                .disabled(busy.is_some())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.backend.send(Command::StartDistro {
                                        name: start_name.clone(),
                                        image_path: start_path.clone(),
                                    });
                                    cx.notify();
                                })),
                            )
                        }),
                );

            root = root.child(card);
        }

        root
    }

    pub fn page_ports(
        &mut self,
        ui: &Ui,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let p = ui.p;
        if !matches!(self.model.status, ConnectionStatus::Ready) {
            return self.connection_empty_state(ui, cx);
        }

        let mut root = div().flex().flex_col().gap(px(14.)).child(ui.page_header(
            "Ports & SSH",
            Some(format!("{} active ADB forwards", self.model.forwards.len())),
        ));

        let ssh_images: Vec<_> = self
            .model
            .snapshot
            .images
            .iter()
            .filter(|i| i.ssh_port > 0)
            .cloned()
            .collect();

        if !ssh_images.is_empty() {
            root = root.child(ui.section_header("SSH over USB"));
            let mut seg = div().flex().flex_col().gap(px(3.));
            let count = ssh_images.len();
            for (idx, img) in ssh_images.into_iter().enumerate() {
                let user = if img.username.is_empty() {
                    img.name.clone()
                } else {
                    img.username.clone()
                };
                let port = img.ssh_port;
                let cmd_str = format!("ssh -p {port} {user}@127.0.0.1");
                let cmd_copy = cmd_str.clone();
                let distro_name = img.name.clone();
                let user_term = user.clone();
                let forwarded = self
                    .model
                    .forwards
                    .iter()
                    .any(|f| f.local == format!("tcp:{port}"));

                seg = seg.child(
                    ui.segmented_row(idx, count)
                        .px(px(18.))
                        .py(px(14.))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(12.))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(3.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(8.))
                                        .child(
                                            div()
                                                .text_size(px(14.))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(p.on_surface)
                                                .child(img.name.clone()),
                                        )
                                        .child(ui.chip(
                                            if forwarded {
                                                "Forwarded"
                                            } else {
                                                "Not forwarded"
                                            },
                                            if forwarded {
                                                ChipTone::Primary
                                            } else {
                                                ChipTone::Surface
                                            },
                                        )),
                                )
                                .child(ui.mono(cmd_str).text_color(p.on_surface_variant)),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .when(!forwarded, |this| {
                                    this.child(
                                        ui.compact_button(
                                            eid(format!("ssh-fwd-{port}")),
                                            "Forward",
                                            Some("network"),
                                            ButtonKind::Tonal,
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.backend.send(Command::AddForward {
                                                local_port: port,
                                                remote_port: port,
                                            });
                                            cx.notify();
                                        })),
                                    )
                                })
                                .child(
                                    ui.compact_button(
                                        eid(format!("ssh-copy-{}", img.name)),
                                        "Copy",
                                        Some("copy"),
                                        ButtonKind::Surface,
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            cmd_copy.clone(),
                                        ));
                                        this.show_toast("Copied SSH command", false, cx);
                                    })),
                                )
                                .child(
                                    ui.compact_button(
                                        eid(format!("ssh-open-{}", img.name)),
                                        "Open SSH",
                                        Some("terminal"),
                                        ButtonKind::Primary,
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.backend.send(Command::OpenTerminal {
                                            distro: distro_name.clone(),
                                            via_ssh: true,
                                            ssh_port: port,
                                            username: user_term.clone(),
                                            guest_cmd: None,
                                        });
                                        cx.notify();
                                    })),
                                ),
                        ),
                );
            }
            root = root.child(seg);
        }

        root = root
            .child(ui.section_header("Add ADB port forward"))
            .child(
                ui.card().p(px(18.)).child(
                    div()
                        .flex()
                        .items_end()
                        .gap(px(10.))
                        .child(div().flex_1().child(self.labeled_input(
                            ui,
                            "Local PC port",
                            &self.fwd_local_input,
                            true,
                        )))
                        .child(div().flex_1().child(self.labeled_input(
                            ui,
                            "Phone / guest port",
                            &self.fwd_remote_input,
                            true,
                        )))
                        .child(
                            ui.button("btn-add-fwd", "Forward", Some("plus"), ButtonKind::Primary)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let l = this
                                        .fwd_local_input
                                        .read(cx)
                                        .value()
                                        .trim()
                                        .parse::<u16>();
                                    let r = this
                                        .fwd_remote_input
                                        .read(cx)
                                        .value()
                                        .trim()
                                        .parse::<u16>();
                                    match (l, r) {
                                        (Ok(lp), Ok(rp)) if lp > 0 && rp > 0 => {
                                            this.backend.send(Command::AddForward {
                                                local_port: lp,
                                                remote_port: rp,
                                            });
                                        }
                                        _ => {
                                            this.show_toast(
                                                "Enter valid port numbers (1..65535)",
                                                true,
                                                cx,
                                            );
                                        }
                                    }
                                })),
                        ),
                ),
            );

        let has_forwards = !self.model.forwards.is_empty();
        root = root.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(ui.section_header("Active ADB tunnels"))
                .when(has_forwards, |this| {
                    this.child(
                        ui.compact_button(
                            "btn-clear-fwds",
                            "Clear all",
                            Some("trash"),
                            ButtonKind::Danger,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.backend.send(Command::ClearForwards);
                            cx.notify();
                        })),
                    )
                }),
        );

        if self.model.forwards.is_empty() {
            root = root.child(
                ui.card()
                    .p(px(20.))
                    .text_size(px(12.5))
                    .text_color(p.on_surface_variant)
                    .child("No active port forwards on this device."),
            );
        } else {
            let mut seg = div().flex().flex_col().gap(px(3.));
            let fwds = self.model.forwards.clone();
            let count = fwds.len();
            for (idx, fwd) in fwds.into_iter().enumerate() {
                let local_port = fwd
                    .local
                    .strip_prefix("tcp:")
                    .unwrap_or(&fwd.local)
                    .to_owned();
                let url = format!("http://127.0.0.1:{local_port}");
                let local_remove = fwd.local.clone();
                seg = seg.child(
                    ui.segmented_row(idx, count)
                        .px(px(18.))
                        .py(px(11.))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(10.))
                        .child(
                            ui.mono(format!("127.0.0.1:{local_port}  ->  {}", fwd.remote))
                                .text_color(p.on_surface),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(
                                    ui.compact_button(
                                        eid(format!("open-fwd-{local_port}")),
                                        "Open",
                                        Some("external"),
                                        ButtonKind::Surface,
                                    )
                                    .on_click(move |_, _, _| open_url(&url)),
                                )
                                .child(
                                    ui.compact_button(
                                        eid(format!("rm-fwd-{local_port}")),
                                        "Remove",
                                        Some("trash"),
                                        ButtonKind::Danger,
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.backend.send(Command::RemoveForward {
                                            local: local_remove.clone(),
                                        });
                                        cx.notify();
                                    })),
                                ),
                        ),
                );
            }
            root = root.child(seg);
        }

        root
    }

    pub fn page_logs(
        &mut self,
        ui: &Ui,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let p = ui.p;
        if !matches!(self.model.status, ConnectionStatus::Ready) {
            return self.connection_empty_state(ui, cx);
        }

        let mut tabs = div().flex().items_center().gap(px(6.));
        for kind in [LogKind::Daemon, LogKind::Autostart, LogKind::Sshd] {
            let active = self.log_kind == kind;
            tabs = tabs.child(
                div()
                    .id(eid(format!("log-tab-{}", kind.label())))
                    .px(px(16.))
                    .py(px(8.))
                    .rounded_full()
                    .cursor_pointer()
                    .bg(if active {
                        p.secondary_container
                    } else {
                        p.surface_container
                    })
                    .text_size(px(12.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(if active {
                        p.on_secondary_container
                    } else {
                        p.on_surface_variant
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.log_kind = kind;
                        this.load_current_log(cx);
                    }))
                    .child(kind.label()),
            );
        }

        let log_copy = self.model.log_text.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(
                div()
                    .flex()
                    .items_end()
                    .justify_between()
                    .child(ui.page_header(
                        "Logs",
                        self.active_distro().map(|d| format!("Active distro: {d}")),
                    ))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                ui.compact_button(
                                    "btn-log-copy",
                                    "Copy",
                                    Some("copy"),
                                    ButtonKind::Surface,
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        log_copy.clone(),
                                    ));
                                    this.show_toast("Copied log to clipboard", false, cx);
                                })),
                            )
                            .child(
                                ui.compact_button(
                                    "btn-log-refresh",
                                    "Refresh",
                                    Some("refresh"),
                                    ButtonKind::Primary,
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.load_current_log(cx))),
                            ),
                    ),
            )
            .child(tabs)
            .child(
                ui.card()
                    .p(px(18.))
                    .min_h(px(380.))
                    .font_family(MONO)
                    .text_size(px(11.5))
                    .text_color(p.on_surface)
                    .child(if self.model.log_loading {
                        "Loading log…".to_owned()
                    } else if self.model.log_text.is_empty() {
                        "Click Refresh to load logs.".to_owned()
                    } else {
                        self.model.log_text.clone()
                    }),
            )
    }

    pub fn page_settings(
        &mut self,
        ui: &Ui,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let p = ui.p;
        let mut root = div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(ui.page_header("Settings", None));

        let mut theme_row = div().flex().gap(px(6.));
        for mode in [ThemeMode::System, ThemeMode::Light, ThemeMode::Dark] {
            let active = self.prefs.theme == mode;
            theme_row = theme_row.child(
                div()
                    .id(eid(format!("theme-{}", mode.label())))
                    .px(px(14.))
                    .py(px(7.))
                    .rounded_full()
                    .cursor_pointer()
                    .bg(if active {
                        p.primary
                    } else {
                        p.surface_high
                    })
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(if active {
                        p.on_primary
                    } else {
                        p.on_surface
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.prefs.theme = mode;
                        this.prefs.save();
                        cx.notify();
                    }))
                    .child(mode.label()),
            );
        }

        root = root
            .child(ui.section_header("Appearance"))
            .child(
                ui.segmented_row(0, 1)
                    .px(px(18.))
                    .py(px(14.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(13.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(p.on_surface)
                                    .child("Theme"),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(p.on_surface_variant)
                                    .child("Follow system appearance or pick Light / Dark"),
                            ),
                    )
                    .child(theme_row),
            );

        let mut term_pills = div().flex().flex_wrap().gap(px(6.));
        for choice in TerminalApp::platform_choices() {
            let active = self.prefs.terminal == choice;
            let label = choice.label();
            term_pills = term_pills.child(
                div()
                    .id(eid(format!("term-choice-{label}")))
                    .px(px(13.))
                    .py(px(6.))
                    .rounded_full()
                    .cursor_pointer()
                    .bg(if active {
                        p.primary
                    } else {
                        p.surface_high
                    })
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(if active {
                        p.on_primary
                    } else {
                        p.on_surface
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.prefs.terminal = choice.clone();
                        this.prefs.save();
                        this.sync_backend_settings();
                        cx.notify();
                    }))
                    .child(label),
            );
        }

        root = root.child(ui.section_header("External terminal")).child(
            div()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    ui.segmented_row(0, 2)
                        .px(px(18.))
                        .py(px(14.))
                        .flex()
                        .flex_col()
                        .gap(px(10.))
                        .child(
                            div()
                                .text_size(px(13.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(p.on_surface)
                                .child("Host terminal app"),
                        )
                        .child(term_pills),
                )
                .child(ui.segmented_row(1, 2).px(px(18.)).py(px(14.)).child(
                    self.toggle_row(
                        ui,
                        "set-term-ssh",
                        "Connect via SSH",
                        "Off = direct ADB shell (works immediately without SSH keys); On = adb forward + ssh",
                        self.prefs.terminal_via_ssh,
                        cx,
                        |this, checked, cx| {
                            this.prefs.terminal_via_ssh = checked;
                            this.prefs.save();
                            cx.notify();
                        },
                    ),
                )),
        );

        root = root.child(ui.section_header("Display & tunnels")).child(
            div()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(ui.segmented_row(0, 3).px(px(18.)).py(px(14.)).child(
                    self.toggle_row(
                        ui,
                        "set-cmd-ctrl",
                        "Map Command / Super to Ctrl in Linux windows",
                        "Lets Cmd+C/V/T/W act as Ctrl shortcuts in Linux GUI apps (next display connect)",
                        self.prefs.cmd_to_ctrl,
                        cx,
                        |this, checked, cx| {
                            this.prefs.cmd_to_ctrl = checked;
                            this.prefs.save();
                            this.sync_backend_settings();
                            cx.notify();
                        },
                    ),
                ))
                .child(ui.segmented_row(1, 3).px(px(18.)).py(px(14.)).child(
                    self.toggle_row(
                        ui,
                        "set-auto-reconnect",
                        "Auto-reconnect Wayland display",
                        "Automatically reconnects the display host if USB hiccups while running",
                        self.prefs.auto_reconnect_display,
                        cx,
                        |this, checked, cx| {
                            this.prefs.auto_reconnect_display = checked;
                            this.prefs.save();
                            this.sync_backend_settings();
                            cx.notify();
                        },
                    ),
                ))
                .child(ui.segmented_row(2, 3).px(px(18.)).py(px(14.)).child(
                    self.toggle_row(
                        ui,
                        "set-auto-fwd",
                        "Auto-forward SSH & image ports",
                        "Keeps adb forward tunnels active for running distributions",
                        self.prefs.auto_forward_ports,
                        cx,
                        |this, checked, cx| {
                            this.prefs.auto_forward_ports = checked;
                            this.prefs.save();
                            this.sync_backend_settings();
                            cx.notify();
                        },
                    ),
                )),
        );

        let adb_text = self
            .model
            .adb_path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Not found (install Android platform-tools or pick adb)".into());
        let has_override = self.prefs.adb_path.is_some();
        let has_kernel = matches!(self.model.status, ConnectionStatus::Ready);
        let sys_count = if has_kernel { 2 } else { 1 };

        let mut sys_seg = div().flex().flex_col().gap(px(3.)).child(
            ui.segmented_row(0, sys_count)
                .px(px(18.))
                .py(px(14.))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .text_size(px(13.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(p.on_surface)
                                .child("ADB binary"),
                        )
                        .child(ui.mono(adb_text).text_color(p.on_surface_variant).truncate()),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .when(has_override, |this| {
                            this.child(
                                ui.compact_button("btn-adb-reset", "Auto", None, ButtonKind::Surface)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.prefs.adb_path = None;
                                        this.prefs.save();
                                        this.sync_backend_settings();
                                        cx.notify();
                                    })),
                            )
                        })
                        .child(
                            ui.compact_button(
                                "btn-adb-browse",
                                "Browse…",
                                Some("folder"),
                                ButtonKind::Tonal,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.pick_adb_binary(cx))),
                        ),
                ),
        );

        if has_kernel {
            let k = &self.model.snapshot.kernel;
            let ns = [
                ("mnt", k.mnt),
                ("uts", k.uts),
                ("pid", k.pid),
                ("ipc", k.ipc),
                ("net", k.net),
                ("cgroup", k.cgroup),
                ("user", k.user),
            ];
            let mut pills = div().flex().flex_wrap().gap(px(6.));
            pills = pills.child(ui.chip(&format!("page {}B", k.page_size), ChipTone::Surface));
            pills = pills.child(ui.chip(
                if k.selinux == 1 {
                    "SELinux Enforcing"
                } else {
                    "SELinux Permissive"
                },
                if k.selinux == 1 {
                    ChipTone::Primary
                } else {
                    ChipTone::Danger
                },
            ));
            for (name, present) in ns {
                pills = pills.child(ui.chip(
                    name,
                    if present {
                        ChipTone::Secondary
                    } else {
                        ChipTone::Surface
                    },
                ));
            }
            sys_seg = sys_seg.child(
                ui.segmented_row(1, sys_count)
                    .px(px(18.))
                    .py(px(14.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        div()
                            .text_size(px(13.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(p.on_surface)
                            .child("Connected phone kernel"),
                    )
                    .child(pills),
            );
        }
        root = root.child(ui.section_header("System")).child(sys_seg);

        const GITHUB_URL: &str = "https://github.com/rfc9849/Bareboot";
        root = root.child(ui.section_header("About")).child(
            div()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    ui.segmented_row(0, 2)
                        .px(px(18.))
                        .py(px(14.))
                        .flex()
                        .items_center()
                        .gap(px(14.))
                        .child(ui.shape_icon(true, "terminal", 42., p.primary, p.on_primary))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .text_size(px(14.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(p.on_surface)
                                        .child(format!(
                                            "Bareboot Desktop {}",
                                            env!("CARGO_PKG_VERSION")
                                        )),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(p.on_surface_variant)
                                        .child("Native ARM64 Linux companion · Apache-2.0"),
                                ),
                        ),
                )
                .child(
                    ui.segmented_row(1, 2)
                        .px(px(18.))
                        .py(px(14.))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(12.))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .text_size(px(13.5))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(p.on_surface)
                                        .child("GitHub"),
                                )
                                .child(ui.mono(GITHUB_URL).text_color(p.on_surface_variant)),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(
                                    ui.compact_button(
                                        "about-copy-gh",
                                        "Copy",
                                        Some("copy"),
                                        ButtonKind::Surface,
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            GITHUB_URL.to_owned(),
                                        ));
                                        this.show_toast("Copied GitHub URL", false, cx);
                                    })),
                                )
                                .child(
                                    ui.compact_button(
                                        "about-open-gh",
                                        "Open",
                                        Some("external"),
                                        ButtonKind::Primary,
                                    )
                                    .on_click(|_, _, _| open_url(GITHUB_URL)),
                                ),
                        ),
                ),
        );

        let _ = Prefs::pin_key;
        root
    }

    fn connection_empty_state(&self, ui: &Ui, cx: &mut Context<Self>) -> gpui_kit::Div {
        let p = ui.p;
        let (title, body, icon_name) = match &self.model.status {
            ConnectionStatus::AdbMissing => (
                "ADB was not found",
                "Install Android platform-tools or select your adb executable in Settings."
                    .to_owned(),
                "settings",
            ),
            ConnectionStatus::NoDevice => (
                "Connect your Android device",
                "Plug in your phone over USB (or connect via Wireless Debugging). Bareboot Companion detects devices automatically."
                    .to_owned(),
                "phone",
            ),
            ConnectionStatus::Unauthorized(st) => (
                "Authorize USB debugging on your phone",
                format!(
                    "ADB device state is \"{st}\". Unlock your phone and accept the debugging prompt."
                ),
                "phone",
            ),
            ConnectionStatus::DaemonUnavailable(msg) => (
                "Cannot reach the Bareboot daemon",
                format!(
                    "{msg}\n\nMake sure the Bareboot root module is installed and ADB Shell has root access in KernelSU / Magisk."
                ),
                "layers",
            ),
            ConnectionStatus::Ready => ("Connected", String::new(), "check"),
        };

        ui.card()
            .p(px(36.))
            .mt(px(16.))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(14.))
            .child(ui.shape_icon(
                true,
                icon_name,
                64.,
                p.primary_container,
                p.on_primary_container,
            ))
            .child(
                div()
                    .text_size(px(18.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(p.on_surface)
                    .child(title),
            )
            .child(
                div()
                    .max_w(px(460.))
                    .text_center()
                    .text_size(px(13.))
                    .text_color(p.on_surface_variant)
                    .child(body),
            )
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .pt(px(4.))
                    .child(
                        ui.button("empty-retry", "Retry", Some("refresh"), ButtonKind::Primary)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.backend.send(Command::RefreshNow);
                                cx.notify();
                            })),
                    )
                    .when(
                        matches!(self.model.status, ConnectionStatus::AdbMissing),
                        |this| {
                            this.child(
                                ui.button(
                                    "empty-settings",
                                    "Open Settings",
                                    Some("settings"),
                                    ButtonKind::Tonal,
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.page = Page::Settings;
                                    cx.notify();
                                })),
                            )
                        },
                    ),
            )
    }
}

fn avatar_color(name: &str) -> gpui_kit::Rgba {
    const COLORS: [u32; 8] = [
        0x5e5847, 0x525a4e, 0x63554b, 0x4f565b, 0x615950, 0x575248, 0x555c54, 0x5f544a,
    ];
    let hash = name
        .bytes()
        .fold(5381u32, |acc, b| acc.wrapping_mul(33).wrapping_add(u32::from(b)));
    rgb(COLORS[(hash as usize) % COLORS.len()])
}
