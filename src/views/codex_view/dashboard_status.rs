//! One preparation slot for the main Dashboard composer and its first handoff.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Preparation {
    label: String,
    retry: Option<Retry>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Retry {
    Worktrees,
    Catalog,
}
impl Preparation {
    fn working(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            retry: None,
        }
    }
    fn failed(label: impl Into<String>, retry: Retry) -> Self {
        Self {
            label: label.into(),
            retry: Some(retry),
        }
    }
}
impl CodexView {
    pub(crate) fn track_dashboard_handoff(
        &mut self,
        session: Entity<CodexSession>,
        cx: &mut Context<Self>,
    ) {
        let observer = cx.observe(&session, |_, _, cx| cx.notify());
        self.dashboard_handoffs
            .push((session.downgrade(), observer));
        cx.notify();
    }
    pub(crate) fn dashboard_handoff_sessions(&self, cx: &App) -> Vec<Entity<CodexSession>> {
        self.dashboard_handoffs
            .iter()
            .filter_map(|(session, _)| session.upgrade())
            .filter(|session| session.read(cx).dashboard_launch_pending())
            .collect()
    }
    fn dashboard_preparation(&self, state: &codex::Snapshot, cx: &App) -> Vec<Preparation> {
        let mut entries = Vec::new();
        if let Some(tray) = &self.dashboard_tray {
            let (loading, error) = tray.read(cx).preparation();
            if let Some(error) = error {
                entries.push(Preparation::failed(
                    format!("Could not load worktrees: {error}"),
                    Retry::Worktrees,
                ));
            }
            let session = self.session.read(cx);
            if let Some(error) = &session.catalog_error {
                entries.push(Preparation::failed(error.clone(), Retry::Catalog));
            }
            if self.pending_image_pastes > 0 {
                entries.push(Preparation::working("Preparing images…"));
            }
            if self.pending_attachment_picker {
                entries.push(Preparation::working("Preparing attachments…"));
            }
            if loading {
                entries.push(Preparation::working("Loading worktrees…"));
            }
            if session.catalog_loading {
                entries.push(Preparation::working(format!(
                    "Loading {} options…",
                    state.saved.provider.label()
                )));
            }
        }
        for (session, _) in &self.dashboard_handoffs {
            let Some(session) = session.upgrade() else {
                continue;
            };
            let session = session.read(cx);
            if !session.dashboard_launch_pending() {
                continue;
            }
            let provider = session.state.saved.provider.label();
            let label = if session.state.status == codex::Status::Starting {
                format!("Connecting to {provider}…")
            } else {
                format!("Sending first message to {provider}…")
            };
            let project = &session.state.saved.project;
            entries.push(Preparation::working(if project.is_empty() {
                label
            } else {
                format!("{label} · {project}")
            }));
        }
        entries
    }

