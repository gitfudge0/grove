//! A conversation-owned plan row and anchored, editable quick review.
use super::*;
use crate::views::native_plan::NativePlan;

const PEEK_WIDTH: f32 = 520.;
const PEEK_HEIGHT: f32 = 620.;
const HOVER_INTENT: Duration = Duration::from_millis(220);
const HOVER_GRACE: Duration = Duration::from_millis(260);

pub(super) struct PlanPeek {
    session: SessionId,
    plan: Entity<NativePlan>,
    explicit: bool,
    closing: bool,
    focus: FocusHandle,
}

impl Sidebar {
    pub(super) fn selected_native_plans(&self, cx: &App) -> Vec<(SessionId, Entity<NativePlan>)> {
        let Some(Selection::Session(id)) = self.selection else {
            return Vec::new();
        };
        self.codex_views
            .get(&id)
            .map(|view| {
                view.read(cx)
                    .native_plans_for_sidebar(cx)
                    .into_iter()
                    .map(|plan| (id, plan))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(super) fn dismiss_plan_peek(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.checkout_panel.plan_peek_task = None;
        self.checkout_panel.plan_hover_target = None;
        let Some(peek) = &mut self.checkout_panel.plan_peek else {
            return;
        };
        if peek.closing {
            return;
        }
        peek.closing = true;
        if peek.focus.contains_focused(window, cx) {
            if let Some(source) = self.checkout_panel.plan_focus.get(&peek.plan.entity_id()) {
                source.focus(window, cx);
            }
        }
        self.checkout_panel.plan_presence.set(false, cx);
        if cx.reduce_motion() {
            if let Some(peek) = self.checkout_panel.plan_peek.take() {
                peek.plan.update(cx, NativePlan::dismiss_peek);
            }
        } else {
            let duration = self.checkout_panel.plan_presence.transition_duration();
            self.checkout_panel.plan_close_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(duration).await;
                let _ = this.update(cx, |this, cx| {
                    if let Some(peek) = this.checkout_panel.plan_peek.take() {
                        peek.plan.update(cx, NativePlan::dismiss_peek);
                    }
                    cx.notify();
                });
            }));
        }
        cx.notify();
    }

    fn open_plan_peek(
        &mut self,
        target: Entity<NativePlan>,
        explicit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_branch_peek(window, cx);
        self.dismiss_script_peek(window, cx);
        self.checkout_panel.plan_peek_task = None;
        if self.confirmation_open() || self.menu.is_some() || !self.checkout_panel.agents_visible {
            return;
        }
        let Some((session, plan)) = self
            .selected_native_plans(cx)
            .into_iter()
            .find(|(_, p)| *p == target)
        else {
            return;
        };
        if plan.read(cx).is_expanded(cx) {
            return;
        }
        self.checkout_panel.plan_close_task = None;
        self.checkout_panel.plan_presence.set(true, cx);
        if let Some(peek) = &mut self.checkout_panel.plan_peek {
            if peek.plan == plan {
                peek.closing = false;
                peek.explicit |= explicit;
                if explicit {
                    peek.focus.focus(window, cx);
                }
                return;
            }
        }
        self.dismiss_agent_peek(window, cx);
        let focus = cx.focus_handle().tab_stop(true);
        if explicit {
            focus.focus(window, cx);
        }
        let source = self
            .checkout_panel
            .plan_focus
            .get(&plan.entity_id())
            .cloned();
        plan.update(cx, |plan, cx| plan.show_peek(source, cx));
        self.checkout_panel.plan_presence = motion::Presence::fast();
        self.checkout_panel.plan_presence.set(true, cx);
        self.checkout_panel.plan_peek = Some(PlanPeek {
            session,
            plan,
            explicit,
            closing: false,
            focus,
        });
        cx.notify();
    }

