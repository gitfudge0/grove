//! Theme selection and appearance settings.

use super::{
    app_theme, c, rpx, zoom, Input, SettingsPanel, ThemeKind, ThemeState, ZoomState, THEME_KIND_W,
    THEME_ROW_H, THEME_SWATCH_RADIUS, ZOOM_DEFAULT,
};
use crate::views::tokens::{
    FORM_VALUE_SIZE, ICON_MD, ICON_SM, ICON_XS, RADIUS_PANEL, SPACE_20, SPACE_LG, SPACE_XS,
};
use gpui::prelude::{
    BorrowAppContext, FluentBuilder, InteractiveElement, ParentElement, StatefulInteractiveElement,
    Styled,
};
use gpui::{div, Context, Window};

impl SettingsPanel {
    pub(super) fn select_theme(
        &mut self,
        id: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.save(cx, |store| {
            store.selected_theme = Some(id.to_string());
            if let Some(item) = app_theme::by_id(id) {
                store.appearance = Some(if item.kind == ThemeKind::Light {
                    grove_core::storage::AppearancePreference::Light
                } else {
                    grove_core::storage::AppearancePreference::Dark
                });
            }
        }) {
            let requested = app_theme::by_id(id).map_or("the theme", |item| item.label);
            let active = grove_core::theme::current();
            let current =
                app_theme::by_id(&active.name).map_or("The previous theme", |item| item.label);
            self.error = Some(format!(
                "Couldn’t save {requested}. {current} is still active. Try again."
            ));
            self.theme_retry = Some(id);
            return;
        }
        self.theme_retry = None;
        if let Some(item) = app_theme::by_id(id) {
            let query = self
                .theme_search
                .as_ref()
                .map(|input| input.read(cx).value().to_string())
                .unwrap_or_default();
            if !item.matches(&query, self.theme_filter) {
                self.theme_filter = None;
                if let Some(input) = &self.theme_search {
                    input.update(cx, |input, cx| input.set_value("", window, cx));
                }
            }
        }
        cx.update_global::<ThemeState, _>(|state, _| state.follow_system = false);
        ThemeState::set_by_name(cx, id);
        if let Some(index) = app_theme::CATALOG.iter().position(|item| item.id == id) {
            self.theme_focus[index].focus(window, cx);
        }
    }

