//! Section-scoped entry points for retained Describe and Review readers.
use super::*;
use crate::views::{
    components,
    status::{self, StatusKind},
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use grove_core::change_description::Scope;

#[derive(Clone)]
pub(super) struct ChangeToolState {
    pub(super) scope: Scope,
    pub(super) label: &'static str,
    pub(super) status: Option<(StatusKind, &'static str)>,
}
#[derive(Default)]
pub(super) struct ChangeTools {
    pub(super) menu: Option<Entity<PopupMenu>>,
    subscription: Option<gpui::Subscription>,
    owner: Option<(String, String, bool)>,
    position: gpui::Point<gpui::Pixels>,
    pub(super) focus: HashMap<bool, FocusHandle>,
    bounds: HashMap<bool, std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>>,
}
impl ChangeTools {
    pub(super) fn dismiss(&mut self) {
        self.menu = None;
        self.subscription = None;
        self.owner = None;
    }
}
impl Sidebar {
    pub(super) fn change_tools_states(
        &mut self,
        path: &str,
        local: bool,
        cx: &mut Context<Self>,
    ) -> [Option<ChangeToolState>; 2] {
        let live = if local {
            self.checkout_has_local_changes(path)
                .then_some(Scope::Local)
        } else {
            self.checkout_description_pr()
                .map(|pr| Scope::PullRequest { url: pr.url })
        };
        [
            self.description_tool_state(path, local, live.clone(), cx),
            self.review_tool_state(path, local, live, cx),
        ]
    }
    pub(super) fn change_tools_entries(
        &mut self,
        path: &str,
        local: bool,
        cx: &mut Context<Self>,
    ) -> Vec<(usize, ChangeToolState)> {
        let mut entries: Vec<_> = self
            .change_tools_states(path, local, cx)
            .into_iter()
            .enumerate()
            .filter_map(|(index, state)| state.map(|state| (index, state)))
            .collect();
        // Explicit-URL and detached retained sessions can differ from the attached PR.
        // Keep their saved navigation alongside the current source's generation actions.
        for (index, retained) in [
            self.description_tool_state(path, local, None, cx),
            self.review_tool_state(path, local, None, cx),
        ]
        .into_iter()
        .enumerate()
        {
            if let Some(retained) = retained {
                if !entries
                    .iter()
                    .any(|(tool, state)| *tool == index && state.scope == retained.scope)
                {
                    entries.push((index, retained));
                }
            }
        }
        entries
    }
    pub(super) fn has_change_tools(
        &mut self,
        path: &str,
        local: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        self.change_tools_states(path, local, cx)
            .iter()
            .any(Option::is_some)
    }
    pub(super) fn render_change_tools_heading(
        &mut self,
        path: &str,
        local: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.restore_session_description(cx);
        self.restore_session_review(cx);
        let states = self.change_tools_entries(path, local, cx);
        let owner = (format!("{:?}", self.selection), path.to_owned(), local);
        let tools = &mut self.checkout_panel.change_tools;
        let focus = tools
            .focus
            .entry(local)
            .or_insert_with(|| cx.focus_handle().tab_stop(true).tab_index(0))
            .clone();
        let bounds = tools.bounds.entry(local).or_default().clone();
        let keyboard_bounds = bounds.clone();
        let target = owner.clone();
        let keyboard_target = owner.clone();
        let id = if local {
            "checkout-local-tools"
        } else {
            "checkout-pr-tools"
        };
        let title = if local {
            "Local changes"
        } else {
            "Pull request"
        };
        let mut heading = div()
            .px(rpx(SPACE_2XL))
            .h(rpx(CONTROL_H))
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .debug_selector(move || {
                        if local {
                            "checkout-changes-heading".into()
                        } else {
                            "checkout-pr-heading".into()
                        }
                    })
                    .text_size(rpx(TEXT_SMALL))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(c::FG_DIM())
                    .child(title),
            );
        if !states.is_empty() {
            heading = heading.child(
                components::header_control(
                    id,
                    if local {
                        "Local changes actions"
                    } else {
                        "Pull request actions"
                    },
                )
                .debug_selector(move || id.into())
                .track_focus(&focus)
                .tab_index(0)
                .aria_expanded(tools.owner.as_ref() == Some(&owner) && tools.menu.is_some())
                .focus_visible(|control| control.bg(c::BG_HOVER()))
                .when(
                    tools.owner.as_ref() == Some(&owner) && tools.menu.is_some(),
                    |control| control.bg(c::BG_HOVER()),
                )
                .on_prepaint(move |rect, _, _| bounds.set(rect))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_change_tools_menu(
                        target.clone(),
                        keyboard_bounds.get().bottom_right(),
                        window,
                        cx,
                    );
                }))
                .on_key_down(
                    cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space" | "down")
                            && !event.is_held
                        {
                            let position = this.checkout_panel.change_tools.bounds[&local]
                                .get()
                                .bottom_right();
                            this.open_change_tools_menu(
                                keyboard_target.clone(),
                                position,
                                window,
                                cx,
                            );
                            window.prevent_default();
                            cx.stop_propagation();
                        }
                    }),
                )
                .child(icon("more", ICON_SM, c::FG_DIM())),
            );
        }
        let mut column = div().flex().flex_col().child(heading);
        let mut detached_sources = HashSet::new();
        for (position, (index, state)) in states.into_iter().enumerate() {
            {
                if let Some((kind, text)) = state.status {
                    column = column.child(div().px(rpx(SPACE_2XL)).py(rpx(SPACE_XS)).child(
                        status::label(
                            format!(
                                "{}-{}-state{}",
                                if local {
                                    "checkout-local"
                                } else {
                                    "checkout-pr"
                                },
                                if index == 0 { "describe" } else { "review" },
                                if position < 2 {
                                    String::new()
                                } else {
                                    format!("-{position}")
                                }
                            ),
                            kind,
                            text,
                            ICON_SM,
                            cx,
                        ),
                    ));
                }
                if !local
                    && self
                        .checkout_description_pr()
                        .is_none_or(|pr| state.scope != (Scope::PullRequest { url: pr.url }))
                {
                    if let Scope::PullRequest { url } = state.scope {
                        // A detached result must not look like work against a current PR.
                        if detached_sources.insert(url.clone()) {
                            column = column.child(
                                div()
                                    .px(rpx(SPACE_2XL))
                                    .text_size(rpx(TEXT_MICRO))
                                    .text_color(c::FG_DIM())
                                    .child(format!(
                                        "Saved · PR #{}",
                                        url.rsplit('/').next().unwrap_or("?")
                                    )),
                            );
                        }
                    }
                }
            }
        }
        let tools = &self.checkout_panel.change_tools;
        if tools.owner.as_ref() == Some(&owner) {
            if let Some(menu) = tools.menu.clone() {
                column = column.child(gpui::deferred(
                    gpui::anchored()
                        .position_mode(gpui::AnchoredPositionMode::Window)
                        .anchor(gpui::Anchor::TopRight)
                        .position(tools.position)
                        .snap_to_window_with_margin(rpx(SPACE_LG).to_pixels(window.rem_size()))
                        .child(menu),
                ));
            }
        }
        column.into_any_element()
    }
    fn open_change_tools_menu(
        &mut self,
        target: (String, String, bool),
        position: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let states = self.change_tools_entries(&target.1, target.2, cx);
        let attached = self
            .checkout_description_pr()
            .map(|pr| Scope::PullRequest { url: pr.url });
        let source = self.checkout_panel.change_tools.focus[&target.2].clone();
        let owner = cx.entity().downgrade();
        let context = source.clone();
        let menu_target = target.clone();
        let menu = PopupMenu::build(window, cx, move |menu, window, _| {
            let mut menu = components::dropdown_menu(menu, window).action_context(context);
            for (index, state) in states {
                let label = match &state.scope {
                    Scope::PullRequest { url } if attached.as_ref() != Some(&state.scope) => {
                        format!(
                            "{} · PR #{}",
                            state.label,
                            url.rsplit('/').next().unwrap_or("?")
                        )
                    }
                    _ => state.label.to_owned(),
                };
                let owner = owner.clone();
                let target = menu_target.clone();
                let source = source.clone();
                menu = menu.item(
                    PopupMenuItem::new(label)
                        .icon(gpui_component::Icon::default().path("icons/diff.svg"))
                        .on_click(move |_, _, cx| {
                            let _ = owner.update(cx, |this, cx| {
                                if format!("{:?}", this.selection) != target.0 {
                                    return;
                                }
                                let targets = this.checkout_targets(cx);
                                if super::checkout_panel::selected_checkout(
                                    &targets,
                                    this.checkout_panel.chosen_root.as_deref(),
                                )
                                .is_none_or(|(_, path)| path != target.1)
                                {
                                    return;
                                }
                                let current = this.change_tools_entries(&target.1, target.2, cx);
                                if !current.iter().any(|(tool, current)| {
                                    *tool == index && current.scope == state.scope
                                }) {
                                    return;
                                }
                                if index == 0 {
                                    this.open_description_tool(
                                        target.1.clone(),
                                        state.scope.clone(),
                                        source.clone(),
                                        cx,
                                    );
                                } else {
                                    this.open_review_tool(
                                        target.1.clone(),
                                        state.scope.clone(),
                                        source.clone(),
                                        cx,
                                    );
                                }
                            });
                        }),
                );
            }
            menu
        });
        let tools = &mut self.checkout_panel.change_tools;
        tools.subscription = Some(cx.subscribe_in(
            &menu,
            window,
            |this, _, _: &gpui::DismissEvent, window, cx| {
                let tools = &mut this.checkout_panel.change_tools;
                if let Some((owner, _, local)) = tools.owner.take() {
                    if owner == format!("{:?}", this.selection) {
                        if let Some(focus) = tools.focus.get(&local) {
                            focus.focus(window, cx);
                        }
                    }
                }
                tools.menu = None;
                tools.subscription = None;
                cx.notify();
            },
        ));
        tools.owner = Some(target);
        tools.position = position;
        menu.focus_handle(cx).focus(window, cx);
        tools.menu = Some(menu);
        cx.notify();
    }
}