    fn plan_hover(
        &mut self,
        target: Entity<NativePlan>,
        hovered: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !hovered
            && self
                .checkout_panel
                .plan_hover_target
                .is_some_and(|id| id != target.entity_id())
        {
            return;
        }
        self.checkout_panel.plan_hover_target = hovered.then_some(target.entity_id());
        self.checkout_panel.plan_peek_task = None;
        if self
            .checkout_panel
            .plan_peek
            .as_ref()
            .is_some_and(|peek| peek.explicit || peek.focus.contains_focused(window, cx))
        {
            return;
        }
        self.checkout_panel.plan_peek_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(if hovered { HOVER_INTENT } else { HOVER_GRACE })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                if hovered {
                    this.open_plan_peek(target, false, window, cx);
                } else {
                    this.dismiss_plan_peek(window, cx);
                }
            });
        }));
    }

    pub(super) fn render_checkout_plan(
        &mut self,
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let plans = self.selected_native_plans(cx);
        if plans.is_empty() {
            return None;
        }
        let mut rows = Vec::new();
        for (_, plan) in plans {
            let (title, status) = plan.read(cx).title_and_status(cx);
            let identity = plan.entity_id();
            let focus = self
                .checkout_panel
                .plan_focus
                .entry(identity)
                .or_insert_with(|| cx.focus_handle().tab_stop(true))
                .clone();
            let bounds = self
                .checkout_panel
                .plan_bounds
                .entry(identity)
                .or_default()
                .clone();
            let hover_plan = plan.clone();
            let click_plan = plan.clone();
            let key_plan = plan.clone();
            rows.push(
                div()
                    .id(format!("checkout-plan-row-{identity:?}"))
                    .debug_selector(|| "checkout-plan-row".into())
                    .role(gpui::Role::Button)
                    .aria_label(format!("Plan: {title}. {status}"))
                    .aria_expanded(
                        self.checkout_panel
                            .plan_peek
                            .as_ref()
                            .is_some_and(|peek| !peek.closing && peek.plan == plan),
                    )
                    .track_focus(&focus)
                    .relative()
                    .cursor_pointer()
                    .px(rpx(SPACE_2XL))
                    .py(rpx(SPACE_LG))
                    .flex()
                    .items_start()
                    .gap(rpx(SPACE_SM))
                    .min_w_0()
                    .hover(|s| s.bg(c::BG_HOVER()))
                    .focus_visible(|s| s.bg(c::BG_HOVER()))
                    .child(
                        gpui::canvas(move |rect, _, _| bounds.set(rect), |_, (), _, _| {})
                            .absolute()
                            .inset_0(),
                    )
                    .child(
                        div()
                            .debug_selector(|| "checkout-plan-icon".into())
                            .flex_shrink_0()
                            .h(rpx(SPACE_3XL))
                            .flex()
                            .items_center()
                            .child(icon("list", ICON_SM, c::FG_DIM())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(rpx(SPACE_XS))
                            .child(
                                div()
                                    .debug_selector(|| "checkout-plan-title".into())
                                    .truncate()
                                    .text_size(rpx(TEXT_SMALL))
                                    .line_height(rpx(SPACE_3XL))
                                    .child(title),
                            )
                            .child(
                                div()
                                    .text_size(rpx(TEXT_MICRO))
                                    .text_color(c::FG_DIM())
                                    .child(status),
                            ),
                    )
                    .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                        this.plan_hover(hover_plan.clone(), *hovered, window, cx);
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_plan_peek(click_plan.clone(), true, window, cx);
                    }))
                    .on_key_down(cx.listener(
                        move |this, event: &gpui::KeyDownEvent, window, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                this.open_plan_peek(key_plan.clone(), true, window, cx);
                                window.prevent_default();
                                cx.stop_propagation();
                            }
                        },
                    ))
                    .into_any_element(),
            );
        }
        Some(
            div()
                .id("checkout-plan")
                .debug_selector(|| "checkout-plan".into())
                .flex()
                .flex_col()
                .min_w_0()
                .child(
                    div()
                        .px(rpx(SPACE_2XL))
                        .py(rpx(SPACE_SM))
                        .text_size(rpx(TEXT_MICRO))
                        .text_color(c::FG_DIM())
                        .child("Plan"),
                )
                .children(rows)
                .into_any_element(),
        )
    }

    pub(super) fn render_plan_peek(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let peek = self.checkout_panel.plan_peek.as_ref()?;
        let current = self.selected_native_plans(cx);
        if !self.checkout_panel.agents_visible
            || self.confirmation_open()
            || self.menu.is_some()
            || !current
                .iter()
                .any(|(id, plan)| *id == peek.session && *plan == peek.plan)
            || peek.plan.read(cx).is_expanded(cx)
        {
            self.checkout_panel.plan_close_task = None;
            self.checkout_panel.plan_peek_task = None;
            if let Some(peek) = self.checkout_panel.plan_peek.take() {
                peek.plan.update(cx, NativePlan::dismiss_peek);
            }
            return None;
        }
        let closing = peek.closing;
        let plan = peek.plan.clone();
        let focus = peek.focus.clone();
        let pane = self
            .codex_views
            .get(&peek.session)?
            .read(cx)
            .transcript_bounds
            .get();
        let margin = rpx(SPACE_LG).to_pixels(window.rem_size());
        if pane.size.width <= margin * 2. || pane.size.height <= margin * 2. {
            return None;
        }
        let source = self
            .checkout_panel
            .plan_bounds
            .get(&peek.plan.entity_id())?
            .get();
        let width = rpx(PEEK_WIDTH)
            .to_pixels(window.rem_size())
            .min(pane.size.width - margin * 2.);
        let height = rpx(PEEK_HEIGHT)
            .to_pixels(window.rem_size())
            .min(pane.size.height - margin * 2.);
        let y = source
            .top()
            .min(pane.bottom() - height - margin)
            .max(pane.top() + margin);
        let right = (source.left() - margin)
            .max(pane.left() + width + margin)
            .min(pane.right() - margin);
        let panel = div()
            .id("plan-peek")
            .debug_selector(|| "plan-peek".into())
            .role(gpui::Role::Dialog)
            .aria_label("Plan preview")
            .when(!closing, |panel| {
                panel.track_focus(&focus).tab_index(0).occlude()
            })
            .w(width)
            .h(height)
            .min_h_0()
            .overflow_hidden()
            .rounded(rpx(RADIUS_PANEL))
            .border_1()
            .border_color(c::BORDER_SOFT())
            .bg(c::SURFACE_RAISED())
            .on_hover(cx.listener(|this, hovered: &bool, window, cx| {
                if *hovered {
                    this.checkout_panel.plan_peek_task = None;
                } else if let Some(peek) = &this.checkout_panel.plan_peek {
                    this.plan_hover(peek.plan.clone(), false, window, cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    if let Some(peek) = &mut this.checkout_panel.plan_peek {
                        peek.explicit = true;
                    }
                }),
            )
            .on_mouse_down_out(
                cx.listener(|this, _, window, cx| this.dismiss_plan_peek(window, cx)),
            )
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.dismiss_plan_peek(window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(plan)
            .when(closing, |panel| {
                panel.relative().child(
                    div()
                        .absolute()
                        .inset_0()
                        .occlude()
                        .capture_any_mouse_down(|_, _, cx| cx.stop_propagation())
                        .capture_any_mouse_up(|_, _, cx| cx.stop_propagation()),
                )
            });
        Some(
            gpui::deferred(
                gpui::anchored()
                    .position_mode(gpui::AnchoredPositionMode::Window)
                    .anchor(gpui::Anchor::TopRight)
                    .position(gpui::point(right, y))
                    .snap_to_window_with_margin(margin)
                    .child(self.checkout_panel.plan_presence.render(
                        panel,
                        "plan-peek-presence",
                        cx,
                    )),
            )
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grove_core::codex::{Action, Connection, Handle, Provider, SavedSession, Snapshot, Status};

    fn saved(provider: Provider) -> SavedSession {
        let mut saved: SavedSession = serde_json::from_str(r#"{
            "key":"plan-preview-fixture","provider":"codex","project":"Plan fixture","cwd":"/tmp/grove-plan-preview",
            "label":"Plan fixture","roots":[],"thread_id":"plan-owner","items":[{"type":"userMessage","text":"Make an implementation plan."},{"id":"answer","type":"agentMessage","text":"The plan is ready for your review."}],"draft":"KEEP THE DRAFT",
            "rich_plans":{"revisions":[{"message_id":"answer","source":"plan.grove-plan.json","assets":[],
            "document":{"schema_version":1,"plan_id":"plan","revision_id":"r1",
                "title":"Review a deliberately long plan title beside the conversation without losing the current workspace",
                "summary":"Resolve the choices here, then explicitly approve implementation in the same conversation.",
                "claims":[{"id":"first","text":"Keep the plan close to the conversation.",
                    "decisions":[{"id":"location","prompt":"Where should the work happen?","default":"alpha",
                        "options":[{"id":"alpha","label":"Current checkout","description":"Keep the same working tree."}]}],
                    "children":[{"id":"second","text":"Verify the approved change before delivery.",
                        "decisions":[{"id":"checks","prompt":"Which verification should run?","options":[{"id":"beta","label":"Focused checks","description":"Exercise the changed interactions."}]}]}]}]}}]}
        }"#).unwrap();
        saved.provider = provider;
        saved
    }

    fn init(cx: &mut App) {
        gpui_component::init(cx);
        crate::fonts::register(cx).unwrap();
        c::sync_component_theme(cx);
        cx.set_reduce_motion(true);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            projects: vec![grove_core::storage::Project {
                name: "Plan fixture".into(),
                path: "/tmp/grove-plan-preview-project".into(),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            }],
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        cx.set_global(crate::zoom::ZoomState::new(1.0));
    }

    fn sidebar(
        provider: Provider,
        decisions: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> (Entity<Sidebar>, std::sync::mpsc::Receiver<Action>) {
        let runtime = cx.new(crate::runtime::Runtime::new);
        let id = runtime.read(cx).registry.clone().update(cx, |registry, _| {
            registry.insert_meta(
                "Plan fixture".into(),
                "/tmp/grove-plan-preview".into(),
                provider.agent(),
            )
        });
        let (handle, actions) = Handle::channel();
        let (_updates, receive) = std::sync::mpsc::channel();
        let mut saved = saved(provider);
        if !decisions {
            saved.rich_plans.revisions[0].document.claims[0]
                .decisions
                .clear();
            saved.rich_plans.revisions[0].document.claims[0]
                .children
                .clear();
        }
        let mut snapshot = Snapshot::new(saved);
        snapshot.status = Status::Idle;
        let session = cx.new(|cx| {
            crate::entities::codex_session::CodexSession::from_connection(
                snapshot,
                Connection {
                    handle: handle.clone(),
                    updates: receive.into(),
                },
                cx,
            )
        });
        runtime
            .read(cx)
            .registry
            .clone()
            .update(cx, |registry, _| registry.attach_codex(id, session, handle));
        let sidebar = cx.new(|cx| {
            let mut sidebar = Sidebar::new(runtime, window, cx);
            sidebar.selection = Some(Selection::Session(id));
            sidebar.initial_selection_pending = false;
            sidebar.checkout_panel.expanded = true;
            sidebar
        });
        (sidebar, actions)
    }

    /// Provider fixtures exercise native ownership, geometry and explicit actions;
    /// these are not authenticated provider-boundary tests.
    #[gpui::test]
    fn codex_and_claude_plan_peek_decisions_footer_approval_and_focus(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(init);
        // The fake text backend has pathological intrinsic sizing for the full
        // nested document. Real native snapshots below cover that selected tree;
        // this harness independently checks nested peek and simple expanded actions.
        for (provider, decisions) in [
            (Provider::Codex, true),
            (Provider::Claude, true),
            (Provider::Codex, false),
            (Provider::Claude, false),
        ] {
            let mut captured = None;
            let (_, cx) = cx.add_window_view(|window, cx| {
                let (sidebar, actions) = sidebar(provider, decisions, window, cx);
                captured = Some((sidebar.clone(), actions));
                gpui_component::Root::new(sidebar, window, cx)
            });
            let (sidebar, actions) = captured.unwrap();
            cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(1000.)));
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            let row = cx
                .debug_bounds("checkout-plan-row")
                .expect("plan row appears without registered checkout or agents");
            assert!(cx.debug_bounds("checkout-agents").is_none());
            let icon = cx.debug_bounds("checkout-plan-icon").unwrap();
            let title = cx.debug_bounds("checkout-plan-title").unwrap();
            assert_eq!(
                icon.center().y,
                title.center().y,
                "plan glyph aligns with its title line"
            );
            // Hover intent preserves composer focus, then permits a short pointer
            // crossing through the gap into the anchored preview.
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    let Some(Selection::Session(id)) = sidebar.selection else {
                        unreachable!()
                    };
                    sidebar.codex_views[&id]
                        .read(cx)
                        .focus_handle(cx)
                        .focus(window, cx);
                });
            });
            let outside = gpui::point(gpui::px(1.), gpui::px(1.));
            cx.simulate_mouse_move(outside, None, gpui::Modifiers::default());
            cx.simulate_mouse_move(row.center(), None, gpui::Modifiers::default());
            cx.run_until_parked();
            assert!(sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.plan_peek.is_none()));
            cx.executor().advance_clock(Duration::from_millis(221));
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            assert!(sidebar.read_with(cx, |sidebar, _| sidebar
                .checkout_panel
                .plan_peek
                .as_ref()
                .is_some_and(|peek| !peek.explicit)));
            assert!(
                cx.update(|window, cx| {
                    let sidebar = sidebar.read(cx);
                    let Some(Selection::Session(id)) = sidebar.selection else {
                        unreachable!()
                    };
                    sidebar.codex_views[&id]
                        .read(cx)
                        .focus_handle(cx)
                        .is_focused(window)
                }),
                "hover keeps composer focus"
            );
            let hover_peek = cx.debug_bounds("plan-peek").expect("hover opens preview");
            cx.simulate_mouse_move(outside, None, gpui::Modifiers::default());
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(100));
            cx.run_until_parked();
            assert!(sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.plan_peek.is_some()));
            cx.simulate_mouse_move(hover_peek.center(), None, gpui::Modifiers::default());
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(300));
            cx.run_until_parked();
            assert!(
                sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.plan_peek.is_some()),
                "entering peek cancels gap dismissal"
            );
            // Pin by clicking inert title chrome, without accidentally answering.
            cx.simulate_click(
                gpui::point(
                    hover_peek.left() + gpui::px(30.),
                    hover_peek.top() + gpui::px(20.),
                ),
                gpui::Modifiers::default(),
            );
            cx.simulate_mouse_move(outside, None, gpui::Modifiers::default());
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(300));
            cx.run_until_parked();
            assert!(
                sidebar.read_with(cx, |sidebar, _| sidebar
                    .checkout_panel
                    .plan_peek
                    .as_ref()
                    .is_some_and(|peek| peek.explicit)),
                "clicked preview stays pinned after pointer leaves"
            );
            cx.simulate_keystrokes("escape");
            cx.run_until_parked();
            assert!(sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.plan_peek.is_none()));
            for key in ["enter", "space"] {
                cx.update(|window, cx| {
                    sidebar.update(cx, |sidebar, cx| {
                        let (_, plan) = sidebar.selected_native_plans(cx).pop().unwrap();
                        sidebar.checkout_panel.plan_focus[&plan.entity_id()].focus(window, cx);
                    });
                });
                cx.simulate_keystrokes(key);
                cx.run_until_parked();
                assert!(
                    sidebar.read_with(cx, |sidebar, _| sidebar
                        .checkout_panel
                        .plan_peek
                        .as_ref()
                        .is_some_and(|peek| peek.explicit)),
                    "keyboard opens pinned peek"
                );
                cx.simulate_keystrokes("escape");
                cx.run_until_parked();
                assert!(
                    cx.update(|window, cx| sidebar
                        .read(cx)
                        .checkout_panel
                        .plan_focus
                        .values()
                        .any(|focus| focus.is_focused(window))),
                    "Escape restores keyboard source"
                );
            }
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            cx.simulate_click(row.center(), gpui::Modifiers::default());
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            let peek = cx
                .debug_bounds("plan-peek")
                .expect("row opens anchored peek");
            let body = cx.debug_bounds("native-plan-body").unwrap();
            let footer = cx.debug_bounds("native-plan-footer").unwrap();
            let header = cx.debug_bounds("native-plan-header").unwrap();
            assert!(
                header.bottom() <= body.top(),
                "scroll body starts below fixed title"
            );
            assert!(
                body.bottom() <= footer.top(),
                "actions stay outside body scrolling"
            );
            assert!(footer.bottom() <= peek.bottom());
            let progress = cx.debug_bounds("native-plan-progress").unwrap();
            let line_height = cx.update(|window, _| rpx(CONTROL_H).to_pixels(window.rem_size()));
            assert!(
                progress.size.height <= line_height,
                "progress must remain a readable line"
            );
            if decisions {
                let approve = cx.debug_bounds("native-plan-approve").unwrap();
                cx.simulate_click(approve.center(), gpui::Modifiers::default());
                assert!(
                    !actions
                        .try_iter()
                        .any(|a| matches!(a, Action::ApprovePlan { .. })),
                    "recommendation alone cannot approve"
                );
                let first = cx.debug_bounds("native-plan-option-alpha").unwrap();
                cx.simulate_click(first.center(), gpui::Modifiers::default());
                cx.run_until_parked();
                cx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
                let next = cx.debug_bounds("native-plan-next-decision").unwrap();
                cx.simulate_click(next.center(), gpui::Modifiers::default());
                cx.run_until_parked();
                cx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
                assert!(
                    cx.debug_bounds("native-plan-decision-checks").is_some(),
                    "nested next decision resolves in the peek"
                );
                assert!(cx.debug_bounds("native-plan-expanded").is_none());
                let second = cx.debug_bounds("native-plan-option-beta").unwrap();
                cx.simulate_click(second.center(), gpui::Modifiers::default());
                cx.run_until_parked();
                cx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
            } else {
                let expand = cx.debug_bounds("native-plan-expand").unwrap();
                cx.simulate_click(expand.center(), gpui::Modifiers::default());
                cx.run_until_parked();
                cx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
                assert!(cx.debug_bounds("native-plan-expanded").is_some());
                assert!(cx.debug_bounds("plan-peek").is_none());
                for width in [760., 1280.] {
                    cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(900.)));
                    cx.run_until_parked();
                    cx.update(|window, cx| {
                        let _ = window.draw(cx);
                    });
                    let body = cx.debug_bounds("native-plan-body").unwrap();
                    let footer = cx.debug_bounds("native-plan-footer").unwrap();
                    let approve = cx.debug_bounds("native-plan-approve").unwrap();
                    assert!(body.bottom() <= footer.top());
                    assert!(approve.bottom() <= footer.bottom());
                    assert!(approve.right() <= footer.right());
                }
            }
            let approve = cx.debug_bounds("native-plan-approve").unwrap();
            cx.simulate_click(approve.center(), gpui::Modifiers::default());
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            cx.simulate_click(approve.center(), gpui::Modifiers::default());
            assert_eq!(actions.try_iter().filter(|a| matches!(a, Action::ApprovePlan { plan_id, revision_id } if plan_id == "plan" && revision_id == "r1")).count(), 1);
            cx.update(|_, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    let (id, _) = sidebar.selected_native_plans(cx).pop().unwrap();
                    let session = sidebar
                        .runtime
                        .read(cx)
                        .registry
                        .read(cx)
                        .codex(id)
                        .cloned()
                        .unwrap();
                    assert_eq!(session.read(cx).state.saved.draft, "KEEP THE DRAFT");
                });
            });
            cx.simulate_keystrokes("escape");
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            assert!(cx.debug_bounds("native-plan-expanded").is_none());
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, _cx| {
                    assert!(
                        sidebar
                            .checkout_panel
                            .plan_focus
                            .values()
                            .any(|focus| focus.is_focused(window)),
                        "return restores plan row focus"
                    );
                });
            });
        }
    }

    #[test]
    #[ignore = "requires macOS Metal renderer; writes native plan/sidebar screenshots to /tmp"]
    #[cfg(target_os = "macos")]
    fn plan_sidebar_native_snapshots() {
        use gpui::AppContext as _;
        let mut cx = gpui::HeadlessAppContext::with_platform(
            gpui_platform::current_platform(true).text_system(),
            std::sync::Arc::new(crate::assets::Assets),
            gpui_platform::current_headless_renderer,
        );
        cx.update(|cx| {
            init(cx);
            crate::fonts::register(cx).unwrap();
            c::sync_component_theme(cx);
        });
        let mut action_channels = Vec::new();
        for (provider, label, width) in [
            (Provider::Codex, "codex-wide", 1440.),
            (Provider::Claude, "claude-narrow", 900.),
        ] {
            let mut captured = None;
            let window = cx
                .open_window(
                    gpui::size(gpui::px(width), gpui::px(1000.)),
                    |window, cx| {
                        let (sidebar, actions) = sidebar(provider, true, window, cx);
                        action_channels.push(actions);
                        captured = Some(sidebar.clone());
                        cx.new(|cx| {
                            gpui_component::Root::new(sidebar, window, cx)
                                .bordered(false)
                                .bg(c::BG())
                        })
                    },
                )
                .unwrap();
            cx.run_until_parked();
            for mode in ["row", "peek", "expanded"] {
                cx.update_window(window.into(), |_, window, cx| {
                    captured.as_ref().unwrap().update(cx, |sidebar, cx| {
                        let (_, plan) = sidebar
                            .selected_native_plans(cx)
                            .pop()
                            .expect("plan exposed in tools");
                        if mode == "peek" {
                            sidebar.open_plan_peek(plan, true, window, cx);
                        } else if mode == "expanded" {
                            plan.update(cx, |plan, cx| {
                                plan.answer_preview_fixture(window, cx);
                                plan.reveal(window, cx);
                            });
                        }
                    });
                })
                .unwrap();
                cx.run_until_parked();
                cx.update_window(window.into(), |_, window, cx| {
                    let _ = window.draw(cx);
                })
                .unwrap();
                let path = format!("/tmp/grove-plan-sidebar-{label}-{mode}.png");
                cx.capture_screenshot(window.into())
                    .unwrap()
                    .save(&path)
                    .unwrap();
                eprintln!("Native plan/sidebar fixture: {path}");
            }
            cx.update_window(window.into(), |_, window, _| window.remove_window())
                .unwrap();
        }
    }
}