    pub(super) fn render_dashboard_status(
        &mut self,
        state: &codex::Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        self.dashboard_tray.as_ref()?;
        self.dashboard_handoffs.retain(|(session, _)| {
            session
                .upgrade()
                .is_some_and(|session| session.read(cx).dashboard_launch_pending())
        });
        let entries = self.dashboard_preparation(state, cx);
        // Preserve typing when async work removes the focused status control.
        // A user who moved to another control keeps that focus.
        let expanded = self.dashboard_status_expanded && self.dashboard_status_signature == entries;
        let visible = if expanded { entries.len() } else { 1 };
        let lost_more = self.dashboard_more_focus.is_focused(window) && entries.len() < 2;
        let lost_retry =
            [Retry::Worktrees, Retry::Catalog]
                .iter()
                .enumerate()
                .any(|(index, retry)| {
                    self.dashboard_retry_focus[index].is_focused(window)
                        && !entries
                            .iter()
                            .take(visible)
                            .any(|entry| entry.retry == Some(*retry))
                });
        if lost_more || lost_retry {
            self.input.focus_handle(cx).focus(window, cx);
        }
        // A new operation/provider/destination must not inherit an old disclosure.
        if self.dashboard_status_signature != entries {
            self.dashboard_status_expanded = false;
            self.dashboard_status_signature.clone_from(&entries);
        }
        let mut slot = div()
            .id("dashboard-composer-status")
            .debug_selector(|| "dashboard-composer-status".into())
            .w_full()
            .min_h(rpx(CONTROL_H))
            .flex_shrink_0()
            .flex()
            .flex_col();
        let Some(primary) = entries.first() else {
            return Some(slot.into_any_element());
        };
        let mut row = div()
            .w_full()
            .min_h(rpx(CONTROL_H))
            .flex()
            .items_start()
            .gap(rpx(SPACE_SM))
            .child(self.dashboard_preparation_label(primary, 0, cx));
        if entries.len() > 1 {
            row = row.child(
                header_control("dashboard-status-more", "Show other preparation activity")
                    .debug_selector(|| "dashboard-status-more".into())
                    .aria_expanded(self.dashboard_status_expanded)
                    .track_focus(&self.dashboard_more_focus)
                    .tab_index(0)
                    .w_auto()
                    .flex_shrink_0()
                    .gap(rpx(SPACE_SM))
                    .focus_visible(|s| s.bg(c::BG_HOVER()))
                    .child(crate::icons::icon(
                        if self.dashboard_status_expanded {
                            "chev-up"
                        } else {
                            "chev-down"
                        },
                        ICON_SM,
                        c::FG_DIM(),
                    ))
                    .child(format!("{} more", entries.len() - 1))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.dashboard_status_expanded = !this.dashboard_status_expanded;
                        cx.notify();
                    }))
                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                        if action_key(event) {
                            this.dashboard_status_expanded = !this.dashboard_status_expanded;
                            cx.notify();
                            cx.stop_propagation();
                        }
                    })),
            );
        }
        slot = slot.child(row);
        if self.dashboard_status_expanded {
            slot = slot.children(entries.iter().enumerate().skip(1).map(|(index, entry)| {
                div()
                    .w_full()
                    .flex()
                    .items_start()
                    .min_h(rpx(CONTROL_H))
                    .child(self.dashboard_preparation_label(entry, index, cx))
            }));
        }
        Some(slot.into_any_element())
    }
    fn dashboard_preparation_label(
        &self,
        entry: &Preparation,
        index: usize,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let kind = if entry.retry.is_some() {
            StatusKind::Failed
        } else {
            StatusKind::Working
        };
        let mut label = div()
            .id(("dashboard-status-entry", index))
            .role(gpui::Role::Status)
            .debug_selector(move || format!("dashboard-status-entry-{index}"))
            .flex_1()
            .min_w_0()
            .flex()
            .items_start()
            .gap(rpx(SPACE_SM))
            .text_size(rpx(TEXT_SMALL))
            .aria_label(entry.label.clone())
            .text_color(kind.color())
            .line_height(rpx(CONTROL_H))
            .child(
                div()
                    .w(rpx(ICON_SM))
                    .h(rpx(CONTROL_H))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .child(status::mark(
                        format!("dashboard-preparation-{index}"),
                        kind,
                        ICON_SM,
                        cx,
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .when(entry.retry.is_none(), gpui::Styled::truncate)
                    .child(entry.label.clone()),
            );
        if let Some(retry) = entry.retry {
            label = label.child(
                header_control(
                    if retry == Retry::Worktrees {
                        "dashboard-worktrees-retry"
                    } else {
                        "dashboard-catalog-retry"
                    },
                    "Retry preparation",
                )
                .debug_selector(move || format!("dashboard-status-retry-{index}"))
                .track_focus(&self.dashboard_retry_focus[usize::from(retry != Retry::Worktrees)])
                .tab_index(0)
                .w_auto()
                .flex_shrink_0()
                .gap(rpx(SPACE_SM))
                .focus_visible(|s| s.bg(c::BG_HOVER()))
                .child(crate::icons::icon("refresh", ICON_SM, c::FG_DIM()))
                .child("Retry")
                .on_click(
                    cx.listener(move |this, _, _, cx| this.retry_dashboard_preparation(retry, cx)),
                )
                .on_key_down(cx.listener(
                    move |this, event: &gpui::KeyDownEvent, _, cx| {
                        if action_key(event) {
                            this.retry_dashboard_preparation(retry, cx);
                            cx.stop_propagation();
                        }
                    },
                )),
            );
        }
        label.into_any_element()
    }
    fn retry_dashboard_preparation(&mut self, retry: Retry, cx: &mut Context<Self>) {
        match retry {
            Retry::Catalog => self.retry_dashboard_catalog(cx),
            Retry::Worktrees => {
                if let Some(tray) = &self.dashboard_tray {
                    tray.update(
                        cx,
                        super::super::sidebar::dashboard::DestinationTray::retry_worktrees,
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{draw, fixture};
    use super::*;
    use grove_core::codex::Status;

    #[gpui::test]
    fn dashboard_status_slot_tracks_overlap_errors_and_keeps_drafts(cx: &mut gpui::TestAppContext) {
        let (view, cx, actions, _updates) = fixture(cx);
        cx.update(|_, cx| cx.set_reduce_motion(true));
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            let tray = cx.update(|window, cx| {
                let tray = cx.new(|_| {
                    super::super::super::sidebar::dashboard::DestinationTray::picker_test_tray(
                        "/unused".into(),
                    )
                });
                view.update(cx, |view, cx| {
                    view.attach_dashboard_tray(tray.clone(), cx);
                    view.session.update(cx, |session, cx| {
                        session.state.saved.provider = provider;
                        session.state.status = Status::Idle;
                        session.state.saved.items = Vec::new().into();
                        session.catalog_loading = false;
                        session.catalog_error = None;
                        cx.notify();
                    });
                    view.input.update(cx, |input, cx| {
                        input.set_value("Keep this draft", window, cx);
                    });
                    view.images = vec!["attachment-owner".into()];
                });
                tray
            });
            draw(cx);
            let idle = cx.debug_bounds("codex-composer").unwrap();
            assert!(cx.debug_bounds("dashboard-status-entry-0").is_none());
            cx.update(|_, cx| {
                tray.update(cx, |tray, cx| tray.set_preparation_for_test(true, None, cx));
                view.update(cx, |view, cx| {
                    view.pending_image_pastes = 1;
                    view.session.update(cx, |session, cx| {
                        session.catalog_loading = true;
                        cx.notify();
                    });
                    cx.notify();
                });
            });
            draw(cx);
            assert_eq!(
                cx.debug_bounds("codex-composer").unwrap(),
                idle,
                "loading keeps the writing surface in place"
            );
            assert!(cx.debug_bounds("dashboard-status-more").is_some());
            cx.update(|window, cx| {
                view.update(cx, |view, cx| view.dashboard_more_focus.focus(window, cx));
            });
            cx.simulate_keystrokes("space");
            draw(cx);
            assert!(cx.debug_bounds("dashboard-status-entry-2").is_some());
            view.read_with(cx, |view, cx| {
                let entries = view.dashboard_preparation(&view.session.read(cx).state, cx);
                assert_eq!(
                    entries
                        .iter()
                        .map(|entry| entry.label.as_str())
                        .collect::<Vec<_>>(),
                    [
                        "Preparing images…",
                        "Loading worktrees…",
                        &format!("Loading {} options…", provider.label())
                    ]
                );
            });
            // Only notify the tray: the parent must update without a session change.
            cx.update(|_, cx| {
                tray.update(cx, |tray, cx| {
                    tray.set_preparation_for_test(
                        false,
                        Some("checkout no longer exists".into()),
                        cx,
                    );
                });
            });
            draw(cx);
            assert!(cx.debug_bounds("dashboard-status-retry-0").is_some());
            assert!(
                cx.debug_bounds("dashboard-status-entry-2").is_none(),
                "changed activity collapses old disclosure"
            );
            let retries = Rc::new(Cell::new(0));
            let count = retries.clone();
            let _subscription = cx.update(|_, cx| {
                cx.subscribe(&tray, move |_, event, _| {
                    if matches!(
                        event,
                        super::super::super::sidebar::dashboard::DestinationEvent::RefreshWorktrees
                    ) {
                        count.set(count.get() + 1);
                    }
                })
            });
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.dashboard_retry_focus[0].focus(window, cx);
                });
            });
            cx.simulate_keystrokes("enter");
            draw(cx);
            assert_eq!(retries.get(), 1, "worktree retry routes to its owning tray");
            view.read_with(cx, |view, cx| {
                assert_eq!(view.input.read(cx).value().as_ref(), "Keep this draft");
                assert_eq!(view.images, ["attachment-owner"]);
            });
            assert!(
                !actions
                    .try_iter()
                    .any(|action| matches!(action, Action::SubmitWithMode { .. })),
                "loading and retry never send the draft"
            );
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    view.pending_image_pastes = 0;
                    cx.notify();
                });
            });
        }
    }

    #[gpui::test]
    fn dashboard_status_blocks_keyboard_launch_and_deduplicates_catalog_errors(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, _actions, _updates) = fixture(cx);
        let launches = Rc::new(Cell::new(0));
        let count = launches.clone();
        let _events = cx.update(|_, cx| {
            cx.subscribe(&view, move |_, event, _| {
                if matches!(event, CodexViewEvent::DashboardSubmit) {
                    count.set(count.get() + 1);
                }
            })
        });
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    let tray = cx.new(|_| {
                        super::super::super::sidebar::dashboard::DestinationTray::picker_test_tray(
                            "/unused".into(),
                        )
                    });
                    view.attach_dashboard_tray(tray, cx);
                    view.session.update(cx, |session, cx| {
                        session.state.saved.provider = provider;
                        session.state.status = Status::Idle;
                        session.catalog_loading = false;
                        session.catalog_error = None;
                        cx.notify();
                    });
                    view.input.update(cx, |input, cx| {
                        input.set_value("Keep this until ready", window, cx);
                    });
                    view.input.focus_handle(cx).focus(window, cx);
                    view.pending_attachment_picker = true;
                    cx.notify();
                });
            });
            draw(cx);
            cx.simulate_keystrokes("enter");
            draw(cx);
            assert_eq!(
                launches.get(),
                0,
                "pending attachments reject keyboard submission"
            );
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    view.pending_attachment_picker = false;
                    view.session.update(cx, |session, cx| {
                        session.catalog_loading = true;
                        cx.notify();
                    });
                    cx.notify();
                });
            });
            draw(cx);
            cx.simulate_keystrokes("enter");
            draw(cx);
            assert_eq!(
                launches.get(),
                0,
                "catalog loading rejects keyboard submission"
            );
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.session.update(cx, |session, cx| {
                        session.catalog_loading = false;
                        session.catalog_error = Some("Provider options unavailable".into());
                        session.state.error = session.catalog_error.clone();
                        cx.notify();
                    });
                    let state = view.session.read(cx).state.shared();
                    assert!(view.session_notice(false, &state, window, cx).is_none());
                    cx.notify();
                });
            });
            draw(cx);
            assert!(cx.debug_bounds("dashboard-status-retry-0").is_some());
            cx.simulate_keystrokes("enter");
            draw(cx);
            assert_eq!(
                launches.get(),
                0,
                "catalog errors reject keyboard submission"
            );
            view.read_with(cx, |view, cx| {
                assert_eq!(
                    view.input.read(cx).value().as_ref(),
                    "Keep this until ready"
                );
            });
        }
    }

    #[gpui::test]
    fn dashboard_status_preserves_native_command_send_during_catalog_loading(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, _actions, _updates) = fixture(cx);
        let commands = Rc::new(Cell::new(0));
        let count = commands.clone();
        let _events = cx.update(|_, cx| {
            cx.subscribe(&view, move |_, event, _| {
                if matches!(
                    event,
                    CodexViewEvent::ReviewChanges(..) | CodexViewEvent::DescribeChanges(..)
                ) {
                    count.set(count.get() + 1);
                }
            })
        });
        let mut expected = 0;
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            for command in ["/review local", "/describe-changes local"] {
                for failed in [false, true] {
                    cx.update(|window, cx| view.update(cx, |view, cx| {
                        let tray = cx.new(|_| super::super::super::sidebar::dashboard::DestinationTray::picker_test_tray("/unused".into()));
                        view.attach_dashboard_tray(tray, cx);
                        view.session.update(cx, |session, cx| {
                            session.state.saved.provider = provider;
                            session.state.status = Status::Idle;
                            session.catalog_loading = !failed;
                            session.catalog_error = failed.then(|| "Model options unavailable".into());
                            cx.notify();
                        });
                        view.input.update(cx, |input, cx| input.set_value(command, window, cx));
                        cx.notify();
                    }));
                    draw(cx);
                    let send = cx.debug_bounds("codex-send").unwrap();
                    cx.simulate_click(send.center(), gpui::Modifiers::default());
                    draw(cx);
                    expected += 1;
                    assert_eq!(
                        commands.get(),
                        expected,
                        "{provider:?} {command}: pointer submission needs no catalog"
                    );
                }
            }
        }
    }

    #[gpui::test]
    fn dashboard_status_restores_typing_only_when_focused_status_control_disappears(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, _actions, _updates) = fixture(cx);
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    let tray = cx.new(|_| {
                        super::super::super::sidebar::dashboard::DestinationTray::picker_test_tray(
                            "/unused".into(),
                        )
                    });
                    view.attach_dashboard_tray(tray, cx);
                    view.session.update(cx, |session, cx| {
                        session.state.saved.provider = provider;
                        session.state.status = Status::Idle;
                        session.catalog_loading = true;
                        cx.notify();
                    });
                    view.pending_image_pastes = 1;
                    view.input
                        .update(cx, |input, cx| input.set_value("Draft", window, cx));
                    view.dashboard_more_focus.focus(window, cx);
                    cx.notify();
                });
            });
            draw(cx);
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    view.pending_image_pastes = 0;
                    view.session.update(cx, |session, cx| {
                        session.catalog_loading = false;
                        cx.notify();
                    });
                    cx.notify();
                });
            });
            draw(cx);
            cx.simulate_keystrokes("x");
            draw(cx);
            view.read_with(cx, |view, cx| {
                assert!(view.input.read(cx).value().contains('x'));
            });
            for keep_other_focus in [false, true] {
                cx.update(|window, cx| {
                    view.update(cx, |view, cx| {
                        view.session.update(cx, |session, cx| {
                            session.catalog_error = Some("Try again".into());
                            cx.notify();
                        });
                        if keep_other_focus {
                            view.option_focus[0].focus(window, cx);
                        } else {
                            view.dashboard_retry_focus[1].focus(window, cx);
                        }
                        cx.notify();
                    });
                });
                draw(cx);
                cx.update(|_, cx| {
                    view.update(cx, |view, cx| {
                        view.session.update(cx, |session, cx| {
                            session.catalog_error = None;
                            cx.notify();
                        });
                    });
                });
                draw(cx);
                cx.update(|window, cx| {
                    view.read_with(cx, |view, cx| {
                        assert_eq!(
                            view.input.focus_handle(cx).is_focused(window),
                            !keep_other_focus
                        );
                        if keep_other_focus {
                            assert!(view.option_focus[0].is_focused(window));
                        }
                    });
                });
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "opens an isolated native X11 Dashboard composer fixture"]
    fn dashboard_status_native_snapshots() {
        use gpui::AppContext as _;
        use grove_core::codex::{Connection, Handle};
        use std::sync::mpsc;
        struct QaComposer {
            view: Entity<CodexView>,
            width: gpui::Pixels,
            handoff: Option<Entity<CodexSession>>,
        }
        impl Render for QaComposer {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div()
                    .w(self.width)
                    .max_w_full()
                    .h_full()
                    .bg(c::BG())
                    .child(self.view.clone())
            }
        }
        assert!(std::env::var_os("GROVE_CONFIG_DIR").is_some());
        let output =
            std::path::PathBuf::from(std::env::var("GROVE_COMPOSER_STATUS_CAPTURE_DIR").unwrap());
        assert!(output.is_absolute());
        fs_err::create_dir_all(&output).unwrap();
        // Isolate each state, as in the native reply QA, to avoid stale X11 compositor frames.
        let selected_case =
            std::env::var("GROVE_COMPOSER_STATUS_CASE").unwrap_or_else(|_| "ready".into());
        let selected_provider =
            std::env::var("GROVE_COMPOSER_STATUS_PROVIDER").unwrap_or_else(|_| "Codex".into());
        assert!(["ready", "overlap", "expanded", "error", "connecting"]
            .contains(&selected_case.as_str()));
        assert!(["Codex", "Claude"].contains(&selected_provider.as_str()));
        gpui_platform::application().with_assets(crate::assets::Assets).run(move |cx| {
            gpui_component::init(cx);
            crate::fonts::register(cx).unwrap();
            cx.set_reduce_motion(true);
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_global(crate::settings::SettingsState::new(grove_core::storage::Store::default()));
            grove_core::theme::set_by_name(if matches!(selected_case.as_str(), "error" | "connecting") { "grove-light" } else { "grove-dark" });
            c::sync_component_theme(cx);
            let (handle, actions) = Handle::channel();
            let (updates, receive) = mpsc::channel();
            let mut retained = None;
            let window = cx.open_window(gpui::WindowOptions {
                app_id: Some("dev.grove.composer-status-qa".into()),
                window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
                    gpui::point(gpui::px(30.), gpui::px(30.)), gpui::size(gpui::px(900.), gpui::px(700.))))),
                ..Default::default()
            }, |window, cx| {
                window.set_window_title("Grove composer status QA");
                let mut state = super::super::tests::snapshot();
                state.status = Status::Idle;
                state.saved.items = Vec::new().into();
                let session = cx.new(|cx| CodexSession::from_connection(state, Connection { handle, updates: receive.into() }, cx));
                let view = cx.new(|cx| CodexView::new(session, window, cx));
                view.update(cx, |view, cx| {
                    view.next_reply_generate = |_, _| Ok(Vec::new());
                    let tray = cx.new(|_| super::super::super::sidebar::dashboard::DestinationTray::picker_test_tray("/unused".into()));
                    view.attach_dashboard_tray(tray, cx);
                    view.input.update(cx, |input, cx| input.set_value("Make the composer loading states feel seamless.", window, cx));
                });
                let container = cx.new(|_| QaComposer { view: view.clone(), width: gpui::px(900.), handoff: None });
                retained = Some((view, container.clone()));
                cx.new(|cx| gpui_component::Root::new(container, window, cx).bordered(false).bg(c::BG()))
            }).unwrap();
            let (view, container) = retained.unwrap();
            cx.activate(true);
            cx.spawn(async move |cx| {
                let _transport = (actions, updates);
                for provider in [codex::Provider::Codex, codex::Provider::Claude] {
                    if format!("{provider:?}") != selected_provider { continue; }
                    for (case, theme, width, rem) in [
                        ("ready", "grove-dark", 900., 16.),
                        ("overlap", "grove-dark", 900., 16.),
                        ("expanded", "grove-dark", 900., 16.),
                        ("error", "grove-light", 360., 20.),
                        ("connecting", "grove-light", 900., 16.),
                    ] {
                        if case != selected_case { continue; }
                        cx.update(|cx| { grove_core::theme::set_by_name(theme); c::sync_component_theme(cx); });
                        cx.update_window(window.into(), |_, window, cx| {
                            window.set_rem_size(gpui::px(rem));
                            gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
                            container.update(cx, |container, cx| { container.width = gpui::px(width); container.handoff = None; cx.notify(); });
                            view.update(cx, |view, cx| {
                                view.dashboard_handoffs.clear();
                                view.pending_image_pastes = usize::from(matches!(case, "overlap" | "expanded"));
                                view.session.update(cx, |session, cx| {
                                    session.state.saved.provider = provider;
                                    session.catalog_loading = matches!(case, "overlap" | "expanded");
                                    session.catalog_error = (case == "error").then(|| "Could not load provider options: the connection timed out. Retry to load the available models.".into());
                                    session.state.error = session.catalog_error.clone();
                                    cx.notify();
                                });
                                view.dashboard_tray.as_ref().unwrap().update(cx, |tray, cx| tray.set_preparation_for_test(matches!(case, "overlap" | "expanded"), None, cx));
                                if case == "connecting" {
                                    let mut saved = view.session.read(cx).state.saved.clone();
                                    saved.project = "grove".into();
                                    let handoff = cx.new(|cx| {
                                        let mut session = CodexSession::history(saved, cx);
                                        session.state.status = Status::Starting;
                                        session.begin_dashboard_handoff_for_test();
                                        session
                                    });
                                    container.update(cx, |container, _| container.handoff = Some(handoff.clone()));
                                    view.track_dashboard_handoff(handoff, cx);
                                }
                                view.dashboard_status_expanded = false;
                                cx.notify();
                            });
                            window.refresh();
                            eprintln!("composer status {provider:?} {case}: viewport={:?}, constrained-width={width}, rem={rem}", window.viewport_size());
                        }).unwrap();
                        cx.background_executor().timer(std::time::Duration::from_millis(350)).await;
                        cx.update_window(window.into(), |_, window, cx| {
                            if case == "expanded" {
                                view.update(cx, |view, cx| view.dashboard_more_focus.focus(window, cx));
                                // Exercise the mounted native control, rather than seeding its state.
                                if !view.read(cx).dashboard_status_expanded {
                                    window.dispatch_keystroke(gpui::Keystroke::parse("space").unwrap(), cx);
                                }
                            }
                            window.refresh();
                        }).unwrap();
                        cx.background_executor().timer(std::time::Duration::from_millis(250)).await;
                        cx.update_window(window.into(), |_, window, cx| { view.update(cx, |_, cx| cx.notify()); window.refresh(); }).unwrap();
                        cx.background_executor().timer(std::time::Duration::from_millis(250)).await;
                        if case == "expanded" {
                            cx.update_window(window.into(), |_, _, cx| {
                                assert!(view.read(cx).dashboard_status_expanded, "native Space activates the activity disclosure");
                                assert_eq!(view.read(cx).input.read(cx).value().as_ref(), "Make the composer loading states feel seamless.");
                            }).unwrap();
                        }
                        assert!(std::process::Command::new("import").args(["-window", "Grove composer status QA"]).arg(output.join(format!("{provider:?}-{case}.png"))).status().unwrap().success());
                    }
                }
                // Clear the platform text handler before releasing the fixture's input entity.
                cx.update_window(window.into(), |_, window, cx| {
                    window.blur();
                    window.replace_root(cx, |_, _| gpui::Empty);
                    let _ = window.draw(cx);
                    let _ = window.draw(cx);
                }).unwrap();
                cx.background_executor().timer(std::time::Duration::from_millis(100)).await;
                cx.update_window(window.into(), |_, window, _| window.remove_window()).unwrap();
                drop(container);
                drop(view);
                cx.background_executor().timer(std::time::Duration::from_millis(100)).await;
                cx.update(|cx| cx.quit());
            }).detach();
        });
    }
}
