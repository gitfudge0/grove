//! Provider browsing is independent of the conversation's selected provider.
use super::*;
use crate::settings::SettingsState;
use serde_json::Value;

const PICKER_W: f32 = 336.0;
const PICKER_H: f32 = 280.0;
const RAIL_W: f32 = 44.0;
const RAIL_ACTIVE_W: f32 = 2.0;
const MODEL_ROW_H: f32 = 42.0;

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
    Loading,
    Ready(Vec<Model>),
    Error(String),
}
#[derive(Clone, Debug)]
pub(super) enum PickerEvent {
    Select(codex::Provider, String),
    Dismiss,
}
pub(super) struct ModelPicker {
    saved: codex::SavedSession,
    allow_switch: bool,
    selected: Option<String>,
    filter: Filter,
    catalogs: [Catalog; 2],
    generations: [u64; 2],
    search: Entity<InputState>,
    cursor: usize,
    legacy_open: bool,
    focus: FocusHandle,
    rail_focus: [FocusHandle; 3],
    row_focus: HashMap<String, (FocusHandle, FocusHandle)>,
    retry_focus: [FocusHandle; 2],
    legacy_focus: FocusHandle,
    scroll: gpui::ScrollHandle,
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
            this.scroll
                .set_offset(gpui::point(gpui::px(0.), gpui::px(0.)));
            cx.notify();
        });
        let mut catalogs = [Catalog::Loading, Catalog::Loading];
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
            search,
            cursor: 0,
            legacy_open: false,
            focus: cx.focus_handle(),
            rail_focus: std::array::from_fn(|_| cx.focus_handle()),
            row_focus: HashMap::new(),
            retry_focus: std::array::from_fn(|_| cx.focus_handle()),
            legacy_focus: cx.focus_handle(),
            scroll: gpui::ScrollHandle::new(),
            _search_events: events,
        };
        picker.search.focus_handle(cx).focus(window, cx);
        picker
    }
    #[cfg(test)]
    pub(super) fn seed_test_catalogs(&mut self) {
        tests::seed(self);
    }
    pub(super) fn discover(&mut self, cx: &mut Context<Self>) {
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            if (self.allow_switch || provider == self.saved.provider)
                && matches!(self.catalogs[index(provider)], Catalog::Loading)
            {
                self.load(provider, cx);
            }
        }
    }
    fn load(&mut self, provider: codex::Provider, cx: &mut Context<Self>) {
        let i = index(provider);
        self.generations[i] = self.generations[i].wrapping_add(1);
        let generation = self.generations[i];
        self.catalogs[i] = Catalog::Loading;
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
        let task = cx.background_spawn(async move { codex::discover_composer(saved, args) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.generations[i] != generation {
                    return;
                }
                this.catalogs[i] = match result {
                    Ok(state) => Catalog::Ready(models(provider, state.models)),
                    Err(error) => Catalog::Error(error),
                };
                cx.notify();
            });
        })
        .detach();
        cx.notify();
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
        self.filter = filter;
        self.cursor = 0;
        self.legacy_open = false;
        self.scroll
            .set_offset(gpui::point(gpui::px(0.), gpui::px(0.)));
        cx.notify();
    }
    fn choose(&self, model: &Model, cx: &mut Context<Self>) {
        let advertised = matches!(&self.catalogs[index(model.provider)],Catalog::Ready(models) if models.iter().any(|m|m.id==model.id));
        if advertised && (self.allow_switch || model.provider == self.saved.provider) {
            cx.emit(PickerEvent::Select(model.provider, model.id.clone()));
        }
    }
    fn key(&mut self, event: &gpui::KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
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
                    self.cursor = if event.keystroke.key == "down" {
                        (self.cursor + 1).min(visible.len() - 1)
                    } else {
                        self.cursor.saturating_sub(1)
                    };
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
                    let y = -(self.cursor as f32 * MODEL_ROW_H - MODEL_ROW_H).max(0.);
                    self.scroll.set_offset(gpui::point(
                        gpui::px(0.),
                        rpx(y).to_pixels(window.rem_size()),
                    ));
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
        let mut rail = div()
            .w(rpx(RAIL_W))
            .flex_shrink_0()
            .h_full()
            .border_r_1()
            .border_color(c::BORDER_SOFT())
            .px(rpx(SPACE_SM))
            .py(rpx(SPACE_LG))
            .flex()
            .flex_col()
            .gap(rpx(SPACE_LG));
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
                header_control_with_tooltip("model-provider", name, move |window, cx| {
                    tooltip(name, window).build(window, cx)
                })
                .id(("model-provider", i))
                .debug_selector(move || format!("model-provider-{i}"))
                .role(gpui::Role::RadioButton)
                .aria_selected(active)
                .track_focus(&self.rail_focus[i])
                .tab_index(0)
                .w_full()
                .h(rpx(CHROME_CONTROL_H))
                .relative()
                .when(active, |r| {
                    r.bg(c::BG_HOVER()).child(
                        div()
                            .absolute()
                            .left(-rpx(SPACE_SM))
                            .top(rpx(SPACE_SM))
                            .w(rpx(RAIL_ACTIVE_W))
                            .h(rpx(SPACE_3XL))
                            .rounded(rpx(RADIUS_FULL))
                            .bg(c::FG_MUTE()),
                    )
                })
                .focus_visible(|r| r.bg(c::BG()))
                .child(crate::icons::icon(glyph, ICON_LG, c::FG_DIM()))
                .on_click(cx.listener(move |this, _, _, cx| this.change_filter(filter, cx)))
                .on_key_down(cx.listener(
                    move |this, event: &gpui::KeyDownEvent, _, cx| {
                        if action_key(event) {
                            this.change_filter(filter, cx);
                            cx.stop_propagation();
                        }
                    },
                )),
            );
        }
        let mut rows = div()
            .id("model-picker-rows")
            .debug_selector(|| "model-picker-rows".into())
            .track_focus(&self.focus)
            .tab_index(0)
            .role(gpui::Role::ListBox)
            .aria_label("Available models")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .p(rpx(SPACE_LG))
            .flex()
            .flex_col();
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
            let full_name = format!("{} · {}", model.name, model.provider.label());
            let picked = model.clone();
            let keyboard = model.clone();
            let toggled = model.clone();
            rows = rows.child(
                div()
                    .id(("model-picker-row", i))
                    .debug_selector(move || format!("model-picker-row-{i}"))
                    .min_h(rpx(MODEL_ROW_H))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_SM))
                    .rounded(rpx(RADIUS_CONTROL))
                    .when(i == self.cursor, |r| r.bg(c::BG_HOVER()))
                    .child(
                        header_control_frame(
                            format!("model-select-{i}"),
                            format!("Select {} · {}", model.name, model.provider.label()),
                            c::BG_HOVER(),
                        )
                        .tooltip(move |window, cx| {
                            tooltip(full_name.clone(), window).build(window, cx)
                        })
                        .role(gpui::Role::ListBoxOption)
                        .track_focus(&focus.0)
                        .aria_selected(selected)
                        .tab_index(0)
                        .h_auto()
                        .min_h(rpx(MODEL_ROW_H))
                        .flex_1()
                        .min_w_0()
                        .px(rpx(SPACE_LG))
                        .gap(rpx(SPACE_SM))
                        .focus_visible(|r| r.bg(c::BG()))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .text_color(c::FG())
                                        .text_size(rpx(TEXT_BODY))
                                        .truncate()
                                        .child(model.name.clone()),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(rpx(SPACE_SM))
                                        .text_color(c::FG_MUTE())
                                        .text_size(rpx(TEXT_MICRO))
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
                                .w(rpx(ICON_SM))
                                .flex_shrink_0()
                                .when(selected, |slot| {
                                    slot.child(crate::icons::icon("check", ICON_SM, c::FG_DIM()))
                                }),
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
                    )
                    .child(
                        header_control_frame(
                            format!("model-favorite-{i}"),
                            format!(
                                "{} {} favorite",
                                if favorite { "Remove" } else { "Add" },
                                model.name
                            ),
                            c::BG_HOVER(),
                        )
                        .debug_selector(move || format!("model-favorite-{i}"))
                        .tab_index(0)
                        .role(gpui::Role::CheckBox)
                        .track_focus(&focus.1)
                        .aria_toggled(if favorite {
                            gpui::Toggled::True
                        } else {
                            gpui::Toggled::False
                        })
                        .focus_visible(|r| r.bg(c::BG()))
                        .child(crate::icons::icon(
                            if favorite { "star-filled" } else { "star" },
                            ICON_SM,
                            c::FG_DIM(),
                        ))
                        .on_click(cx.listener(move |_, _, _, cx| {
                            SettingsState::update(cx, |store| {
                                store.toggle_model_favorite(toggled.provider, &toggled.id);
                            });
                            cx.notify();
                        }))
                        .on_key_down(cx.listener(
                            move |this, event: &gpui::KeyDownEvent, _, cx| {
                                if action_key(event) {
                                    if let Some(m) = this.visible(cx).get(i) {
                                        let m = m.clone();
                                        SettingsState::update(cx, |store| {
                                            store.toggle_model_favorite(m.provider, &m.id);
                                        });
                                    }
                                    cx.stop_propagation();
                                    cx.notify();
                                }
                            },
                        )),
                    ),
            );
        }
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            if !self.allow_switch && provider != self.saved.provider {
                continue;
            }
            if matches!(self.filter, Filter::Provider(p) if p != provider) {
                continue;
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
                Catalog::Error(error) => {
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
                                .gap(rpx(SPACE_SM))
                                .child(crate::icons::icon("restart", ICON_SM, c::FG_DIM()))
                                .child("Retry")
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.load(provider, cx)),
                                )
                                .on_key_down(cx.listener(
                                    move |this, event: &gpui::KeyDownEvent, _, cx| {
                                        if action_key(event) {
                                            this.load(provider, cx);
                                            cx.stop_propagation();
                                        }
                                    },
                                )),
                            ),
                    );
                }
                Catalog::Ready(_) => {}
            }
        }
        let all_ready = [codex::Provider::Codex, codex::Provider::Claude]
            .into_iter()
            .filter(|provider| {
                (self.allow_switch || *provider == self.saved.provider)
                    && !matches!(self.filter,Filter::Provider(p) if p!=*provider)
            })
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
                header_control("model-picker-legacy", "Show or hide legacy models")
                    .w_full()
                    .h_auto()
                    .min_h(rpx(MODEL_ROW_H))
                    .track_focus(&self.legacy_focus)
                    .tab_index(0)
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
                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                        if action_key(event) {
                            this.legacy_open = !this.legacy_open;
                            cx.stop_propagation();
                            cx.notify();
                        }
                    })),
            );
        }
        let scale = f32::from(window.rem_size()) / crate::zoom::REM_BASE;
        let width = PICKER_W.min(f32::from(window.viewport_size().width) / scale - SPACE_3XL);
        div()
            .id("provider-model-picker")
            .debug_selector(|| "provider-model-picker".into())
            .w(rpx(width))
            .h(rpx(PICKER_H))
            .max_h(window.viewport_size().height - rpx(APPBAR_TOTAL_H).to_pixels(window.rem_size()))
            .rounded(rpx(RADIUS_PANEL))
            .border_1()
            .border_color(c::BORDER())
            .bg(c::BG_RAIL())
            .text_size(rpx(TEXT_SMALL))
            .flex()
            .overflow_hidden()
            .occlude()
            .capture_key_down(cx.listener(Self::key))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(PickerEvent::Dismiss)))
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
                            .h(rpx(CHROME_CONTROL_H))
                            .w_full()
                            .px(rpx(SPACE_LG))
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
                                Input::new(&self.search)
                                    .aria_label("Search models")
                                    .appearance(false)
                                    .bordered(false)
                                    .focus_bordered(false)
                                    .p_0()
                                    .w_full(),
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
            for (width, rem) in [(420., 16.), (320., 16.), (420., 24.)] {
                cx.update(|cx| gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem));
                for mode in ["claude", "favorites", "empty"] {
                    let mut retained = None;
                    let window = cx
                        .open_window(gpui::size(gpui::px(width), gpui::px(480.)), |window, cx| {
                            window.set_rem_size(gpui::px(rem));
                            let picker = cx.new(|cx| {
                                let mut p = ModelPicker::new(&state(), true, window, cx);
                                seed(&mut p);
                                if mode != "favorites" {
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
