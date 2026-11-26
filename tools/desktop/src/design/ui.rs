use gpui_kit::base::{Button, Switch};
use gpui_kit::prelude::*;
use gpui_kit::{Div, ElementId, FontWeight, Rgba, SharedString, Svg, div, px, relative, svg};

use super::theme::{MONO, Palette};

pub fn eid(name: impl Into<String>) -> ElementId {
    ElementId::Name(SharedString::from(name.into()))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    Primary,
    Tonal,
    Surface,
    Ghost,
    Danger,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ChipTone {
    Surface,
    Primary,
    Secondary,
    Tertiary,
    Amber,
    Danger,
}

#[derive(Clone, Copy)]
pub struct Ui {
    pub p: Palette,
}

impl Ui {
    pub fn new(p: Palette) -> Self {
        Self { p }
    }

    pub fn icon(&self, name: &str, size: f32, color: Rgba) -> Svg {
        svg()
            .path(SharedString::from(format!("icons/{name}.svg")))
            .size(px(size))
            .flex_none()
            .text_color(color)
    }

    pub fn shape_icon(
        &self,
        cookie: bool,
        icon: &str,
        size: f32,
        container: Rgba,
        content: Rgba,
    ) -> Div {
        let icon_px = size * 0.46;
        if cookie {
            div()
                .relative()
                .size(px(size))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    svg()
                        .path("shapes/cookie9.svg")
                        .absolute()
                        .inset_0()
                        .size(px(size))
                        .text_color(container),
                )
                .child(div().relative().child(self.icon(icon, icon_px, content)))
        } else {
            div()
                .size(px(size))
                .flex_none()
                .rounded(px(size * 0.32))
                .bg(container)
                .flex()
                .items_center()
                .justify_center()
                .child(self.icon(icon, icon_px, content))
        }
    }

    pub fn mono(&self, text: impl Into<SharedString>) -> Div {
        div()
            .font_family(MONO)
            .text_size(px(11.5))
            .child(text.into())
    }

    pub fn page_header(&self, title: &str, subtitle: Option<String>) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(
                div()
                    .text_size(px(26.))
                    .line_height(px(32.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(self.p.on_surface)
                    .child(title.to_owned()),
            )
            .when_some(subtitle, |this, sub| {
                this.child(
                    div()
                        .text_size(px(13.))
                        .text_color(self.p.on_surface_variant)
                        .child(sub),
                )
            })
    }

    pub fn section_header(&self, title: &str) -> Div {
        div()
            .px(px(6.))
            .pt(px(6.))
            .text_size(px(12.5))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(self.p.primary)
            .child(title.to_owned())
    }

    pub fn button(
        &self,
        id: impl Into<ElementId>,
        label: &str,
        icon: Option<&str>,
        kind: ButtonKind,
    ) -> Button {
        let p = self.p;
        let (bg, fg, hover) = match kind {
            ButtonKind::Primary => (p.primary, p.on_primary, p.primary_hover),
            ButtonKind::Tonal => (
                p.secondary_container,
                p.on_secondary_container,
                p.secondary_container_hover,
            ),
            ButtonKind::Surface => (p.surface_high, p.on_surface, p.surface_highest),
            ButtonKind::Ghost => (p.surface, p.on_surface_variant, p.surface_high),
            ButtonKind::Danger => (p.error_container, p.on_error_container, p.error_container),
        };
        let bg = if kind == ButtonKind::Ghost {
            gpui_kit::transparent_black()
        } else {
            bg.into()
        };
        Button::new(id)
            .accessibility_label(label.to_owned())
            .h(px(40.))
            .px(px(18.))
            .gap(px(8.))
            .rounded_full()
            .bg(bg)
            .text_color(fg)
            .text_size(px(13.))
            .font_weight(FontWeight::MEDIUM)
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .focus_visible(move |style| style.border_1().border_color(p.primary))
            .when_some(icon, |this, name| this.child(self.icon(name, 16., fg)))
            .child(label.to_owned())
    }

    pub fn compact_button(
        &self,
        id: impl Into<ElementId>,
        label: &str,
        icon: Option<&str>,
        kind: ButtonKind,
    ) -> Button {
        self.button(id, label, icon, kind)
            .h(px(34.))
            .px(px(14.))
            .gap(px(6.))
            .text_size(px(12.))
    }

    pub fn icon_button(&self, id: impl Into<ElementId>, icon: &str, label: &str) -> Button {
        let p = self.p;
        Button::new(id)
            .accessibility_label(label.to_owned())
            .size(px(36.))
            .flex_none()
            .rounded_full()
            .text_color(p.on_surface_variant)
            .cursor_pointer()
            .hover(move |style| style.bg(p.surface_high).text_color(p.on_surface))
            .focus_visible(move |style| style.border_1().border_color(p.primary))
            .child(self.icon(icon, 18., p.on_surface_variant))
    }

    pub fn switch(&self, id: impl Into<ElementId>, checked: bool, label: &str) -> Switch {
        let p = self.p;
        let track = if checked { p.primary } else { p.surface_highest };
        let thumb = if checked {
            p.on_primary
        } else {
            p.on_surface_variant
        };
        let thumb_size = if checked { px(20.) } else { px(16.) };
        let mut switch = Switch::new(id)
            .checked(checked)
            .accessibility_label(label.to_owned())
            .w(px(46.))
            .h(px(26.))
            .px(px(3.))
            .flex_none()
            .rounded_full()
            .bg(track)
            .when(!checked, |s| s.border_1().border_color(p.outline_variant))
            .cursor_pointer()
            .flex()
            .items_center();
        if checked {
            switch = switch.justify_end();
        }
        switch.child(div().size(thumb_size).rounded_full().bg(thumb))
    }

    pub fn chip(&self, text: &str, tone: ChipTone) -> Div {
        let p = self.p;
        let (bg, fg) = match tone {
            ChipTone::Surface => (p.surface_highest, p.on_surface_variant),
            ChipTone::Primary => (p.primary_container, p.on_primary_container),
            ChipTone::Secondary => (p.secondary_container, p.on_secondary_container),
            ChipTone::Tertiary => (p.tertiary_container, p.on_tertiary_container),
            ChipTone::Amber => (p.amber_container, p.amber),
            ChipTone::Danger => (p.error_container, p.on_error_container),
        };
        div()
            .flex()
            .items_center()
            .gap(px(5.))
            .px(px(10.))
            .py(px(4.))
            .rounded_full()
            .bg(bg)
            .text_color(fg)
            .text_size(px(11.))
            .font_weight(FontWeight::MEDIUM)
            .whitespace_nowrap()
            .child(text.to_owned())
    }

    pub fn hero_card(&self, active: bool) -> Div {
        div()
            .rounded(px(28.))
            .bg(if active {
                self.p.primary_container
            } else {
                self.p.surface_high
            })
            .text_color(if active {
                self.p.on_primary_container
            } else {
                self.p.on_surface
            })
    }

    pub fn card(&self) -> Div {
        div().rounded(px(24.)).bg(self.p.surface_container)
    }

    pub fn segmented_row(&self, index: usize, count: usize) -> Div {
        let mut d = div().bg(self.p.surface_container);
        if count <= 1 {
            d = d.rounded(px(20.));
        } else if index == 0 {
            d = d.rounded_t(px(20.)).rounded_b(px(6.));
        } else if index + 1 == count {
            d = d.rounded_t(px(6.)).rounded_b(px(20.));
        } else {
            d = d.rounded(px(6.));
        }
        d
    }

    pub fn m3_progress(&self, ratio: f32, active_color: Rgba, track_color: Rgba) -> Div {
        let ratio = ratio.clamp(0.02, 1.0);
        let rest = (1.0 - ratio).max(0.0);
        div()
            .w_full()
            .h(px(6.))
            .flex()
            .items_center()
            .gap(px(4.))
            .child(
                div()
                    .h_full()
                    .w(relative(ratio))
                    .rounded_full()
                    .bg(active_color),
            )
            .when(rest > 0.02, |this| {
                this.child(
                    div()
                        .flex_1()
                        .h_full()
                        .rounded_full()
                        .bg(track_color)
                        .flex()
                        .items_center()
                        .justify_end()
                        .pr(px(1.))
                        .child(div().size(px(4.)).rounded_full().bg(active_color)),
                )
            })
    }

    pub fn dot(&self, color: Rgba) -> Div {
        div().size(px(8.)).rounded_full().bg(color).flex_none()
    }
}
