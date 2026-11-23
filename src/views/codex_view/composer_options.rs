//! Composer model, effort, speed and option controls.

use super::{
    action_key, c, codex, composer_model_name, composer_placeholder, composer_scope,
    effective_model, effort_fraction, effort_key_index, effort_label, effort_stop,
    full_model_label, header_control_frame, model_picker, preference_choices, rpx,
    session_has_yolo_access, settings_control, speed_disabled_explanation,
    truncation_safe_label_width, Action, ClosingComposerPopup, CodexView, ComposerMenu, EffortDrag,
    EffortVisual, PopupMenu, PopupMenuItem, COMPOSER_MENU_W, MODEL_EFFORT_CARD_W,
};
use crate::views::tokens::{
    CHROME_CONTROL_H, CONTROL_H, ICON_BTN_W, ICON_MD, ICON_SM, ICON_XS, MOTION_BASE_MS,
    MOTION_FAST_MS, OPACITY_DISABLED, RADIUS_FULL, RADIUS_PANEL, SPACE_2XL, SPACE_3XL, SPACE_LG,
    SPACE_MD, SPACE_SM, SPACE_XL, SPACE_XS, TEXT_BODY, TEXT_MICRO, TEXT_SMALL, TEXT_TITLE,
};
use gpui::prelude::{
    FluentBuilder, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement,
    Styled,
};
use gpui::AppContext as _;
use gpui::{div, App, Context, Focusable, Window};

