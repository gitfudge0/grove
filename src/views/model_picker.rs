//! Provider browsing is independent of the conversation's selected provider.
use super::*;
use crate::{settings::SettingsState, views::components::disabled_header_control};
use serde_json::Value;

const PICKER_W: f32 = 360.0;
const RAIL_W: f32 = 44.0;
const RAIL_ACTIVE_W: f32 = 2.0;
const RAIL_TARGET_H: f32 = 32.0;
const SEARCH_H: f32 = 36.0;
const MODEL_ROW_H: f32 = 48.0;
const MODEL_NAME_LINE_H: f32 = 17.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Filter {
    Favorites,
    Provider(codex::Provider),
}
#[derive(Clone)]
struct Model {
    provider: codex::Provider,
    id: String,
    name: String,
    legacy: bool,
}
#[derive(Clone)]
enum Catalog {
    Unloaded,
    Loading,
    Ready(Vec<Model>),
    Error(String),
}
#[derive(Clone, Debug)]
pub(super) enum PickerEvent {
    CatalogUpdated(codex::Provider, Vec<Value>),
    Select(codex::Provider, String),
    Dismiss,
}
pub(super) struct ModelPicker {
    pub(super) passive: bool,
    saved: codex::SavedSession,
    allow_switch: bool,
    selected: Option<String>,
    filter: Filter,
    catalogs: [Catalog; 2],
    generations: [u64; 2],
    refreshing: [bool; 2],
    refresh_errors: [Option<String>; 2],
    refresh_focus: FocusHandle,
    search: Entity<InputState>,
    cursor: usize,
    keyboard_cursor: bool,
    legacy_open: bool,
    focus: FocusHandle,
    rail_focus: [FocusHandle; 3],
    row_focus: HashMap<String, (FocusHandle, FocusHandle)>,
    retry_focus: [FocusHandle; 2],
    legacy_focus: FocusHandle,
    _search_events: Subscription,
}
impl gpui::EventEmitter<PickerEvent> for ModelPicker {}
fn index(provider: codex::Provider) -> usize {
    usize::from(provider == codex::Provider::Claude)
}
fn models(provider: codex::Provider, values: Vec<Value>) -> Vec<Model> {
    let mut seen = HashSet::new();
    values
        .into_iter()
        .filter_map(|value| {
            let id = value["model"].as_str()?.to_owned();
            if id.is_empty() || !seen.insert(id.clone()) {
                return None;
            }
            Some(Model {
                provider,
                name: value["displayName"]
                    .as_str()
                    .filter(|v| !v.is_empty())
                    .unwrap_or(&id)
                    .to_owned(),
                id,
                // Only an explicit provider catalog flag creates a legacy group.
                legacy: value["isLegacy"].as_bool() == Some(true),
            })
        })
        .collect()
}
impl ModelPicker {
    pub(super) fn new(
        state: &codex::Snapshot,
        allow_switch: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search models…"));
        let events = cx.subscribe(&search, |this, _, _: &InputEvent, cx| {
            this.cursor = 0;
            this.keyboard_cursor = false;
            cx.notify();
        });
        let mut catalogs = [Catalog::Unloaded, Catalog::Unloaded];
        if !state.models.is_empty() {
            catalogs[index(state.saved.provider)] =
                Catalog::Ready(models(state.saved.provider, state.models.clone()));
        }
        let has_scoped_favorites = cx
            .global::<SettingsState>()
            .store
            .model_favorites
            .iter()
            .any(|favorite| allow_switch || favorite.provider == state.saved.provider);
        let picker = Self {
            passive: false,
            saved: state.saved.clone(),
            allow_switch,
            selected: state.model.clone().or_else(|| state.resolved_model.clone()),
            filter: if has_scoped_favorites {
                Filter::Favorites
            } else {
                Filter::Provider(state.saved.provider)
            },
            catalogs,
            generations: [0; 2],
            refreshing: [false; 2],
            refresh_errors: [None, None],
            refresh_focus: cx.focus_handle(),
            search,
            cursor: 0,
            keyboard_cursor: false,
            legacy_open: false,
            focus: cx.focus_handle(),
            rail_focus: std::array::from_fn(|_| cx.focus_handle()),
            row_focus: HashMap::new(),
            retry_focus: std::array::from_fn(|_| cx.focus_handle()),
            legacy_focus: cx.focus_handle(),
            _search_events: events,
        };
        picker.search.focus_handle(cx).focus(window, cx);
        picker
    }
    #[cfg(test)]
    pub(super) fn seed_test_catalogs(&mut self) {
        tests::seed(self);
    }
    fn needs_provider(&self, provider: codex::Provider, cx: &App) -> bool {
        (self.allow_switch || provider == self.saved.provider)
            && match self.filter {
                Filter::Provider(selected) => provider == selected,
                Filter::Favorites => cx
                    .global::<SettingsState>()
                    .store
                    .model_favorites
                    .iter()
                    .any(|favorite| favorite.provider == provider),
            }
    }
    pub(super) fn discover(&mut self, cx: &mut Context<Self>) {
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            if self.needs_provider(provider, cx)
                && matches!(self.catalogs[index(provider)], Catalog::Unloaded)
            {
                self.load(provider, false, cx);
            }
        }
    }
    fn load(&mut self, provider: codex::Provider, refresh: bool, cx: &mut Context<Self>) {
        let i = index(provider);
        if self.refreshing[i] {
            return;
        }
        let retry = refresh
            || matches!(self.catalogs[i], Catalog::Error(_))
            || self.refresh_errors[i].is_some();
        self.generations[i] = self.generations[i].wrapping_add(1);
        let generation = self.generations[i];
        self.refresh_errors[i] = None;
        let retained = matches!(self.catalogs[i], Catalog::Ready(_));
        if !retained {
            self.catalogs[i] = Catalog::Loading;
        }
        if !std::path::Path::new(&self.saved.cwd).is_dir() {
            self.catalogs[i] =
                Catalog::Error("The selected checkout is no longer available.".into());
            cx.notify();
            return;
        }
        let mut saved = self.saved.clone();
        saved.provider = provider;
        saved.label = provider.label().into();
        saved.thread_id = None;
        saved.claude_resume = None;
        saved.items.clear();
        saved.draft.clear();
        saved.draft_images.clear();
        saved.draft_reference = None;
        saved.composer = codex::ComposerSettings::default();
        let settings = &cx.global::<SettingsState>().store;
        let args = provider.agent().launch_args_for_backend(
            true,
            settings.yolo_enabled,
            settings.chrome_enabled.unwrap_or(false),
        );
        if !retry {
            if let Some(state) = grove_core::composer_catalog::cached_models(&saved, &args) {
                self.catalogs[i] = Catalog::Ready(models(provider, state.clone()));
                cx.emit(PickerEvent::CatalogUpdated(provider, state));
                cx.notify();
                return;
            }
        }
        self.refreshing[i] = true;
        let task = cx.background_spawn(async move {
            if retry {
                grove_core::composer_catalog::invalidate(&saved, &args);
            }
            grove_core::composer_catalog::discover_models(saved, args)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.complete_models(provider, generation, result, cx);
            });
        })
        .detach();
        cx.notify();
    }
    fn complete_models(
        &mut self,
        provider: codex::Provider,
        generation: u64,
        result: Result<Vec<Value>, String>,
        cx: &mut Context<Self>,
    ) {
        let i = index(provider);
        if self.generations[i] != generation {
            return;
        }
        self.refreshing[i] = false;
        match result {
            Ok(values) => {
                self.catalogs[i] = Catalog::Ready(models(provider, values.clone()));
                self.refresh_errors[i] = None;
                cx.emit(PickerEvent::CatalogUpdated(provider, values));
            }
            Err(error) if matches!(self.catalogs[i], Catalog::Ready(_)) => {
                self.refresh_errors[i] = Some(error);
            }
            Err(error) => self.catalogs[i] = Catalog::Error(error),
        }
        cx.notify();
    }
    fn refresh_visible(&mut self, cx: &mut Context<Self>) {
        if self.passive || self.refreshing.iter().any(|pending| *pending) {
            return;
        }
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            if !self.needs_provider(provider, cx) || self.refreshing[index(provider)] {
                continue;
            }
            self.load(provider, true, cx);
        }
    }
    fn visible(&self, cx: &App) -> Vec<Model> {
        let query = self.search.read(cx).value().trim().to_lowercase();
        let favorites = &cx.global::<SettingsState>().store;
        self.catalogs
            .iter()
            .filter_map(|c| {
                if let Catalog::Ready(m) = c {
                    Some(m)
                } else {
                    None
                }
            })
            .flatten()
            .filter(|m| {
                (self.allow_switch || m.provider == self.saved.provider)
                    && match self.filter {
                        Filter::Favorites => favorites.is_model_favorite(m.provider, &m.id),
                        Filter::Provider(p) => p == m.provider,
                    }
                    && (self.filter == Filter::Favorites
                        || self.legacy_open
                        || !query.is_empty()
                        || !m.legacy)
                    && (m.name.to_lowercase().contains(&query)
                        || m.id.to_lowercase().contains(&query))
            })
            .cloned()
            .collect()
    }
    fn change_filter(&mut self, filter: Filter, cx: &mut Context<Self>) {
        if self.passive {
            return;
        }
        self.filter = filter;
        self.discover(cx);
        self.cursor = 0;
        self.keyboard_cursor = false;
        self.legacy_open = false;
        cx.notify();
    }
    fn choose(&self, model: &Model, cx: &mut Context<Self>) {
        if self.passive {
            return;
        }
        let advertised = matches!(&self.catalogs[index(model.provider)],Catalog::Ready(models) if models.iter().any(|m|m.id==model.id));
        if advertised && (self.allow_switch || model.provider == self.saved.provider) {
            cx.emit(PickerEvent::Select(model.provider, model.id.clone()));
        }
    }
    fn key(&mut self, event: &gpui::KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.passive {
            return;
        }
        let visible = self.visible(cx);
        let search_focused = self.search.focus_handle(cx).is_focused(window);
        let list_focused = self.focus.contains_focused(window, cx);
        let list_root_focused = self.focus.is_focused(window);
        if list_focused
            && !list_root_focused
            && matches!(event.keystroke.key.as_str(), "up" | "down")
        {
            if let Some(i) = visible.iter().position(|m| {
                self.row_focus
                    .get(&format!("{:?}:{}", m.provider, m.id))
                    .is_some_and(|f| f.0.is_focused(window) || f.1.is_focused(window))
            }) {
                self.cursor = i;
            }
        }
        match event.keystroke.key.as_str() {
            "escape" => {
                cx.emit(PickerEvent::Dismiss);
                cx.stop_propagation();
            }
            "down" | "up" if search_focused || list_focused => {
                if !visible.is_empty() {
                    self.cursor = if search_focused && !self.keyboard_cursor {
                        0
                    } else if event.keystroke.key == "down" {
                        (self.cursor + 1).min(visible.len() - 1)
                    } else {
                        self.cursor.saturating_sub(1)
                    };
                    self.keyboard_cursor = true;
                    if list_focused && !list_root_focused {
                        if let Some(model) = visible.get(self.cursor) {
                            if let Some(focus) = self
                                .row_focus
                                .get(&format!("{:?}:{}", model.provider, model.id))
                            {
                                focus.0.focus(window, cx);
                            }
                        }
                    }
                }
                cx.stop_propagation();
                cx.notify();
            }
            "enter" if search_focused || list_root_focused => {
                if let Some(model) = visible.get(self.cursor.min(visible.len().saturating_sub(1))) {
                    self.choose(model, cx);
                }
                cx.stop_propagation();
            }
            _ => {}
        }
    }
}
impl Render for ModelPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let values = self.visible(cx);
        self.cursor = self.cursor.min(values.len().saturating_sub(1));
        let scale = f32::from(window.rem_size()) / crate::zoom::REM_BASE;
        let width = PICKER_W.min(f32::from(window.viewport_size().width) / scale - SPACE_3XL);
        let refreshing = self.refreshing.iter().any(|pending| *pending);
        let search_navigation = self.keyboard_cursor
            && (self.search.focus_handle(cx).is_focused(window) || self.focus.is_focused(window));
        let mut rail = div()
            .debug_selector(|| "model-provider-rail".into())
            .w(rpx(RAIL_W))
            .flex_shrink_0()
            .self_stretch()
            .border_r_1()
            .border_color(c::BORDER_SOFT())
            .py(rpx(SPACE_MD))
            .items_center()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_SM));
        for (i, filter, glyph, name) in [
            (0, Filter::Favorites, "star", "Favorite models"),
            (
                1,
                Filter::Provider(codex::Provider::Codex),
                "codex",
                "Codex models",
            ),
            (
                2,
                Filter::Provider(codex::Provider::Claude),
                "claude",
                "Claude models",
            ),
        ] {
            if let Filter::Provider(provider) = filter {
                if !self.allow_switch && provider != self.saved.provider {
                    continue;
                }
            }
            let active = self.filter == filter;
            rail = rail.child(
                header_control("model-provider", name)
                    .id(("model-provider", i))
                    .debug_selector(move || format!("model-provider-{i}"))
                    .role(gpui::Role::RadioButton)
                    .aria_selected(active)
                    .track_focus(&self.rail_focus[i])
                    .tab_index(0)
                    .tab_stop(!self.passive)
                    .w(rpx(RAIL_TARGET_H))
                    .h(rpx(RAIL_TARGET_H))
                    .relative()
                    .when(active, |r| {
                        r.bg(c::BG_HOVER()).child(
                            div()
                                .absolute()
                                .left(-rpx(SPACE_MD))
                                .top(rpx(SPACE_LG))
                                .w(rpx(RAIL_ACTIVE_W))
                                .h(rpx(SPACE_3XL))
                                .rounded(rpx(RADIUS_FULL))
                                .bg(c::FG_MUTE()),
                        )
                    })
                    .focus_visible(|r| r.bg(c::BG()))
                    .child(crate::icons::icon(glyph, ICON_LG, c::FG_DIM()))
                    .on_click(cx.listener(move |this, _, _, cx| this.change_filter(filter, cx)))
                    .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                        if action_key(event) {
                            this.change_filter(filter, cx);
                            cx.stop_propagation();
                        }
                    })),
            );
        }
        let mut rows = div()
            .id("model-picker-rows")
            .debug_selector(|| "model-picker-rows".into())
            .track_focus(&self.focus)
            .tab_index(0)
            .tab_stop(!self.passive)
            .role(gpui::Role::ListBox)
            .aria_label("Available models")
            .flex_shrink_0()
            .p(rpx(SPACE_LG))
            .flex()
            .flex_col()
            .gap(rpx(SPACE_XS));
        for (i, model) in values.iter().enumerate() {
            let selected = self.saved.provider == model.provider
                && self.selected.as_deref() == Some(model.id.as_str());
            let favorite = cx
                .global::<SettingsState>()
                .store
                .is_model_favorite(model.provider, &model.id);
            let focus = self
                .row_focus
                .entry(format!("{:?}:{}", model.provider, model.id))
                .or_insert_with(|| (cx.focus_handle(), cx.focus_handle()))
                .clone();
            let picked = model.clone();
            let keyboard = model.clone();
            let favorite_pointer = model.clone();
            let favorite_keyboard = model.clone();
            rows = rows.child(
                header_control_frame(
                    format!("model-select-{i}"),
                    format!("Select {} · {}", model.name, model.provider.label()),
                    c::BG_HOVER(),
                )
                .debug_selector(move || format!("model-picker-row-{i}"))
                .role(gpui::Role::ListBoxOption)
                .track_focus(&focus.0)
                .aria_selected(selected)
                .tab_index(0)
                .tab_stop(!self.passive)
                .w_full()
                .h_auto()
                .min_h(rpx(MODEL_ROW_H))
                .p(rpx(SPACE_LG))
                .gap(rpx(SPACE_LG))
                // Only the row owns the model highlight, including its action slots.
                // An idle search cursor must not look like a hovered model.
                .when(selected || (search_navigation && i == self.cursor), |row| {
                    row.bg(c::BG_HOVER()).text_color(c::FG())
                })
                .focus_visible(|row| row.bg(c::BG_HOVER()))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(rpx(SPACE_XS))
                        .child(
                            div()
                                .id(("model-picker-name", i))
                                .text_color(
                                    if selected || (search_navigation && i == self.cursor) {
                                        c::FG()
                                    } else {
                                        c::FG_DIM()
                                    },
                                )
                                .group_hover("neutral-control", |name| name.text_color(c::FG()))
                                .text_size(rpx(TEXT_BODY))
                                .line_height(rpx(MODEL_NAME_LINE_H))
                                .truncate()
                                .child(model.name.clone()),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(rpx(SPACE_SM))
                                .text_color(c::FG_DIM())
                                .text_size(rpx(TEXT_MICRO))
                                .line_height(rpx(TEXT_BODY))
                                .child(crate::icons::icon(
                                    model.provider.agent().icon_name(),
                                    ICON_XS,
                                    c::FG_DIM(),
                                ))
                                .child(model.provider.label()),
                        ),
                )
                .child(
                    div()
                        .debug_selector(move || format!("model-check-{i}"))
                        .w(rpx(ICON_SM))
                        .flex_shrink_0()
                        .when(selected, |slot| {
                            slot.child(crate::icons::icon("check", ICON_SM, c::FG_DIM()))
                        }),
                )
                .child(
                    header_control(
                        "model-favorite",
                        if favorite {
                            "Remove favorite"
                        } else {
                            "Add favorite"
                        },
                    )
                    .id(("model-favorite", i))
                    .aria_label(format!(
                        "{} {} favorite",
                        if favorite { "Remove" } else { "Add" },
                        model.name
                    ))
                    .debug_selector(move || format!("model-favorite-{i}"))
                    .tab_index(0)
                    .tab_stop(!self.passive)
                    .role(gpui::Role::CheckBox)
                    .track_focus(&focus.1)
                    .aria_toggled(if favorite {
                        gpui::Toggled::True
                    } else {
                        gpui::Toggled::False
                    })
                    .focus_visible(|target| target.bg(c::BG_HOVER()))
                    .child(crate::icons::icon(
                        if favorite { "star-filled" } else { "star" },
                        ICON_SM,
                        c::FG_DIM(),
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.passive {
                            return;
                        }
                        SettingsState::update(cx, |store| {
                            store.toggle_model_favorite(
                                favorite_pointer.provider,
                                &favorite_pointer.id,
                            );
                        });
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .on_key_down(cx.listener(
                        move |this, event: &gpui::KeyDownEvent, _, cx| {
                            if this.passive {
                                return;
                            }
                            if action_key(event) {
                                SettingsState::update(cx, |store| {
                                    store.toggle_model_favorite(
                                        favorite_keyboard.provider,
                                        &favorite_keyboard.id,
                                    );
                                });
                                cx.stop_propagation();
                                cx.notify();
                            }
                        },
                    )),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.choose(&picked, cx)))
                .on_key_down(cx.listener(
                    move |this, event: &gpui::KeyDownEvent, _, cx| {
                        if action_key(event) {
                            this.choose(&keyboard, cx);
                            cx.stop_propagation();
                        }
                    },
                )),
            );
        }
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            if !self.needs_provider(provider, cx) {
                continue;
            }
            let refresh_error = self.refresh_errors[index(provider)].as_ref();
            if self.refreshing[index(provider)]
                && matches!(self.catalogs[index(provider)], Catalog::Ready(_))
            {
                rows = rows.child(
                    div()
                        .p(rpx(SPACE_LG))
                        .text_color(c::FG_DIM())
                        .child(format!("Refreshing {} models…", provider.label())),
                );
            }
            match &self.catalogs[index(provider)] {
                Catalog::Loading => {
                    rows = rows.child(
                        div()
                            .p(rpx(SPACE_LG))
                            .text_color(c::FG_DIM())
                            .child(format!("Loading {} models…", provider.label())),
                    );
                }
                Catalog::Error(_) | Catalog::Ready(_)
                    if refresh_error.is_some()
                        || matches!(&self.catalogs[index(provider)], Catalog::Error(_)) =>
                {
                    let error = refresh_error
                        .or_else(|| match &self.catalogs[index(provider)] {
                            Catalog::Error(error) => Some(error),
                            _ => None,
                        })
                        .cloned()
                        .unwrap_or_default();
                    rows = rows.child(
                        div()
                            .p(rpx(SPACE_LG))
                            .flex()
                            .flex_col()
                            .gap(rpx(SPACE_SM))
                            .child(
                                div()
                                    .text_color(c::FORM_ERROR())
                                    .whitespace_normal()
                                    .child(format!("{}: {error}", provider.label())),
                            )
                            .child(
                                header_control_frame(
                                    format!("model-retry-{}", index(provider)),
                                    format!("Retry {} models", provider.label()),
                                    c::BG_HOVER(),
                                )
                                .w_auto()
                                .track_focus(&self.retry_focus[index(provider)])
                                .tab_index(0)
                                .tab_stop(!self.passive)
                                .gap(rpx(SPACE_SM))
                                .child(crate::icons::icon("refresh", ICON_SM, c::FG_DIM()))
                                .child("Retry")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.load(provider, false, cx);
                                }))
                                .on_key_down(cx.listener(
                                    move |this, event: &gpui::KeyDownEvent, _, cx| {
                                        if action_key(event) {
                                            this.load(provider, false, cx);
                                            cx.stop_propagation();
                                        }
                                    },
                                )),
                            ),
                    );
                }
                Catalog::Unloaded | Catalog::Ready(_) | Catalog::Error(_) => {}
            }
        }
        let all_ready = [codex::Provider::Codex, codex::Provider::Claude]
            .into_iter()
            .filter(|provider| self.needs_provider(*provider, cx))
            .all(|provider| matches!(self.catalogs[index(provider)], Catalog::Ready(_)));
        if values.is_empty() && all_ready {
            rows = rows.child(
                div()
                    .p(rpx(SPACE_LG))
                    .text_color(c::FG_DIM())
                    .whitespace_normal()
                    .child(if self.search.read(cx).value().is_empty() {
                        if self.filter == Filter::Favorites {
                            if cx.global::<SettingsState>().store.model_favorites.iter().any(|favorite| self.allow_switch || favorite.provider == self.saved.provider) {
                                "No saved favorites are currently available. Choose a provider to browse models."
                            } else {
                                "No favorite models yet. Star a model in a provider list."
                            }
                        } else {
                            "No models to show."
                        }
                    } else {
                        "No matching models. Try another name or provider."
                    }),
            );
        }
        let legacy_count = self
            .catalogs
            .iter()
            .filter_map(|c| {
                if let Catalog::Ready(m) = c {
                    Some(m)
                } else {
                    None
                }
            })
            .flatten()
            .filter(|m| {
                m.legacy
                    && (self.allow_switch || m.provider == self.saved.provider)
                    && match self.filter {
                        Filter::Provider(p) => m.provider == p,
                        Filter::Favorites => cx
                            .global::<SettingsState>()
                            .store
                            .is_model_favorite(m.provider, &m.id),
                    }
            })
            .count();
        if legacy_count > 0
            && self.filter != Filter::Favorites
            && self.search.read(cx).value().is_empty()
        {
            rows = rows.child(
                header_control_frame(
                    "model-picker-legacy",
                    "Show or hide legacy models",
                    c::BG_HOVER(),
                )
                .w_full()
                .h_auto()
                .min_h(rpx(MODEL_ROW_H))
                .track_focus(&self.legacy_focus)
                .tab_index(0)
                .tab_stop(!self.passive)
                .gap(rpx(SPACE_SM))
                .child(crate::icons::icon(
                    if self.legacy_open {
                        "chev-down"
                    } else {
                        "chev-right"
                    },
                    ICON_SM,
                    c::FG_DIM(),
                ))
                .child(format!("Legacy models · {legacy_count}"))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.legacy_open = !this.legacy_open;
                    cx.notify();
                }))
                .on_key_down(cx.listener(
                    |this, event: &gpui::KeyDownEvent, _, cx| {
                        if action_key(event) {
                            this.legacy_open = !this.legacy_open;
                            cx.stop_propagation();
                            cx.notify();
                        }
                    },
                )),
            );
        }
        div()
            .id("provider-model-picker")
            .debug_selector(|| "provider-model-picker".into())
            .w(rpx(width))
            .h_auto()
            .min_h(rpx(RAIL_TARGET_H * 3.0 + SPACE_SM * 2.0 + SPACE_MD * 2.0))
            .rounded(rpx(RADIUS_PANEL))
            .border_1()
            .border_color(c::BORDER())
            .bg(c::BG_RAIL())
            .text_size(rpx(TEXT_SMALL))
            .flex()
            .overflow_hidden()
            .when(!self.passive, |picker| picker.occlude()
                .capture_key_down(cx.listener(Self::key))
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(PickerEvent::Dismiss))))
            .child(rail)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .h(rpx(SEARCH_H))
                            .w_full()
                            .px(rpx(SPACE_2XL))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .gap(rpx(SPACE_SM))
                            .border_b_1()
                            .border_color(c::BORDER_SOFT())
                            .when(self.search.focus_handle(cx).is_focused(window), |r| {
                                r.bg(c::BG_HOVER())
                            })
                            .child(crate::icons::icon("search", ICON_SM, c::FG_DIM()))
                            .child(
                                Input::new(&self.search).file_drop_overlay(crate::views::components::file_drop_overlay())
                                    .aria_label("Search models")
                                    .appearance(false)
                                    .bordered(false)
                                    .focus_bordered(false)
                                    .p_0()
                                    .flex_1()
                                    .min_w_0(),
                            )
                            .child(
                                (if refreshing || self.passive {
                                    disabled_header_control("model-refresh", "Refresh models")
                                } else {
                                    header_control("model-refresh", "Refresh models")
                                })
                                    .debug_selector(|| "model-refresh".into())
                                    .track_focus(&self.refresh_focus)
                                    .tab_index(0)
                                    .tab_stop(!self.passive && !refreshing)
                                    .child(crate::icons::icon("refresh", ICON_SM, c::FG_DIM()))
                                    .on_click(cx.listener(|this, _, _, cx| this.refresh_visible(cx)))
                                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                                        if action_key(event) {
                                            this.refresh_visible(cx);
                                            cx.stop_propagation();
                                        }
                                    })),
                            ),
                    )
                    .child(rows),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    pub(super) fn seed(picker: &mut ModelPicker) {
        picker.catalogs = [
            Catalog::Ready(models(
                codex::Provider::Codex,
                vec![
                    serde_json::json!({"model":"code-1","displayName":"Code model"}),
                    serde_json::json!({"model":"code-2","displayName":"Second code model"}),
                ],
            )),
            Catalog::Ready(models(
                codex::Provider::Claude,
                vec![
                    serde_json::json!({"model":"claude-1","displayName":"Claude model with a deliberately long advertised name"}),
                    serde_json::json!({"model":"claude-old","displayName":"Older Claude model","isLegacy":true}),
                ],
            )),
        ];
    }
    fn state() -> codex::Snapshot {
        let mut state = super::super::tests::snapshot();
        state.saved.provider = codex::Provider::Codex;
        state.model = Some("code-1".into());
        state.saved.draft = "Keep this draft".into();
        state
    }
    fn draw(cx: &mut gpui::VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            let _ = window.draw(cx);
        });
    }
    #[gpui::test]
    fn discovery_only_loads_browsed_or_favorited_providers(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
        });
        let (_, _) = cx.add_window_view(|window, cx| {
            let mut state = state();
            state.saved.cwd = "/missing-catalog-fixture".into();
            state.models.clear();
            let picker = cx.new(|cx| {
                let mut picker = ModelPicker::new(&state, true, window, cx);
                picker.discover(cx);
                assert!(matches!(picker.catalogs[0], Catalog::Error(_)));
                assert!(matches!(picker.catalogs[1], Catalog::Unloaded));
                picker.change_filter(Filter::Provider(codex::Provider::Claude), cx);
                assert!(matches!(picker.catalogs[1], Catalog::Error(_)));
                picker
            });
            picker.update(cx, |picker, cx| {
                picker.catalogs = [Catalog::Unloaded, Catalog::Unloaded];
                cx.global_mut::<SettingsState>()
                    .store
                    .toggle_model_favorite(codex::Provider::Claude, "claude-1");
                picker.change_filter(Filter::Favorites, cx);
                assert!(matches!(picker.catalogs[0], Catalog::Unloaded));
                assert!(matches!(picker.catalogs[1], Catalog::Error(_)));
                picker.allow_switch = false;
                picker.catalogs = [Catalog::Unloaded, Catalog::Unloaded];
                picker.discover(cx);
                assert!(
                    picker
                        .catalogs
                        .iter()
                        .all(|catalog| matches!(catalog, Catalog::Unloaded)),
                    "running Codex must not discover a Claude favorite"
                );
            });
            gpui_component::Root::new(picker, window, cx).bordered(false)
        });
    }

    #[gpui::test]
    fn refresh_retains_models_and_rejects_obsolete_results(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
        });
        let (_, _) = cx.add_window_view(|window, cx| {
            let picker = cx.new(|cx| {
                let mut picker = ModelPicker::new(&state(), true, window, cx);
                seed(&mut picker);
                picker.saved.cwd = "/missing-catalog-fixture".into();
                picker.refreshing[1] = true;
                picker.refresh_visible(cx);
                assert!(
                    matches!(picker.catalogs[0], Catalog::Ready(_)),
                    "disabled refresh must not start another provider"
                );
                picker.refreshing[1] = false;
                for provider in [codex::Provider::Codex, codex::Provider::Claude] {
                    let i = index(provider);
                    picker.filter = Filter::Provider(provider);
                    let before = picker
                        .visible(cx)
                        .iter()
                        .map(|m| m.id.clone())
                        .collect::<Vec<_>>();
                    picker.generations[i] = 2;
                    picker.refreshing[i] = true;
                    picker.complete_models(provider, 1, Ok(vec![]), cx);
                    assert!(picker.refreshing[i]);
                    picker.complete_models(provider, 2, Err("offline".into()), cx);
                    assert!(!picker.refreshing[i]);
                    assert_eq!(picker.refresh_errors[i].as_deref(), Some("offline"));
                    assert_eq!(
                        picker
                            .visible(cx)
                            .iter()
                            .map(|m| m.id.clone())
                            .collect::<Vec<_>>(),
                        before
                    );
                    picker.complete_models(
                        provider,
                        2,
                        Ok(vec![serde_json::json!({"model":"new"})]),
                        cx,
                    );
                    assert!(picker.refresh_errors[i].is_none());
                    assert_eq!(picker.visible(cx)[0].id, "new");
                }
                picker
            });
            gpui_component::Root::new(picker, window, cx).bordered(false)
        });
    }

    #[gpui::test]
    fn picker_shows_every_model_without_internal_scrolling(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let picker = cx.new(|cx| {
                let mut picker = ModelPicker::new(&state(), false, window, cx);
                picker.catalogs[0] = Catalog::Ready(
                    (0..8)
                        .map(|i| Model {
                            provider: codex::Provider::Codex,
                            id: format!("model-{i}"),
                            name: format!("Model {i}"),
                            legacy: false,
                        })
                        .collect(),
                );
                picker
            });
            gpui_component::Root::new(picker, window, cx).bordered(false)
        });
        cx.simulate_resize(gpui::size(gpui::px(1000.), gpui::px(1200.)));
        draw(cx);
        let panel = cx.debug_bounds("provider-model-picker").unwrap();
        let last = cx.debug_bounds("model-picker-row-7").unwrap();
        assert!(
            panel.size.height > gpui::px(280.),
            "long catalog grows beyond the old cap"
        );
        assert!(
            last.top() >= panel.top() && last.bottom() <= panel.bottom(),
            "last model is visible without scrolling"
        );
        let before = last;
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: panel.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-200.))),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        draw(cx);
        assert_eq!(cx.debug_bounds("model-picker-row-7").unwrap(), before);
    }

    #[gpui::test]
    fn picker_defaults_to_favorites_only_with_scoped_saved_favorites(
        cx: &mut gpui::TestAppContext,
    ) {
        use grove_core::storage::ModelFavorite;
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
        });
        let (_, _) = cx.add_window_view(|window, cx| {
            let mut last = None;
            for (provider, allow_switch, favorite_provider, expected) in [
                (
                    codex::Provider::Codex,
                    true,
                    None,
                    Filter::Provider(codex::Provider::Codex),
                ),
                (
                    codex::Provider::Claude,
                    false,
                    None,
                    Filter::Provider(codex::Provider::Claude),
                ),
                (
                    codex::Provider::Claude,
                    true,
                    Some(codex::Provider::Codex),
                    Filter::Favorites,
                ),
                (
                    codex::Provider::Codex,
                    false,
                    Some(codex::Provider::Claude),
                    Filter::Provider(codex::Provider::Codex),
                ),
                (
                    codex::Provider::Codex,
                    false,
                    Some(codex::Provider::Codex),
                    Filter::Favorites,
                ),
                (
                    codex::Provider::Claude,
                    false,
                    Some(codex::Provider::Claude),
                    Filter::Favorites,
                ),
            ] {
                cx.global_mut::<SettingsState>().store.model_favorites = favorite_provider
                    .into_iter()
                    .map(|provider| ModelFavorite {
                        provider,
                        model: "not-yet-advertised".into(),
                    })
                    .collect();
                let mut state = state();
                state.saved.provider = provider;
                state.models.clear();
                let picker = cx.new(|cx| ModelPicker::new(&state, allow_switch, window, cx));
                assert_eq!(
                    picker.read(cx).filter,
                    expected,
                    "saved favorites choose the initial scope before discovery finishes"
                );
                last = Some(picker);
            }
            gpui_component::Root::new(last.unwrap(), window, cx).bordered(false)
        });
    }

    #[gpui::test]
    fn picker_keyboard_browsing_favorites_and_selection_are_independent(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
        });
        let mut retained = None;
        let mut subscription = None;
        let events = Rc::new(RefCell::new(Vec::new()));
        let captured = events.clone();
        let (_, cx) = cx.add_window_view(|window, cx| {
            let picker = cx.new(|cx| {
                let mut picker = ModelPicker::new(&state(), true, window, cx);
                seed(&mut picker);
                picker
            });
            retained = Some(picker.clone());
            subscription = Some(cx.subscribe(&picker, move |_, _, event: &PickerEvent, _| {
                captured.borrow_mut().push(event.clone());
            }));
            gpui_component::Root::new(picker, window, cx).bordered(false)
        });
        let picker = retained.unwrap();
        draw(cx);
        picker.read_with(cx, |p, _| assert!(!p.keyboard_cursor));
        cx.simulate_keystrokes("down");
        draw(cx);
        picker.read_with(cx, |p, _| {
            assert!(p.keyboard_cursor);
            assert_eq!(p.cursor, 0, "first Down enters the first result");
        });
        cx.simulate_keystrokes("down");
        draw(cx);
        picker.read_with(cx, |p, _| assert_eq!(p.cursor, 1));
        // Pointer favorite activation must not bubble into the enclosing model row.
        let favorite = cx.debug_bounds("model-favorite-0").unwrap();
        let row = cx.debug_bounds("model-picker-row-0").unwrap();
        let panel = cx.debug_bounds("provider-model-picker").unwrap();
        let rail = cx.debug_bounds("model-provider-rail").unwrap();
        assert_eq!(
            rail.size.height,
            panel.size.height - gpui::px(2.0),
            "provider seam spans the content-fitting panel"
        );
        let (row_height, favorite_size, expected_height) = cx.update(|window, _| {
            (
                rpx(MODEL_ROW_H).to_pixels(window.rem_size()),
                rpx(CHROME_CONTROL_H).to_pixels(window.rem_size()),
                rpx(SEARCH_H + SPACE_LG * 2.0 + MODEL_ROW_H * 2.0 + SPACE_XS)
                    .to_pixels(window.rem_size())
                    + gpui::px(2.0),
            )
        });
        assert_eq!(row.size.height, row_height);
        assert_eq!(favorite.size, gpui::size(favorite_size, favorite_size));
        assert_eq!(
            panel.size.height, expected_height,
            "short lists hug their content"
        );
        assert!(favorite.left() >= row.left() && favorite.right() <= row.right());
        cx.simulate_click(favorite.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(
            events.borrow().is_empty(),
            "favorite click must not select a model"
        );
        cx.update(|_, cx| {
            assert!(cx
                .global::<SettingsState>()
                .store
                .is_model_favorite(codex::Provider::Codex, "code-1"));
        });
        cx.simulate_click(favorite.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(events.borrow().is_empty());
        // Browsing changes only the filter, never launch identity or draft.
        let target = cx.debug_bounds("model-provider-2").unwrap().center();
        cx.simulate_click(target, gpui::Modifiers::default());
        draw(cx);
        picker.read_with(cx, |p, cx| {
            assert_eq!(p.saved.provider, codex::Provider::Codex);
            assert_eq!(p.saved.draft, "Keep this draft");
            assert_eq!(p.visible(cx).len(), 1);
        });
        assert!(events.borrow().is_empty());
        // Tab-reachable favorite activation must not be intercepted as model selection.
        cx.update(|window, cx| {
            picker.update(cx, |p, cx| {
                p.row_focus
                    .get("Claude:claude-1")
                    .unwrap()
                    .1
                    .focus(window, cx);
            });
        });
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert!(
            events.borrow().is_empty(),
            "favorite Enter must not select a model"
        );
        cx.update(|_, cx| {
            assert!(cx
                .global::<SettingsState>()
                .store
                .is_model_favorite(codex::Provider::Claude, "claude-1"));
        });
        let target = cx.debug_bounds("model-provider-0").unwrap().center();
        cx.simulate_click(target, gpui::Modifiers::default());
        draw(cx);
        picker.read_with(cx, |p, cx| {
            let values = p.visible(cx);
            assert_eq!(values.len(), 1);
            assert_eq!(values[0].provider, codex::Provider::Claude);
        });
        // Search filters literal names; entering a result emits a composite identity.
        cx.update(|window, cx| {
            picker.update(cx, |p, cx| {
                p.search.update(cx, |input, cx| {
                    input.set_value("long advertised", window, cx);
                });
                p.search.focus_handle(cx).focus(window, cx);
            });
        });
        draw(cx);
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert!(
            matches!(events.borrow().last(),Some(PickerEvent::Select(codex::Provider::Claude,id)) if id=="claude-1")
        );
        events.borrow_mut().clear();
        // A focused row's Enter chooses that row, not the old search cursor.
        cx.update(|window, cx| {
            picker.update(cx, |p, cx| {
                p.change_filter(Filter::Provider(codex::Provider::Codex), cx);
                p.search
                    .update(cx, |input, cx| input.set_value("", window, cx));
            });
        });
        draw(cx);
        cx.update(|window, cx| {
            picker.update(cx, |p, cx| {
                p.row_focus.get("Codex:code-2").unwrap().0.focus(window, cx);
            });
        });
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert!(
            matches!(events.borrow().last(),Some(PickerEvent::Select(codex::Provider::Codex,id)) if id=="code-2")
        );
        events.borrow_mut().clear();
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(matches!(events.borrow().last(), Some(PickerEvent::Dismiss)));
        drop(subscription);
    }

    #[gpui::test]
    fn picker_active_sessions_exclude_other_provider_and_catalogs_are_truthful(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
        });
        let mut retained = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let picker = cx.new(|cx| {
                let mut p = ModelPicker::new(&state(), false, window, cx);
                seed(&mut p);
                p
            });
            retained = Some(picker.clone());
            gpui_component::Root::new(picker, window, cx).bordered(false)
        });
        let picker = retained.unwrap();
        draw(cx);
        assert!(cx.debug_bounds("model-provider-2").is_none());
        cx.update(|_, cx| {
            picker.update(cx, |p, cx| {
                p.filter = Filter::Favorites;
                SettingsState::update(cx, |s| {
                    s.toggle_model_favorite(codex::Provider::Claude, "claude-1");
                });
                assert!(p.visible(cx).is_empty());
            });
        });
        // Malformed and duplicate catalog entries are not selectable rows.
        assert_eq!(
            models(
                codex::Provider::Codex,
                vec![
                    serde_json::json!({"displayName":"missing"}),
                    serde_json::json!({"model":""}),
                    serde_json::json!({"model":"a"}),
                    serde_json::json!({"model":"a"})
                ]
            )
            .len(),
            1
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "opens isolated native model picker QA"]
    fn provider_catalog_native_capture() {
        assert!(std::env::var_os("GROVE_CONFIG_DIR").is_some());
        let output = std::path::PathBuf::from(std::env::var("GROVE_CATALOG_CAPTURE_DIR").unwrap());
        assert!(output.is_absolute());
        fs_err::create_dir_all(&output).unwrap();
        let narrow = std::env::var_os("GROVE_CATALOG_NARROW").is_some();
        gpui_platform::application()
            .with_assets(crate::assets::Assets)
            .run(move |cx| {
                gpui_component::init(cx);
                crate::fonts::register(cx).unwrap();
                cx.set_reduce_motion(true);
                cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
                c::sync_component_theme(cx);
                let mut retained = None;
                let window = cx
                    .open_window(
                        gpui::WindowOptions {
                            window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
                                gpui::point(gpui::px(30.), gpui::px(30.)),
                                gpui::size(
                                    gpui::px(if narrow { 360. } else { 600. }),
                                    gpui::px(540.),
                                ),
                            ))),
                            ..Default::default()
                        },
                        |window, cx| {
                            window.set_window_title("Grove model catalog QA");
                            let picker = cx.new(|cx| {
                                let mut picker = ModelPicker::new(&state(), true, window, cx);
                                picker.saved.cwd = "/missing-catalog-fixture".into();
                                seed(&mut picker);
                                picker
                            });
                            retained = Some(picker.clone());
                            cx.new(|cx| {
                                gpui_component::Root::new(picker, window, cx)
                                    .bordered(false)
                                    .bg(c::BG())
                            })
                        },
                    )
                    .unwrap();
                let picker = retained.unwrap();
                cx.activate(true);
                cx.spawn(async move |cx| {
                    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
                        for mode in ["ready", "refreshing", "failure"] {
                            cx.update_window(window.into(), |_, window, cx| {
                                picker.update(cx, |picker, cx| {
                                    seed(picker);
                                    picker.filter = Filter::Provider(provider);
                                    picker.refreshing = [false; 2];
                                    picker.refresh_errors = [None, None];
                                    if mode == "refreshing" {
                                        picker.refreshing[index(provider)] = true;
                                    }
                                    if mode == "failure" {
                                        picker.refresh_errors[index(provider)] = Some(
                                            "Catalog refresh unavailable; cached models retained."
                                                .into(),
                                        );
                                    }
                                    picker.refresh_focus.focus(window, cx);
                                    cx.notify();
                                });
                                window.refresh();
                            })
                            .unwrap();
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(250))
                                .await;
                            let file = output.join(format!("{provider:?}-{mode}.png"));
                            assert!(std::process::Command::new("import")
                                .args(["-window", "Grove model catalog QA"])
                                .arg(file)
                                .status()
                                .unwrap()
                                .success());
                        }
                    }
                    cx.update_window(window.into(), |_, window, cx| {
                        picker.update(cx, |picker, cx| {
                            picker.refresh_errors = [None, None];
                            picker.refresh_focus.focus(window, cx);
                        });
                        window.dispatch_keystroke(gpui::Keystroke::parse("space").unwrap(), cx);
                        picker.read_with(cx, |picker, _| {
                            assert!(
                                matches!(picker.catalogs[1], Catalog::Error(_)),
                                "Space activates refresh on the focused native control"
                            );
                            assert!(
                                matches!(picker.catalogs[0], Catalog::Ready(_)),
                                "refresh preserves the other provider"
                            );
                        });
                        window.blur();
                        window.refresh();
                    })
                    .unwrap();
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(100))
                        .await;
                    cx.update_window(window.into(), |_, window, _| window.remove_window())
                        .unwrap();
                    drop(picker);
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(100))
                        .await;
                    cx.update(|cx| cx.quit());
                })
                .detach();
            });
    }

    #[test]
    #[ignore = "requires macOS Metal renderer; writes native picker PNGs to /tmp"]
    #[cfg(target_os = "macos")]
    fn provider_picker_native_snapshots() {
        let mut cx = gpui::HeadlessAppContext::with_platform(
            gpui_platform::current_platform(true).text_system(),
            Arc::new(crate::assets::Assets),
            gpui_platform::current_headless_renderer,
        );
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::fonts::register(cx).unwrap();
            cx.set_reduce_motion(true);
            let mut store = grove_core::storage::Store::default();
            store.toggle_model_favorite(codex::Provider::Codex, "code-1");
            store.toggle_model_favorite(codex::Provider::Claude, "claude-1");
            cx.set_global(SettingsState::new(store));
        });
        let previous = grove_core::theme::current().name.clone();
        for appearance in ["dark", "light"] {
            cx.update(|cx| {
                assert!(grove_core::theme::set_by_name(&format!(
                    "grove-{appearance}"
                )));
                c::sync_component_theme(cx);
            });
            for (width, rem) in [(420., 16.), (336., 16.), (420., 24.)] {
                cx.update(|cx| gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem));
                let mut modes = vec!["claude", "favorites", "empty", "codex"];
                if appearance == "dark" && rem == 16.0 {
                    modes.extend(["hover", "keyboard", "favorite-focus"]);
                    if width == 420.0 {
                        modes.push("overflow");
                    }
                }
                for mode in modes {
                    let mut retained = None;
                    let window = cx
                        .open_window(gpui::size(gpui::px(width), gpui::px(480.)), |window, cx| {
                            window.set_rem_size(gpui::px(rem));
                            let picker = cx.new(|cx| {
                                let mut p = ModelPicker::new(&state(), true, window, cx);
                                seed(&mut p);
                                if ["codex", "hover", "keyboard", "favorite-focus", "overflow"].contains(&mode) {
                                    p.filter = Filter::Provider(codex::Provider::Codex);
                                    p.catalogs[0] = Catalog::Ready(models(codex::Provider::Codex, vec![
                                        serde_json::json!({"model":"code-1","displayName":"GPT-6.1-Sol"}),
                                        serde_json::json!({"model":"code-2","displayName":"GPT-6.1 Terra"}),
                                        serde_json::json!({"model":"code-3","displayName":"GPT-6-Luna"}),
                                    ]));
                                    p.selected = Some("code-2".into());
                                    if mode == "overflow" {
                                        if let Catalog::Ready(models) = &mut p.catalogs[0] {
                                            for index in 4..=12 {
                                                models.push(Model {
                                                    provider: codex::Provider::Codex,
                                                    id: format!("code-{index}"),
                                                    name: format!("Additional advertised model {index}"),
                                                    legacy: false,
                                                });
                                            }
                                        }
                                    }
                                } else if mode != "favorites" {
                                    p.filter = Filter::Provider(codex::Provider::Claude);
                                }
                                if mode == "empty" {
                                    p.search.update(cx, |input, cx| {
                                        input.set_value("no-matching-model", window, cx);
                                    });
                                }
                                p
                            });
                            retained = Some(picker.clone());
                            cx.new(|cx| {
                                gpui_component::Root::new(picker, window, cx)
                                    .bordered(false)
                                    .bg(c::BG())
                            })
                        })
                        .unwrap();
                    for _ in 0..4 {
                        cx.advance_clock(std::time::Duration::from_millis(16));
                        cx.run_until_parked();
                        cx.update_window(window.into(), |_, window, cx| {
                            window.simulate_next_frame(cx);
                            let _ = window.draw(cx);
                        })
                        .unwrap();
                    }
                    if ["hover", "keyboard", "favorite-focus"].contains(&mode) {
                        cx.update_window(window.into(), |_, window, cx| {
                            if mode == "hover" {
                                // Route a real GPUI pointer event into the first model row.
                                window.simulate_mouse_move(
                                    gpui::point(
                                        rpx(RAIL_W + SPACE_LG * 3.0).to_pixels(window.rem_size()),
                                        rpx(SEARCH_H + SPACE_LG + MODEL_ROW_H / 2.0)
                                            .to_pixels(window.rem_size()),
                                    ),
                                    cx,
                                );
                            } else if mode == "keyboard" {
                                window.dispatch_keystroke(
                                    gpui::Keystroke::parse("down").unwrap(),
                                    cx,
                                );
                            } else {
                                retained.as_ref().unwrap().update(cx, |p, cx| {
                                    p.row_focus.get("Codex:code-2").unwrap().1.focus(window, cx);
                                    cx.notify();
                                });
                                // Explicit target focus plus a routed keyboard event enables
                                // focus_visible without claiming this fixture tested Tab routing.
                                window.dispatch_keystroke(
                                    gpui::Keystroke::parse("right").unwrap(),
                                    cx,
                                );
                            }
                            window.simulate_next_frame(cx);
                            let _ = window.draw(cx);
                        })
                        .unwrap();
                        cx.run_until_parked();
                    }
                    let path = format!(
                        "/tmp/grove-provider-picker-{appearance}-{width:.0}-{rem:.0}-{mode}.png"
                    );
                    let screenshot = cx.capture_screenshot(window.into()).unwrap();
                    screenshot.save(&path).unwrap();
                    eprintln!("Native provider picker: {path}");
                    cx.update_window(window.into(), |_, window, cx| {
                        let p = retained.as_ref().unwrap().read(cx);
                        assert_eq!(p.saved.provider, codex::Provider::Codex);
                        assert_eq!(window.rem_size(), gpui::px(rem));
                        window.remove_window();
                    })
                    .unwrap();
                }
            }
        }
        cx.update(|cx| {
            assert!(grove_core::theme::set_by_name(&previous));
            c::sync_component_theme(cx);
        });
    }
}