    pub(super) fn themes_body(&self, cx: &mut Context<Self>) -> gpui::Div {
        let query = self
            .theme_search
            .as_ref()
            .map(|input| input.read(cx).value().to_string())
            .unwrap_or_default();
        let active = grove_core::theme::current();
        let mut body = div().flex().flex_col().gap(rpx(SPACE_LG));
        if let Some(input) = &self.theme_search {
            body = body.child(
                super::super::components::project_field_well(false, false).child(
                    Input::new(input)
                        .file_drop_overlay(crate::views::components::file_drop_overlay())
                        .aria_label("Search themes")
                        .appearance(false)
                        .bordered(false)
                        .focus_bordered(false)
                        .p_0()
                        .text_size(rpx(FORM_VALUE_SIZE))
                        .text_color(c::FG()),
                ),
            );
        }
        let mut filters = Self::choices();
        for (id, label, kind) in [
            ("all", "All", None),
            ("light", "Light", Some(ThemeKind::Light)),
            ("dark", "Dark", Some(ThemeKind::Dark)),
        ] {
            filters = filters.child(
                self.choice(
                    format!("theme-filter-{id}"),
                    label,
                    self.theme_filter == kind,
                    true,
                )
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.theme_filter = kind;
                    this.scroll.set_offset(gpui::Point::default());
                    cx.notify();
                })),
            );
        }
        body = body.child(filters);
        if let Some(id) = self.theme_retry {
            body = body.child(
                div().flex().child(
                    self.button("theme-save-retry", "Retry saving theme", false, true)
                        .child(crate::icons::icon("refresh", ICON_MD, c::FG()))
                        .child("Retry")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.select_theme(id, window, cx);
                        })),
                ),
            );
        }
        let mut count = 0;
        let mut list = div()
            .id("theme-list")
            .role(gpui::Role::RadioGroup)
            .aria_label("Application theme")
            .flex()
            .flex_col()
            .w_full()
            .rounded(rpx(RADIUS_PANEL))
            .border_1()
            .border_color(c::BORDER())
            .overflow_hidden();
        for (index, item) in app_theme::CATALOG
            .iter()
            .enumerate()
            .filter(|(_, item)| item.matches(&query, self.theme_filter))
        {
            count += 1;
            let palette = item.terminal();
            let selected = active.name == item.id;
            let mut swatches = div().flex().flex_shrink_0().gap(rpx(SPACE_XS));
            for color in c::swatches(&palette) {
                swatches = swatches.child(
                    div()
                        .size(rpx(ICON_XS))
                        .rounded(rpx(THEME_SWATCH_RADIUS))
                        .border_1()
                        .border_color(c::BORDER_SOFT())
                        .bg(color),
                );
            }
            let kind = if item.kind == ThemeKind::Light {
                "Light"
            } else {
                "Dark"
            };
            list = list.child(
                self.choice(format!("theme-{}", item.id), item.label, selected, true)
                    .role(gpui::Role::RadioButton)
                    .aria_label(format!("{}, {} theme", item.label, kind))
                    .aria_toggled(if selected {
                        gpui::Toggled::True
                    } else {
                        gpui::Toggled::False
                    })
                    .track_focus(&self.theme_focus[index])
                    .anchor_scroll(Some(self.theme_anchors[index].clone()))
                    .w_full()
                    .h(rpx(THEME_ROW_H))
                    .rounded_none()
                    .when(count > 1, |row| {
                        row.border_t_1().border_color(c::BORDER_SOFT())
                    })
                    .gap(rpx(SPACE_LG))
                    .child(swatches)
                    .child(
                        div()
                            .id(format!("theme-name-{}", item.id))
                            .debug_selector(move || format!("theme-name-{}", item.id))
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(item.label),
                    )
                    .child(
                        div()
                            .id(format!("theme-kind-{}", item.id))
                            .debug_selector(move || format!("theme-kind-{}", item.id))
                            .w(rpx(THEME_KIND_W))
                            .flex_shrink_0()
                            .flex()
                            .justify_end()
                            .items_center()
                            .whitespace_nowrap()
                            .text_color(c::FG_DIM())
                            .child(
                                div()
                                    .id(format!("theme-kind-text-{}", item.id))
                                    .debug_selector(move || format!("theme-kind-text-{}", item.id))
                                    .flex_shrink_0()
                                    .whitespace_nowrap()
                                    .child(kind),
                            ),
                    )
                    .child(div().w(rpx(ICON_MD)).flex_shrink_0().when(selected, |el| {
                        el.child(crate::icons::icon("check", ICON_MD, c::FG()))
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_theme(item.id, window, cx);
                    })),
            );
        }
        if count > 0 {
            body = body.child(list);
        }
        if count == 0 {
            body = body.child(
                div()
                    .py(rpx(SPACE_20))
                    .text_color(c::FG_DIM())
                    .child("No themes found. Try another search or choose All."),
            );
        }
        body = body.child(
            Self::disclosure(
                "theme-notices",
                "Theme palette acknowledgements",
                self.show_theme_notices,
            )
            .child(crate::icons::icon(
                if self.show_theme_notices {
                    "chev-down"
                } else {
                    "chev-right"
                },
                ICON_SM,
                c::FG_DIM(),
            ))
            .child("Theme palette acknowledgements")
            .on_click(cx.listener(|this, _, _, cx| {
                this.show_theme_notices = !this.show_theme_notices;
                cx.notify();
            })),
        );
        if self.show_theme_notices {
            body = body.child(super::super::components::readonly_value(
                "theme-notices-text",
                include_str!("../../../assets/themes/NOTICES.txt").to_string(),
            ));
        }
        body
    }

    pub(super) fn set_zoom(&mut self, delta: f32, cx: &mut Context<Self>) {
        let current = cx.global::<ZoomState>().zoom;
        let next = if delta == 0.0 {
            ZOOM_DEFAULT
        } else {
            zoom::snap(current + delta)
        };
        if next != current && self.save(cx, |store| store.ui_zoom = Some(next)) {
            cx.update_global::<ZoomState, _>(|state, _| state.zoom = next);
            crate::platform::text_preferences::sync_component_size(cx);
            cx.refresh_windows();
        }
    }
}