impl CodexView {
    fn retain_popup_exit(&mut self, cx: &mut Context<Self>) {
        let Some(menu) = self.menu else {
            return;
        };
        self.popup_generation = self.popup_generation.wrapping_add(1);
        self.closing_popup = None;
        if cx.reduce_motion() || !self.composer_menu_valid(cx) {
            return;
        }
        let index = match menu {
            ComposerMenu::Add => 3,
            ComposerMenu::Access => 2,
            _ => 0,
        };
        self.closing_popup = Some(ClosingComposerPopup {
            menu,
            popup: self.popup.clone(),
            model_picker: self.model_picker.clone(),
            bounds: self.option_bounds[index].get(),
            scope: composer_scope(&self.session.read(cx).state),
        });
        self.popup_motion.hide(cx);
        let generation = self.popup_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(super::super::motion::PopupMotion::exit_duration())
                .await;
            let _ = this.update(cx, |this, cx| {
                if generation == this.popup_generation {
                    this.closing_popup = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn close_menu(&mut self, cx: &mut Context<Self>) {
        self.retain_popup_exit(cx);
        self.menu = None;
        self.popup = None;
        self.model_picker = None;
        self.popup_subscription = None;
        self.composer_menu_scope = None;
        self.composer_menu_launch_config = None;
        self.model_from_card = false;
        self.effort_drag = None;
        self.effort_visual = None;
        self.effort_pending = None;
        cx.notify();
    }

    pub(crate) fn update_provider_placeholder(
        &mut self,
        provider: codex::Provider,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.input.update(cx, |input, cx| {
            input.set_placeholder(composer_placeholder(provider), window, cx);
        });
    }

    pub(super) fn open_menu(
        &mut self,
        menu: ComposerMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.clear_stop_confirmation(cx);
        let state = self.session.read(cx).state.clone();
        if menu == ComposerMenu::Add
            && (self.plan_answer.is_some()
                || self.submitted.is_some()
                || self.pending_attachment_picker
                || self.edit_request_pending
                || self.edit_confirmation.is_some()
                || state.saved.pending_edit.is_some())
        {
            return;
        }
        if menu == ComposerMenu::Access && session_has_yolo_access(&state) {
            self.close_menu(cx);
            return;
        }
        self.popup_generation = self.popup_generation.wrapping_add(1);
        self.closing_popup = None;
        self.popup_motion.show(cx);
        self.composer_menu_scope = Some(composer_scope(&state));
        self.composer_menu_launch_config = Some(
            cx.try_global::<crate::settings::SettingsState>()
                .map_or((false, false), |s| {
                    (
                        s.store.yolo_enabled,
                        s.store.chrome_enabled.unwrap_or(false),
                    )
                }),
        );
        self.effort_drag = None;
        self.effort_visual = None;
        if menu == ComposerMenu::Model {
            self.popup = None;
            self.menu = Some(menu);
            let allow_switch = self.dashboard_tray.is_some() && self.session.read(cx).is_deferred();
            let picker =
                cx.new(|cx| model_picker::ModelPicker::new(&state, allow_switch, window, cx));
            self.popup_subscription = Some(cx.subscribe_in(
                &picker,
                window,
                |this, _, event, window, cx| {
                    if !this.composer_menu_valid(cx) {
                        this.close_menu(cx);
                        return;
                    }
                    match event {
                        model_picker::PickerEvent::CatalogUpdated(provider, models) => {
                            if this.session.read(cx).state.saved.provider == *provider {
                                let recovered_cli =
                                    this.session.read(cx).catalog_missing_executable.is_some();
                                this.session.update(cx, |session, cx| {
                                    session.update_models(models.clone(), cx);
                                });
                                if recovered_cli {
                                    this.retry_dashboard_catalog(cx);
                                }
                            }
                        }
                        model_picker::PickerEvent::Dismiss => {
                            if this.model_from_card {
                                this.model_from_card = false;
                                this.open_menu(ComposerMenu::Settings, window, cx);
                                this.card_model_focus.focus(window, cx);
                            } else {
                                this.close_menu(cx);
                                this.option_focus[0].focus(window, cx);
                            }
                        }
                        model_picker::PickerEvent::Select(provider, model) => {
                            let current = this.session.read(cx).state.saved.provider;
                            if this.dashboard_tray.is_some() && this.session.read(cx).is_deferred()
                            {
                                let settings = &cx.global::<crate::settings::SettingsState>().store;
                                let args = provider.agent().launch_args_for_backend(
                                    true,
                                    settings.yolo_enabled,
                                    settings.chrome_enabled.unwrap_or(false),
                                );
                                let result = this.session.update(cx, |session, cx| {
                                    session.select_draft_model(*provider, model.clone(), args, cx)
                                });
                                if let Err(error) = result {
                                    this.session.update(cx, |session, cx| {
                                        session.state.error = Some(error);
                                        cx.notify();
                                    });
                                    return;
                                }
                                this.update_provider_placeholder(*provider, window, cx);
                                this.model_from_card = false;
                                this.open_menu(ComposerMenu::Settings, window, cx);
                            } else if *provider == current {
                                this.choose_option(model, window, cx);
                            }
                        }
                    }
                    cx.notify();
                },
            ));
            self.model_picker = Some(picker.clone());
            picker.update(cx, model_picker::ModelPicker::discover);
            cx.notify();
            return;
        }
        self.model_picker = None;
        if matches!(menu, ComposerMenu::Settings | ComposerMenu::Effort) {
            self.popup = None;
            self.popup_subscription = None;
            self.menu = Some(ComposerMenu::Settings);
            self.option_focus[1].focus(window, cx);
            cx.notify();
            return;
        }
        let (choices, _) = Self::menu_choices(&state, menu);
        let selected = match menu {
            ComposerMenu::Add => None,
            ComposerMenu::Model => state.model.as_deref().or(state.resolved_model.as_deref()),
            ComposerMenu::Settings | ComposerMenu::Effort => {
                state.effort.as_deref().or(state.resolved_effort.as_deref())
            }
            ComposerMenu::Access => {
                if state.saved.provider == codex::Provider::Claude {
                    state.access.as_deref().or(state.resolved_access.as_deref())
                } else {
                    state.access.as_deref()
                }
            }
        }
        .map(str::to_owned);
        let index = match menu {
            ComposerMenu::Add => 3,
            ComposerMenu::Model => 0,
            ComposerMenu::Settings | ComposerMenu::Effort => 1,
            ComposerMenu::Access => 2,
        };
        let context = self.option_focus[index].clone();
        let owner = cx.entity().downgrade();
        let scale = f32::from(window.rem_size()) / crate::zoom::REM_BASE;
        let width =
            COMPOSER_MENU_W.min(f32::from(window.viewport_size().width) / scale - SPACE_3XL);
        let popup = PopupMenu::build(window, cx, move |mut popup, window, _| {
            popup = crate::views::components::dropdown_menu(popup, window);
            popup = popup
                .action_context(context)
                .scrollable(false)
                .max_w(rpx(width).to_pixels(window.rem_size()))
                .min_w(rpx(width).to_pixels(window.rem_size()));
            if menu == ComposerMenu::Access {
                popup = popup.check_side(gpui_component::Side::Right);
            }
            for (value, label) in choices {
                let owner = owner.clone();
                let checked = selected.as_deref() == Some(value.as_str());
                let item = if menu == ComposerMenu::Access {
                    let (title, description) = label.split_once(" · ").map_or_else(
                        || {
                            (
                                label.clone(),
                                "Use the provider's configured access rules".to_owned(),
                            )
                        },
                        |(title, description)| (title.to_owned(), description.to_owned()),
                    );
                    let glyph = match value.as_str() {
                        "read-only" | "plan" => "eye",
                        "workspace-write" | "acceptEdits" => "edit",
                        "full-access" | "bypassPermissions" => "lock-open",
                        "__configured" => "reset",
                        _ => "lock",
                    };
                    let row_id = format!("access-choice-{value}");
                    PopupMenuItem::element(move |_, _| {
                        div()
                            .id(row_id.clone())
                            .when(cfg!(test), |row| {
                                row.debug_selector({
                                    let id = row_id.clone();
                                    move || id.clone()
                                })
                            })
                            .flex_1()
                            .min_w_0()
                            .py(rpx(SPACE_MD))
                            .flex()
                            .flex_col()
                            .gap_0()
                            .aria_label(label.clone())
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(rpx(SPACE_LG))
                                    .child(crate::icons::icon(glyph, ICON_MD, c::FG_DIM()))
                                    .child(
                                        div()
                                            .text_size(rpx(TEXT_BODY))
                                            .line_height(rpx(SPACE_3XL))
                                            .text_color(c::FG())
                                            .child(title.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .ml(rpx(ICON_MD + SPACE_LG))
                                    .text_size(rpx(TEXT_SMALL))
                                    .line_height(rpx(SPACE_3XL))
                                    .text_color(c::FG_MUTE())
                                    .whitespace_normal()
                                    .child(description.clone()),
                            )
                    })
                    .checked(checked)
                } else {
                    PopupMenuItem::new(label).checked(checked)
                };
                let item = if menu == ComposerMenu::Add {
                    item.icon(gpui_component::Icon::default().path(format!(
                        "icons/{}.svg",
                        match value.as_str() {
                            "__attach" => "paperclip",
                            "__reference" => "file",
                            _ => "tool",
                        }
                    )))
                } else {
                    item
                };
                popup = popup.item(item.on_click(move |_, window, cx| {
                    let _ = owner.update(cx, |this, cx| this.choose_option(&value, window, cx));
                }));
            }
            popup
        });
        self.popup_subscription = Some(cx.subscribe_in(
            &popup,
            window,
            |this, _, _: &gpui::DismissEvent, _, cx| this.close_menu(cx),
        ));
        self.menu = Some(menu);
        popup.focus_handle(cx).focus(window, cx);
        self.popup = Some(popup);
        cx.notify();
    }

    pub(super) fn composer_menu_valid(&self, cx: &App) -> bool {
        self.composer_menu_scope
            .as_ref()
            .is_some_and(|scope| *scope == composer_scope(&self.session.read(cx).state))
            && self.composer_menu_launch_config.is_some_and(|config| {
                config
                    == cx.try_global::<crate::settings::SettingsState>().map_or(
                        (false, false),
                        |s| {
                            (
                                s.store.yolo_enabled,
                                s.store.chrome_enabled.unwrap_or(false),
                            )
                        },
                    )
            })
    }

    pub(super) fn effort_choices(state: &codex::Snapshot) -> Vec<String> {
        effective_model(state)
            .and_then(|model| model["supportedReasoningEfforts"].as_array())
            .into_iter()
            .flatten()
            .filter_map(|entry| entry["reasoningEffort"].as_str().map(str::to_owned))
            .collect()
    }

    fn effort_model(state: &codex::Snapshot) -> String {
        effective_model(state)
            .and_then(|model| model["model"].as_str())
            .or(state.model.as_deref())
            .or(state.resolved_model.as_deref())
            .unwrap_or_default()
            .to_owned()
    }

    pub(super) fn effort_index(
        &self,
        state: &codex::Snapshot,
        choices: &[String],
    ) -> Option<usize> {
        if let Some(drag) = &self.effort_drag {
            if drag.model == Self::effort_model(state) && drag.choices == choices {
                return Some(drag.index);
            }
        }
        let pending = self
            .effort_pending
            .as_ref()
            .filter(|(model, _)| *model == Self::effort_model(state));
        let value = pending
            .map(|(_, effort)| effort.as_str())
            .or(state.effort.as_deref())
            .or(state.resolved_effort.as_deref())
            .or_else(|| {
                effective_model(state).and_then(|model| model["defaultReasoningEffort"].as_str())
            });
        value
            .and_then(|value| choices.iter().position(|entry| entry == value))
            .or_else(|| (choices.len() == 1).then_some(0))
    }

    fn sync_effort_visual(&mut self, state: &codex::Snapshot, choices: &[String], cx: &App) {
        let scope = composer_scope(state);
        let model = Self::effort_model(state);
        // Retain a newer local choice until its matching snapshot arrives, then
        // allow subsequent same-model configuration changes to become visible.
        let effective_effort = state.effort.as_deref().or(state.resolved_effort.as_deref());
        if self
            .effort_pending
            .as_ref()
            .is_some_and(|(pending_model, effort)| {
                *pending_model != model
                    || !choices.contains(effort)
                    || effective_effort == Some(effort.as_str())
            })
        {
            self.effort_pending = None;
        }
        let valid = self.effort_visual.as_ref().is_some_and(|visual| {
            visual.scope == scope && visual.model == model && visual.choices == choices
        });
        if !valid {
            self.effort_drag = None;
            let fraction = self
                .effort_index(state, choices)
                .map_or(0.0, |index| effort_fraction(index, choices.len()));
            self.effort_visual = Some(EffortVisual {
                scope,
                model,
                choices: choices.to_vec(),
                from: fraction,
                target: fraction,
                started: None,
                duration: std::time::Duration::from_millis(MOTION_BASE_MS),
            });
        } else if self.effort_drag.is_none() {
            // Provider acknowledgments can change effort with the same catalog.
            // Never replace an active drag's continuous pointer target.
            if let Some(index) = self.effort_index(state, choices) {
                self.move_effort_visual(effort_fraction(index, choices.len()), MOTION_BASE_MS, cx);
            }
        }
        if cx.reduce_motion() {
            if let Some(visual) = &mut self.effort_visual {
                visual.from = visual.target;
                visual.started = None;
            }
        }
    }

    fn move_effort_visual(&mut self, fraction: f32, duration: u64, cx: &App) {
        let now = cx.background_executor().now();
        if let Some(visual) = &mut self.effort_visual {
            let target = fraction.clamp(0.0, 1.0);
            if cx.reduce_motion() {
                visual.from = target;
                visual.target = target;
                visual.started = None;
            } else if visual.target != target {
                // Retarget from the clock-sampled position, never from the old stop.
                visual.from = visual.position(now);
                visual.target = target;
                visual.started = Some(now);
                visual.duration = std::time::Duration::from_millis(duration);
            }
        }
    }

    fn send_effort(&mut self, value: &str, cx: &mut Context<Self>) {
        if !self.composer_menu_valid(cx) {
            self.close_menu(cx);
            return;
        }
        let state = &self.session.read(cx).state;
        if !Self::effort_choices(state)
            .iter()
            .any(|choice| choice == value)
        {
            return;
        }
        let choices = Self::effort_choices(state);
        let model = Self::effort_model(state);
        if let Some(index) = choices.iter().position(|choice| choice == value) {
            self.move_effort_visual(effort_fraction(index, choices.len()), MOTION_BASE_MS, cx);
        }
        self.effort_pending = Some((model, value.to_owned()));
        self.session.update(cx, |session, cx| {
            session.send(
                Action::Configure {
                    model: None,
                    effort: Some(value.to_owned()),
                    access: None,
                },
                cx,
            );
        });
        cx.notify();
    }

    fn preview_effort_at(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        moving: bool,
        cx: &mut Context<Self>,
    ) {
        let bounds = self.effort_bounds.get();
        if bounds.size.width <= gpui::px(0.) {
            return;
        }
        let state = &self.session.read(cx).state;
        let valid = self.effort_drag.as_ref().is_some_and(|drag| {
            drag.scope == composer_scope(state)
                && drag.model == Self::effort_model(state)
                && drag.choices == Self::effort_choices(state)
        });
        if !valid {
            self.effort_drag = None;
            self.effort_visual = None;
            cx.notify();
            return;
        }
        let fraction =
            (f32::from(position.x - bounds.left()) / f32::from(bounds.size.width)).clamp(0.0, 1.0);
        if let Some(drag) = &mut self.effort_drag {
            drag.index = effort_stop(fraction, drag.choices.len());
        }
        self.move_effort_visual(
            fraction,
            if moving {
                MOTION_FAST_MS
            } else {
                MOTION_BASE_MS
            },
            cx,
        );
        cx.notify();
    }

    fn finish_effort_drag(&mut self, position: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        if self.effort_drag.is_none() {
            return;
        }
        self.preview_effort_at(position, true, cx);
        if let Some(drag) = self.effort_drag.take() {
            if let Some(value) = drag.choices.get(drag.index) {
                self.send_effort(value, cx);
            }
            cx.notify();
        }
    }

    fn reset_model_effort(&mut self, cx: &mut Context<Self>) {
        if !self.composer_menu_valid(cx) {
            self.close_menu(cx);
            return;
        }
        self.effort_drag = None;
        self.effort_visual = None;
        self.effort_pending = None;
        self.session
            .update(cx, |session, cx| session.send(Action::ResetModelEffort, cx));
        cx.notify();
    }

    pub(super) fn send_speed(&mut self, speed: codex::Speed, cx: &mut Context<Self>) {
        if !self.composer_menu_valid(cx) {
            self.close_menu(cx);
            return;
        }
        if self
            .session
            .read(cx)
            .state
            .speed_unavailable(speed)
            .is_some()
        {
            return;
        }
        self.session
            .update(cx, |session, cx| session.send(Action::SetSpeed(speed), cx));
        cx.notify();
    }

    fn speed_controls(
        &self,
        state: &codex::Snapshot,
        passive: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let selected = state.saved.composer.speed;
        let mut choices = preference_choices().w_full().bg(c::BG());
        for (index, speed) in [
            codex::Speed::Standard,
            codex::Speed::Fast,
            codex::Speed::Ultrafast,
        ]
        .into_iter()
        .enumerate()
        {
            if state.saved.provider == codex::Provider::Claude && speed == codex::Speed::Ultrafast {
                continue;
            }
            let unavailable = state.speed_unavailable(speed);
            let id = format!("codex-speed-{index}");
            choices =
                choices.child(
                    settings_control(
                        id,
                        format!(
                            "{} speed{}",
                            speed.label(),
                            if unavailable.is_some() {
                                ": unavailable"
                            } else {
                                ""
                            }
                        ),
                        selected == Some(speed),
                        unavailable.is_none(),
                    )
                    .debug_selector(move || format!("codex-speed-{index}"))
                    .when(!passive, |control| {
                        control.track_focus(&self.speed_focus[index])
                    })
                    .aria_description(unavailable.unwrap_or(
                        "Requested speed for the next turn; provider availability applies",
                    ))
                    .border_0()
                    .flex_1()
                    .justify_center()
                    .bg(if selected == Some(speed) {
                        c::BG_HOVER()
                    } else {
                        c::BG()
                    })
                    .child(crate::icons::icon("lightning", ICON_SM, c::FG_DIM()))
                    .child(speed.label())
                    .when(unavailable.is_none(), |control| {
                        control
                            .on_click(cx.listener(move |this, _, _, cx| this.send_speed(speed, cx)))
                            .on_key_down(cx.listener(
                                move |this, event: &gpui::KeyDownEvent, window, cx| {
                                    if action_key(event) {
                                        this.send_speed(speed, cx);
                                        window.prevent_default();
                                        cx.stop_propagation();
                                    }
                                },
                            ))
                    }),
                );
        }
        let mut notes = Vec::new();
        if selected.is_none() {
            notes.push("Using this conversation’s inherited provider speed. Choose a speed to override it.".to_owned());
        }
        if let Some(speed) = selected.filter(|speed| *speed != codex::Speed::Standard) {
            if let Some(reason) = state.speed_unavailable(speed) {
                notes.push(format!(
                    "Using Standard for the next turn. {} preference retained. {reason}",
                    speed.label()
                ));
            }
        }
        if state.saved.provider == codex::Provider::Codex {
            notes.push(
                match selected {
                    Some(codex::Speed::Ultrafast) => {
                        "ChatGPT plans: Ultrafast uses 8× included usage or 6× purchased credits. API pricing varies."
                    }
                    _ => "ChatGPT plans: Fast uses 2.5× included usage or 2× purchased credits. API pricing varies.",
                }
                .into(),
            );
            if state.speed_unavailable(codex::Speed::Ultrafast).is_some() {
                notes.push("Ultrafast unavailable for this model or account.".into());
            }
        } else {
            notes.push("Fast uses paid usage credits on subscriptions; included allowance does not cover it. Account and organization eligibility apply.".into());
            if state.speed_unavailable(codex::Speed::Fast).is_some() {
                notes.push(
                    "Fast requires a supported Opus model. Choose a model to enable it.".into(),
                );
            }
            if selected == Some(codex::Speed::Fast) {
                if let Some(reported) = state
                    .reported_speed
                    .as_deref()
                    .filter(|s| *s == "off" || *s == "cooldown")
                {
                    notes.push(format!(
                        "Claude last reported Fast {reported}{}. Fast preference retained.",
                        state
                            .speed_disabled_reason
                            .as_ref()
                            .map(|r| format!(": {}", speed_disabled_explanation(r)))
                            .unwrap_or_default()
                    ));
                }
            }
        }
        div()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_LG))
            .pt(rpx(SPACE_2XL))
            .border_t_1()
            .border_color(c::BORDER_SOFT())
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child(div().text_color(c::FG()).child("Speed"))
                    .child(div().text_color(c::FG_MUTE()).child("Next turn")),
            )
            .child(choices)
            .children(notes.into_iter().map(|note| {
                div()
                    .text_size(rpx(TEXT_MICRO))
                    .text_color(c::FG_DIM())
                    .child(note)
            }))
            .into_any_element()
    }

    pub(super) fn model_effort_card(
        &mut self,
        state: &codex::Snapshot,
        passive: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let choices = Self::effort_choices(state);
        if !passive {
            self.sync_effort_visual(state, &choices, cx);
        }
        let selected = self.effort_index(state, &choices);
        let effort = selected.and_then(|index| choices.get(index)).map_or_else(
            || {
                if choices.is_empty() {
                    "Effort unavailable".into()
                } else {
                    "Default effort".into()
                }
            },
            |value| effort_label(value),
        );
        let scale = f32::from(window.rem_size()) / crate::zoom::REM_BASE;
        let width =
            MODEL_EFFORT_CARD_W.min(f32::from(window.viewport_size().width) / scale - SPACE_3XL);
        let model_name = composer_model_name(state);
        let now = cx.background_executor().now();
        let fraction = self
            .effort_visual
            .as_ref()
            .map_or(0.0, |visual| visual.position(now));
        if !passive
            && !cx.reduce_motion()
            && self
                .effort_visual
                .as_ref()
                .is_some_and(|visual| visual.active(now))
        {
            window.request_animation_frame();
        }
        let track_bounds = self.effort_bounds.clone();
        let pointer_move = cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
            if this.effort_drag.is_some() {
                if event.pressed_button == Some(gpui::MouseButton::Left) {
                    this.preview_effort_at(event.position, true, cx);
                } else {
                    this.effort_drag = None;
                    this.effort_visual = None;
                    cx.notify();
                }
            }
        });
        let pointer_up = cx.listener(|this, event: &gpui::MouseUpEvent, _, cx| {
            if event.button == gpui::MouseButton::Left {
                this.finish_effort_drag(event.position, cx);
            }
        });
        let track = div()
            .relative()
            .w_full()
            .h(rpx(CONTROL_H))
            .rounded(rpx(RADIUS_FULL))
            .bg(c::BG_HOVER())
            .child(
                div()
                    .absolute()
                    .left(rpx(ICON_MD))
                    .right(rpx(ICON_MD))
                    .h_full()
                    .child(
                        gpui::canvas(
                            move |bounds, _, _| track_bounds.set(bounds),
                            move |_, (), window, _| {
                                // Capture an owned drag even over other controls or outside the track.
                                window.on_mouse_event(
                                    move |event: &gpui::MouseMoveEvent, phase, window, cx| {
                                        if phase == gpui::DispatchPhase::Capture {
                                            pointer_move(event, window, cx);
                                        }
                                    },
                                );
                                window.on_mouse_event(
                                    move |event: &gpui::MouseUpEvent, phase, window, cx| {
                                        if phase == gpui::DispatchPhase::Capture {
                                            pointer_up(event, window, cx);
                                        }
                                    },
                                );
                            },
                        )
                        .absolute()
                        .size_full(),
                    )
                    .when(selected.is_some(), |path| {
                        path.child(
                            div()
                                .absolute()
                                .left_0()
                                .top_0()
                                .h_full()
                                .w(gpui::relative(fraction))
                                .child(
                                    div()
                                        .id("codex-effort-fill")
                                        .debug_selector(|| "codex-effort-fill".into())
                                        .absolute()
                                        .left(-rpx(ICON_MD))
                                        // Keep both caps full-sized beneath the thumb, even
                                        // at the first stop where a short fill would flatten.
                                        .right(-rpx(ICON_MD))
                                        .h_full()
                                        .rounded(rpx(RADIUS_FULL))
                                        .bg(c::BLUE()),
                                ),
                        )
                    })
                    .children(choices.iter().enumerate().map(|(index, _)| {
                        div()
                            .absolute()
                            .left(gpui::relative(effort_fraction(index, choices.len())))
                            .top(rpx(SPACE_XL))
                            .ml(-rpx(SPACE_XS))
                            .size(rpx(SPACE_SM))
                            .rounded(rpx(RADIUS_FULL))
                            .bg(c::FG_MUTE())
                    }))
                    .when_some(selected, |path, _| {
                        path.child(
                            div()
                                .id("codex-effort-thumb")
                                .debug_selector(|| "codex-effort-thumb".into())
                                .absolute()
                                .left(gpui::relative(fraction))
                                .top(-rpx(SPACE_XS))
                                .ml(-rpx(ICON_MD))
                                .size(rpx(ICON_BTN_W))
                                .rounded(rpx(RADIUS_FULL))
                                .bg(c::FG()),
                        )
                    }),
            );
        let slider = div()
            .id("codex-effort-slider")
            .debug_selector(|| "codex-effort-slider".into())
            .role(gpui::Role::Slider)
            .aria_label("Reasoning effort")
            .aria_value(effort.clone())
            .aria_description(if choices.len() < 2 {
                "This model has no adjustable effort range"
            } else {
                "Applies to the next turn. Use arrow keys, Home or End"
            })
            .aria_min_numeric_value(0.0)
            .aria_max_numeric_value(choices.len().saturating_sub(1) as f64)
            .aria_numeric_value(selected.unwrap_or(0) as f64)
            .aria_numeric_value_step(1.0)
            .track_focus(&self.option_focus[1])
            .tab_index(0)
            .tab_stop(!passive)
            .h(rpx(ICON_BTN_W))
            .w_full()
            .flex()
            .items_center()
            .rounded(rpx(RADIUS_FULL))
            .focus_visible(|style| style.bg(c::BG_HOVER()))
            .when(choices.len() < 2, |slider| slider.opacity(OPACITY_DISABLED))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                    if !this.composer_menu_valid(cx) {
                        return;
                    }
                    let state = &this.session.read(cx).state;
                    let choices = Self::effort_choices(state);
                    if choices.len() > 1 {
                        this.effort_drag = Some(EffortDrag {
                            scope: composer_scope(state),
                            model: Self::effort_model(state),
                            choices,
                            index: 0,
                        });
                        this.preview_effort_at(event.position, false, cx);
                        this.option_focus[1].focus(window, cx);
                    }
                    window.prevent_default();
                    cx.stop_propagation();
                }),
            )
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                let state = &this.session.read(cx).state;
                let choices = Self::effort_choices(state);
                let index = this.effort_index(state, &choices).unwrap_or(0);
                if let Some(index) = effort_key_index(&event.keystroke.key, index, choices.len()) {
                    this.effort_drag = None;
                    this.send_effort(&choices[index], cx);
                    cx.stop_propagation();
                }
            }))
            .child(track);
        div()
            .id("codex-model-effort-card")
            .debug_selector(|| "codex-model-effort-card".into())
            .role(gpui::Role::Group)
            .when(!passive, |control| control.track_focus(&self.card_focus))
            .aria_label("Model, reasoning effort and speed")
            .w(rpx(width))
            .p(rpx(SPACE_2XL))
            .rounded(rpx(RADIUS_PANEL))
            .border_1()
            .border_color(c::BORDER())
            .bg(c::BG_RAIL())
            .text_size(rpx(TEXT_SMALL))
            .text_color(c::FG_DIM())
            .flex()
            .flex_col()
            .gap(rpx(SPACE_2XL))
            .when(!passive, |card| {
                card.occlude()
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down_out(cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                        if !this.option_bounds[0].get().contains(&event.position) {
                            this.close_menu(cx);
                        }
                    }))
            })
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(rpx(SPACE_SM))
                    .child(
                        div()
                            .w(rpx(CONTROL_H))
                            .h(rpx(CONTROL_H))
                            .flex()
                            .items_center()
                            .child(crate::icons::icon("lightning", ICON_MD, c::FG_MUTE())),
                    )
                    .child(
                        header_control_frame(
                            "codex-card-model",
                            format!("Choose model: {}", composer_model_name(state)),
                            c::BG_HOVER(),
                        )
                        .when(!passive, |control| {
                            control.track_focus(&self.card_model_focus)
                        })
                        .tab_index(0)
                        .tab_stop(!passive)
                        .flex_1()
                        .min_w_0()
                        .h_auto()
                        .flex_col()
                        .focus_visible(|style| style.bg(c::BG_HOVER()))
                        .child(
                            div()
                                .id("codex-card-effort-label")
                                .debug_selector(|| "codex-card-effort-label".into())
                                .text_size(rpx(TEXT_TITLE))
                                .text_color(c::FG_DIM())
                                .group_hover("neutral-control", |text| text.text_color(c::FG()))
                                .child(effort),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .h(rpx(CHROME_CONTROL_H))
                                .flex()
                                .items_center()
                                .justify_center()
                                .gap(rpx(SPACE_XS))
                                .child(
                                    full_model_label(gpui::StyledText::new(model_name))
                                        .id("codex-card-model-label")
                                        .debug_selector(|| "codex-card-model-label".into()),
                                )
                                .child(crate::icons::icon("chev-right", ICON_XS, c::FG_MUTE())),
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.model_from_card = true;
                            this.effort_pending = None;
                            this.open_menu(ComposerMenu::Model, window, cx);
                        }))
                        .on_key_down(cx.listener(
                            |this, event: &gpui::KeyDownEvent, window, cx| {
                                if action_key(event) {
                                    this.model_from_card = true;
                                    this.effort_pending = None;
                                    this.open_menu(ComposerMenu::Model, window, cx);
                                    cx.stop_propagation();
                                }
                            },
                        )),
                    )
                    .child(
                        super::super::components::header_control(
                            "codex-model-effort-reset",
                            "Reset model and effort to configured values",
                        )
                        .debug_selector(|| "codex-model-effort-reset".into())
                        .when(!passive, |control| {
                            control.track_focus(&self.card_reset_focus)
                        })
                        .tab_index(0)
                        .tab_stop(!passive)
                        .focus_visible(|style| style.bg(c::BG_HOVER()))
                        .child(crate::icons::icon("reset", ICON_MD, c::FG_DIM()))
                        .on_click(cx.listener(|this, _, _, cx| this.reset_model_effort(cx)))
                        .on_key_down(cx.listener(
                            |this, event: &gpui::KeyDownEvent, _, cx| {
                                if action_key(event) {
                                    this.reset_model_effort(cx);
                                    cx.stop_propagation();
                                }
                            },
                        )),
                    ),
            )
            .child(slider)
            .child(self.speed_controls(state, passive, cx))
            .into_any_element()
    }

    pub(super) fn option_triggers(
        &self,
        state: &codex::Snapshot,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let model = composer_model_name(state);
        let effort = state
            .effort
            .as_deref()
            .or(state.resolved_effort.as_deref())
            .map_or_else(|| "Default effort".into(), effort_label);
        let access_value = state.access.as_deref().or(state.resolved_access.as_deref());
        let access_icon = match access_value {
            Some("full-access" | "bypassPermissions") => "lock-open",
            Some("read-only" | "plan") => "eye",
            Some("workspace-write" | "acceptEdits") => "edit",
            _ => "lock",
        };
        let access = match access_value {
            Some("full-access") => "Full access",
            Some("read-only") => "Read only",
            Some("workspace-write") => "Workspace access",
            Some("default" | "manual") => "Ask for permission",
            Some("auto") => "Automatic permissions",
            Some("acceptEdits") => "Accept edits",
            Some("plan") => "Plan mode",
            Some("dontAsk") => "Deny unapproved tools",
            Some("bypassPermissions") => "Bypass permissions",
            _ => "Configured access",
        };
        let access = if state.saved.provider == codex::Provider::Codex && state.access.is_none() {
            state.configured_access_summary()
        } else {
            access.to_owned()
        };
        let mut controls = Vec::new();
        for (index, menu, id, name, icon, label) in [
            (
                0,
                ComposerMenu::Settings,
                "codex-model",
                "Choose model, reasoning effort and speed",
                state.saved.provider.agent().icon_name(),
                format!(
                    "{model} · {effort} · {}",
                    state
                        .saved
                        .composer
                        .speed
                        .map_or("Inherited speed", codex::Speed::label)
                ),
            ),
            (
                2,
                ComposerMenu::Access,
                "codex-access",
                "Choose access level",
                access_icon,
                access.clone(),
            ),
        ]
        .into_iter()
        .rev()
        {
            if menu == ComposerMenu::Access && session_has_yolo_access(state) {
                controls.push(
                    div()
                        .id("codex-yolo-access")
                        .debug_selector(|| "codex-yolo-access".into())
                        .flex()
                        .items_center()
                        .flex_shrink_0()
                        .min_h(rpx(CHROME_CONTROL_H))
                        .px(rpx(SPACE_SM))
                        .gap(rpx(SPACE_SM))
                        .text_color(c::FG_DIM())
                        .text_size(rpx(TEXT_SMALL))
                        .child(crate::icons::icon("lock-open", ICON_SM, c::FG_DIM()))
                        .child("Yolo mode · change in Settings")
                        .into_any_element(),
                );
                continue;
            }
            let bounds = self.option_bounds[index].clone();
            let summary_id = if menu == ComposerMenu::Settings {
                "codex-model-effort-summary"
            } else {
                "codex-access-summary"
            };
            // GPUI's intrinsic flex text measurement can undercount glyph advances,
            // causing an ellipsis even when the toolbar has space for this label.
            let mut label_run = window.text_style().to_run(label.len());
            label_run.font.family = crate::fonts::UI_FAMILY.into();
            let label_width = truncation_safe_label_width(
                &label,
                rpx(TEXT_SMALL).to_pixels(window.rem_size()),
                &label_run,
                window.text_system(),
            );
            let control = super::super::components::inline_disclosure(id, name);
            controls.push(
                control
                    .relative()
                    .track_focus(&self.option_focus[index])
                    .child(
                        gpui::canvas(move |rect, _, _| bounds.set(rect), |_, (), _, _| {})
                            .absolute()
                            .size_full(),
                    )
                    .w_auto()
                    .min_w_0()
                    .px(rpx(SPACE_SM))
                    .gap(rpx(SPACE_SM))
                    .tab_index(0)
                    .text_color(c::FG_DIM())
                    .text_size(rpx(TEXT_SMALL))
                    .when(
                        self.menu == Some(menu)
                            || (menu == ComposerMenu::Settings && self.model_from_card),
                        |button| button.text_color(c::FG()),
                    )
                    .focus_visible(|style| style.border_1().border_color(c::SEL_RING()))
                    .child(
                        div()
                            .flex_shrink_0()
                            .size(rpx(ICON_SM))
                            .child(crate::icons::icon(icon, ICON_SM, c::FG_DIM())),
                    )
                    .when(menu == ComposerMenu::Settings, |button| {
                        button
                            .max_w_full()
                            .min_h(rpx(CHROME_CONTROL_H))
                            .when(self.dashboard_tray.is_some(), gpui::Styled::h_auto)
                    })
                    .when(menu == ComposerMenu::Access, |button| {
                        button.max_w_full().h_auto().min_h(rpx(CHROME_CONTROL_H))
                    })
                    .child(if menu == ComposerMenu::Settings {
                        full_model_label(gpui::StyledText::new(label))
                            .when(self.dashboard_tray.is_some(), |label| {
                                label.min_w_0().flex_shrink_1().whitespace_normal()
                            })
                            .id(summary_id)
                            .debug_selector(move || summary_id.into())
                    } else {
                        div()
                            .id(summary_id)
                            .debug_selector(move || summary_id.into())
                            .w(label_width)
                            .min_w_0()
                            .flex_shrink_1()
                            .whitespace_normal()
                            .child(label)
                    })
                    .child(
                        div()
                            .id(if menu == ComposerMenu::Access {
                                "codex-access-indicator"
                            } else {
                                "codex-model-indicator"
                            })
                            .flex_shrink_0()
                            .size(rpx(ICON_SM))
                            .when(menu == ComposerMenu::Access, |indicator| {
                                indicator.debug_selector(|| "codex-access-indicator".into())
                            })
                            .child(crate::icons::icon("chev-down", ICON_SM, c::FG_DIM())),
                    )
                    .on_key_down(cx.listener(
                        move |this, event: &gpui::KeyDownEvent, window, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space" | "down")
                                && !event.is_held
                            {
                                this.open_menu(menu, window, cx);
                                cx.stop_propagation();
                            } else if event.keystroke.key == "escape" {
                                this.close_menu(cx);
                                cx.stop_propagation();
                            }
                        },
                    ))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if this.menu == Some(menu) {
                            this.close_menu(cx);
                        } else {
                            this.open_menu(menu, window, cx);
                        }
                    }))
                    .into_any_element(),
            );
        }
        controls
    }

    pub(super) fn menu_choices(
        state: &codex::Snapshot,
        menu: ComposerMenu,
    ) -> (Vec<(String, String)>, &'static str) {
        let mut choices = Vec::new();
        let title = match menu {
            ComposerMenu::Add => {
                return (
                    vec![
                        ("__attach".into(), "Files and images".into()),
                        ("__reference".into(), "Reference a file or folder".into()),
                        ("__skills".into(), "Skills".into()),
                        ("__commands".into(), "Commands".into()),
                    ],
                    "Add context",
                )
            }
            ComposerMenu::Model => {
                choices = state
                    .models
                    .iter()
                    .filter_map(|model| {
                        Some((
                            model["model"].as_str()?.to_owned(),
                            model["displayName"]
                                .as_str()
                                .unwrap_or(model["model"].as_str()?)
                                .to_owned(),
                        ))
                    })
                    .collect();
                "Model · applies to the next turn"
            }
            ComposerMenu::Settings | ComposerMenu::Effort => {
                if let Some(model) = effective_model(state) {
                    choices = model["supportedReasoningEfforts"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|entry| {
                            let effort = entry["reasoningEffort"].as_str()?;
                            Some((effort.to_owned(), effort.to_owned()))
                        })
                        .collect();
                }
                "Reasoning effort · applies to the next turn"
            }
            ComposerMenu::Access => {
                choices = if state.saved.provider == codex::Provider::Claude {
                    vec![
                        ("default", "Ask for permission · provider rules apply"),
                        ("manual", "Ask for permission · provider rules apply"),
                        (
                            "auto",
                            "Automatic permissions · provider evaluates tool risk",
                        ),
                        ("acceptEdits", "Accept edits · ask before other tools"),
                        ("plan", "Plan mode · explore before implementing"),
                        ("dontAsk", "Deny tools that require permission"),
                        (
                            "bypassPermissions",
                            "Bypass approval checks · provider restrictions still apply",
                        ),
                    ]
                    .into_iter()
                    .filter(|(mode, _)| {
                        grove_core::claude::can_select_permission_mode(&state.saved, mode)
                            && state
                                .permission_modes
                                .iter()
                                .any(|supported| supported == mode)
                    })
                    .collect()
                } else {
                    vec![
                        ("read-only", "Read only · inspect without edits"),
                        (
                            "workspace-write",
                            "Workspace access · edit in this worktree",
                        ),
                        (
                            "full-access",
                            "Full access · no sandbox or approval prompts",
                        ),
                    ]
                }
                .into_iter()
                .map(|(value, label)| (value.into(), label.into()))
                .collect();
                "Access · explicit selection overrides provider defaults"
            }
        };

        choices.push(("__configured".into(), "Restore configured defaults".into()));
        (choices, title)
    }

    pub(super) fn choose_option(
        &mut self,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.menu.is_some() && !self.composer_menu_valid(cx) {
            self.close_menu(cx);
            return;
        }
        if self.menu == Some(ComposerMenu::Add) {
            let blocked = self.plan_answer.is_some()
                || self.submitted.is_some()
                || self.pending_attachment_picker
                || self.edit_request_pending
                || self.edit_confirmation.is_some()
                || self.session.read(cx).state.saved.pending_edit.is_some();
            self.close_menu(cx);
            if blocked {
                return;
            }
            if value == "__attach" {
                self.attach_files(window, cx);
            } else if matches!(value, "__reference" | "__skills" | "__commands") {
                let trigger = match value {
                    "__reference" => "@",
                    "__skills" => "$",
                    _ => "/",
                };
                let input = self.input.read(cx);
                let prefix = if input.cursor() > 0
                    && !input.value()[..input.cursor()].ends_with(char::is_whitespace)
                {
                    " "
                } else {
                    ""
                };
                let text = format!("{prefix}{trigger}");
                self.input
                    .update(cx, |input, cx| input.insert(text, window, cx));
                let draft = self.input.read(cx).value().to_string();
                self.session
                    .update(cx, |session, cx| session.send(Action::Draft(draft), cx));
                self.dismissed_suggestions = None;
                self.set_composer_collapsed(false, cx);
                self.refresh_files(cx);
                self.input.focus_handle(cx).focus(window, cx);
            }
            cx.notify();
            return;
        }
        let return_to_card = self.model_from_card;
        self.popup_subscription = None;
        if value == "__configured" {
            self.close_menu(cx);
            self.session.update(cx, |session, cx| {
                session.send(
                    if return_to_card {
                        Action::ResetModelEffort
                    } else {
                        Action::ResetConfiguration
                    },
                    cx,
                );
            });
        } else if let Some(menu) = self.menu {
            self.retain_popup_exit(cx);
            self.menu = None;
            self.popup = None;
            let action = Action::Configure {
                model: (menu == ComposerMenu::Model).then(|| value.to_owned()),
                effort: (menu == ComposerMenu::Effort).then(|| value.to_owned()),
                access: (menu == ComposerMenu::Access).then(|| value.to_owned()),
            };
            self.session
                .update(cx, |session, cx| session.send(action, cx));
        } else {
            self.accept_suggestion(value, true, window, cx);
        }
        if return_to_card {
            self.model_from_card = false;
            self.open_menu(ComposerMenu::Settings, window, cx);
        } else if self.popup.is_none() {
            self.input.focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }
}
