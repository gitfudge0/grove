//! An attached, checkout-owned PTY. Visibility never owns the process lifetime.
use super::*;
use crate::views::components::transcript_action;

const TERMINAL_PANE_H: f32 = 220.;
const TERMINAL_HEADER_H: f32 = 36.;
const TERMINAL_MAX_FRACTION: f32 = 0.45;

#[derive(Default)]
pub(super) struct WorktreeTerminal {
    shown: HashSet<String>,
    pub(super) views: HashMap<SessionId, Entity<TerminalView>>,
    pending_focus: Option<String>,
    observers: HashMap<SessionId, gpui::Subscription>,
    signatures: HashMap<SessionId, (Option<String>, bool)>,
    motion: Option<(String, Instant, f32, f32)>,
}

impl Sidebar {
    pub(super) fn open_worktree_terminal(&mut self, path: &str, cx: &mut Context<Self>) {
        // Reject a stale row after a workspace, project or multi-root switch.
        if !self
            .checkout_targets(cx)
            .iter()
            .any(|(_, target)| target == path)
        {
            return;
        }
        let registry = self.runtime.read(cx).registry.clone();
        let active = registry.read(cx).active_wt_shell_idx(path);
        let existing = active.and_then(|idx| registry.read(cx).wt_shell(path, idx).cloned());
        if existing
            .as_ref()
            .is_none_or(|shell| shell.read(cx).has_exited())
        {
            if !self
                .runtime
                .update(cx, |runtime, cx| runtime.spawn_wt_shell(path, cx))
            {
                return;
            }
            // A replacement is explicit; hidden or selected checkouts never respawn.
            if let Some(idx) = active {
                registry.update(cx, |registry, cx| {
                    registry.close_wt_shell(path, idx);
                    cx.notify();
                });
            }
        }
        let was_shown = self.worktree_terminal.shown.contains(path);
        if !was_shown {
            self.start_worktree_terminal_motion(path, true, cx);
        }
        self.worktree_terminal.shown.insert(path.to_owned());
        self.worktree_terminal.pending_focus = Some(path.to_owned());
        cx.notify();
    }

    fn hide_worktree_terminal(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.start_worktree_terminal_motion(path, false, cx);
        self.worktree_terminal.shown.remove(path);
        self.worktree_terminal.pending_focus = None;
        self.focus_worktree_terminal_trigger(window, cx);
        cx.notify();
    }

    fn worktree_terminal_fraction(&self, path: &str) -> f32 {
        match &self.worktree_terminal.motion {
            Some((target, started, from, to)) if target == path => {
                let progress = (started.elapsed().as_secs_f32() / (MOTION_BASE_MS as f32 / 1000.))
                    .clamp(0., 1.);
                from + (to - from) * motion::ease_standard(progress)
            }
            _ => {
                if self.worktree_terminal.shown.contains(path) {
                    1.
                } else {
                    0.
                }
            }
        }
    }

    fn start_worktree_terminal_motion(&mut self, path: &str, show: bool, cx: &App) {
        if cx.reduce_motion() {
            self.worktree_terminal.motion = None;
        } else {
            let from = self.worktree_terminal_fraction(path);
            self.worktree_terminal.motion = Some((
                path.to_owned(),
                Instant::now(),
                from,
                if show { 1. } else { 0. },
            ));
        }
    }

    pub(super) fn render_content(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let body = self.render_checkout_content(window, cx);
        self.attach_worktree_terminal(body, window, cx)
    }

    // Keep terminal composition out of the checkout/dashboard builder's stack frame.
    fn attach_worktree_terminal(
        &mut self,
        body: AnyElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Home has no session header to host its compact Checkout disclosure.
        let body = if self.dashboard_open {
            match self.checkout_panel.narrow_content.take() {
                Some(panel) => div()
                    .size_full()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(panel)
                    .child(div().flex_1().min_h_0().child(body))
                    .into_any_element(),
                None => body,
            }
        } else {
            body
        };
        let registry = self.runtime.read(cx).registry.clone();
        let live: HashSet<_> = registry
            .read(cx)
            .worktree_shells()
            .map(|meta| meta.id)
            .collect();
        let live_paths: HashSet<_> = registry
            .read(cx)
            .worktree_shells()
            .map(|meta| meta.wt_path.as_str())
            .collect();
        self.worktree_terminal
            .shown
            .retain(|path| live_paths.contains(path.as_str()));
        self.worktree_terminal
            .views
            .retain(|id, _| live.contains(id));
        self.worktree_terminal
            .observers
            .retain(|id, _| live.contains(id));
        self.worktree_terminal
            .signatures
            .retain(|id, _| live.contains(id));
        // Editors, history, grid and Zen keep their own surface and focus ownership.
        if self.is_zen()
            || (self.mode == ViewMode::Grid && !self.dashboard_open)
            || self.project_panel.is_some()
            || self.project_setup.is_some()
            || self.pending_new_worktree.is_some()
            || self
                .settings_panel
                .as_ref()
                .is_some_and(|settings| settings.read(cx).is_open())
            || (self.destination == global_navigation::Destination::History
                && self.session_history_selected.is_some())
        {
            return body;
        }
        let targets = self.checkout_targets(cx);
        let Some((_, path)) =
            checkout_panel::selected_checkout(&targets, self.checkout_panel.chosen_root.as_deref())
        else {
            return body;
        };
        let shown = self.worktree_terminal.shown.contains(&path);
        let fraction = if cx.reduce_motion() {
            self.worktree_terminal.motion = None;
            if shown {
                1.
            } else {
                0.
            }
        } else {
            let fraction = self.worktree_terminal_fraction(&path);
            if self
                .worktree_terminal
                .motion
                .as_ref()
                .is_some_and(|(target, started, _, _)| {
                    target == &path && started.elapsed() < Duration::from_millis(MOTION_BASE_MS)
                })
            {
                window.request_animation_frame();
            } else {
                self.worktree_terminal.motion = None;
            }
            fraction
        };
        if !shown && fraction <= 0. {
            return body;
        }
        let Some(idx) = registry.read(cx).active_wt_shell_idx(&path) else {
            return body;
        };
        let Some(meta) = registry.read(cx).wt_shells(&path).get(idx).cloned() else {
            return body;
        };
        let Some(session) = registry.read(cx).wt_shell(&path, idx).cloned() else {
            return body;
        };
        if !self.worktree_terminal.observers.contains_key(&meta.id) {
            let id = meta.id;
            self.worktree_terminal.signatures.insert(
                id,
                (
                    session.read(cx).current_cwd().map(str::to_owned),
                    session.read(cx).has_exited(),
                ),
            );
            let observer = cx.observe(&session, move |this, session, cx| {
                let signature = (
                    session.read(cx).current_cwd().map(str::to_owned),
                    session.read(cx).has_exited(),
                );
                if this.worktree_terminal.signatures.get(&id) != Some(&signature) {
                    this.worktree_terminal.signatures.insert(id, signature);
                    cx.notify();
                }
            });
            self.worktree_terminal.observers.insert(id, observer);
        }
        let cwd = session
            .read(cx)
            .current_cwd()
            .or(session.read(cx).initial_cwd())
            .unwrap_or(&path)
            .to_owned();
        let exited = session.read(cx).has_exited();
        let view = self
            .worktree_terminal
            .views
            .entry(meta.id)
            .or_insert_with(|| cx.new(|cx| TerminalView::new(session, cx)))
            .clone();
        if shown && self.worktree_terminal.pending_focus.as_deref() == Some(&path) {
            view.focus_handle(cx).focus(window, cx);
            self.worktree_terminal.pending_focus = None;
        }
        let hide_path = path.clone();
        let pane_contents = div()
            .w_full()
            .h(rpx(TERMINAL_PANE_H))
            .max_h_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(c::BG())
            .child(
                div()
                    .id("worktree-terminal-header")
                    .debug_selector(|| "worktree-terminal-header".into())
                    .h(rpx(TERMINAL_HEADER_H))
                    .flex_shrink_0()
                    .px(rpx(SPACE_2XL))
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_LG))
                    .bg(c::BG_STRIP())
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .child(icon("terminal", ICON_MD, c::FG_DIM()))
                    .child(if exited {
                        "Terminal · Exited"
                    } else {
                        "Terminal"
                    })
                    .child(div().flex_1().min_w_0().truncate().child(cwd))
                    .child(
                        transcript_action(
                            "hide-worktree-terminal",
                            "Hide terminal",
                            false,
                            window,
                            cx,
                        )
                        .debug_selector(|| "hide-worktree-terminal".into())
                        .icon_only()
                        .w(rpx(ICON_BTN_W))
                        .tooltip("Hide terminal")
                        .icon(gpui_component::Icon::default().path("icons/close.svg"))
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.hide_worktree_terminal(&hide_path, window, cx);
                            },
                        )),
                    ),
            )
            .child(
                div()
                    .id("worktree-terminal-body")
                    .debug_selector(|| "worktree-terminal-body".into())
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .px(rpx(SPACE_LG))
                    .child(view),
            );
        let pane = div()
            .id("worktree-terminal-pane")
            .aria_label(format!("Terminal in {path}"))
            .debug_selector(|| "worktree-terminal-pane".into())
            .w_full()
            .h(rpx(TERMINAL_PANE_H * fraction))
            .max_h(gpui::relative(TERMINAL_MAX_FRACTION * fraction))
            .flex_shrink_0()
            .min_h_0()
            .overflow_hidden()
            .border_t_1()
            .border_color(c::BORDER())
            .bg(c::BG())
            .when(shown, |pane| pane.child(pane_contents));
        div()
            .id("worktree-terminal-layout")
            .debug_selector(|| "worktree-terminal-layout".into())
            .size_full()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("worktree-terminal-upper")
                    .debug_selector(|| "worktree-terminal-upper".into())
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_hidden()
                    .child(body),
            )
            .child(pane)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draw(cx: &mut gpui::VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    #[gpui::test]
    fn worktree_terminal_pushes_home_up_and_reopens_the_same_pty(cx: &mut gpui::TestAppContext) {
        let path = std::env::temp_dir().join(format!(
            "grove-bottom-pane-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs_err::create_dir(&path).unwrap();
        let path = path.to_string_lossy().into_owned();
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_reduce_motion(true);
            cx.set_global(crate::zoom::ZoomState::new(1.));
            cx.set_global(crate::theme::ThemeState::new(
                false,
                "grove-dark".into(),
                "grove-light".into(),
            ));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            let mut store = grove_core::storage::Store {
                projects: vec![grove_core::storage::Project {
                    name: "Bottom terminal fixture".into(),
                    path: path.clone(),
                    archived: false,
                    worktree_dir: None,
                    scripts: Default::default(),
                }],
                sidebar_collapsed: true,
                native_agents_enabled: true,
                ..Default::default()
            };
            store.assign_project_to_active_workspace(&path);
            cx.set_global(SettingsState::new(store));
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        cx.simulate_resize(gpui::size(gpui::px(1440.), gpui::px(900.)));
        draw(cx);
        let finder = cx.debug_bounds("checkout-finder").unwrap();
        let open = cx.debug_bounds("checkout-terminal").unwrap();
        assert_eq!(open.top() - finder.bottom(), gpui::px(SPACE_XS));
        assert_eq!(finder.left(), open.left());
        assert_eq!(finder.size.height, open.size.height);
        let height_before = cx.debug_bounds("dashboard-page").unwrap().size.height;
        assert!(cx.debug_bounds("worktree-terminal-pane").is_none());
        cx.simulate_click(open.center(), gpui::Modifiers::default());
        draw(cx);
        let pane = cx.debug_bounds("worktree-terminal-pane").unwrap();
        let upper = cx.debug_bounds("worktree-terminal-upper").unwrap();
        let height_after = cx.debug_bounds("dashboard-page").unwrap().size.height;
        assert!((f32::from(height_before - height_after - pane.size.height)).abs() < 1.);
        assert_eq!(upper.bottom(), pane.top());
        let first = cx.update(|window, cx| {
            let sidebar = sidebar.read(cx);
            let registry = sidebar.runtime.read(cx).registry.read(cx);
            assert!(registry.all().is_empty());
            assert!(registry.home_terminals().is_empty());
            let terminal = registry.wt_shell(&path, 0).unwrap().clone();
            assert_eq!(terminal.read(cx).initial_cwd(), Some(path.as_str()));
            assert!(sidebar
                .worktree_terminal
                .views
                .values()
                .any(|view| view.focus_handle(cx).is_focused(window)));
            terminal
        });
        let hide = cx.debug_bounds("hide-worktree-terminal").unwrap();
        cx.simulate_click(hide.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("worktree-terminal-pane").is_none());
        assert_eq!(
            cx.debug_bounds("dashboard-page").unwrap().size.height,
            height_before
        );
        // Both provider fixtures use the same shell without submitting or changing drafts.
        for provider in [
            grove_core::codex::Provider::Codex,
            grove_core::codex::Provider::Claude,
        ] {
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    let dashboard = sidebar.dashboard.as_ref().unwrap();
                    dashboard.session.update(cx, |session, _| {
                        session.state.saved.provider = provider;
                        session.state.saved.draft = "Preserve this agent draft".into();
                    });
                    sidebar.focus_worktree_terminal_trigger(window, cx);
                });
            });
            cx.simulate_keystrokes("enter");
            draw(cx);
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    let registry = sidebar.runtime.read(cx).registry.read(cx);
                    assert_eq!(registry.wt_shells(&path).len(), 1);
                    assert_eq!(registry.wt_shell(&path, 0), Some(&first));
                    assert!(registry.all().is_empty());
                    assert_eq!(
                        sidebar
                            .dashboard
                            .as_ref()
                            .unwrap()
                            .session
                            .read(cx)
                            .state
                            .saved
                            .draft,
                        "Preserve this agent draft"
                    );
                    sidebar.hide_worktree_terminal(&path, window, cx);
                });
            });
            draw(cx);
        }
        cx.update(|_, cx| {
            sidebar.update(cx, |sidebar, cx| sidebar.open_worktree_terminal(&path, cx))
        });
        cx.simulate_resize(gpui::size(gpui::px(760.), gpui::px(640.)));
        draw(cx);
        let pane = cx.debug_bounds("worktree-terminal-pane").unwrap();
        let upper = cx.debug_bounds("worktree-terminal-upper").unwrap();
        assert_eq!(upper.bottom(), pane.top());
        assert!(pane.size.height <= gpui::px(640. * TERMINAL_MAX_FRACTION));
        cx.simulate_resize(gpui::size(gpui::px(320.), gpui::px(540.)));
        draw(cx);
        let pane = cx.debug_bounds("worktree-terminal-pane").unwrap();
        let hide = cx.debug_bounds("hide-worktree-terminal").unwrap();
        assert!(hide.left() >= pane.left() && hide.right() <= pane.right());
        // A reversed slide continues from the intermediate height; Reduce motion
        // clears the transition and applies the exact final layout immediately.
        cx.update(|window, cx| {
            cx.set_reduce_motion(false);
            sidebar.update(cx, |sidebar, cx| {
                sidebar.hide_worktree_terminal(&path, window, cx);
                let (_, started, _, _) = sidebar.worktree_terminal.motion.as_mut().unwrap();
                *started = Instant::now() - Duration::from_millis(MOTION_BASE_MS / 2);
                let fraction = sidebar.worktree_terminal_fraction(&path);
                assert!(fraction > 0. && fraction < 1.);
                sidebar.open_worktree_terminal(&path, cx);
                let (_, _, from, to) = sidebar.worktree_terminal.motion.as_ref().unwrap();
                assert!((*from - fraction).abs() < 0.05);
                assert_eq!(*to, 1.);
            });
            cx.set_reduce_motion(true);
        });
        draw(cx);
        cx.update(|_, cx| {
            assert!(sidebar.read(cx).worktree_terminal.motion.is_none());
            first.update(cx, |shell, _| shell.send(b"exit\n"));
        });
        let _ = fs_err::remove_dir_all(path);
    }
}
