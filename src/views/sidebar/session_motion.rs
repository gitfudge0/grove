//! Brief, session-owned presence transitions in the expanded navigation list.
use super::*;
use std::{cell::Cell, rc::Rc};

const DURATION: Duration = Duration::from_millis(MOTION_SLOW_MS);
const INITIAL_OPACITY: f32 = 0.35;

#[derive(Clone)]
struct CardSnapshot {
    meta: SessionMeta,
    title: String,
    worktree: String,
    status: &'static str,
    tasks: Option<task_glance::TaskGlance>,
    roots: Vec<String>,
    selected: bool,
    group: String,
    group_position: usize,
    group_occurrence: usize,
    position: usize,
}

struct Transition {
    started: Instant,
    height: Rc<Cell<f32>>,
    from: f32,
    exiting: Option<CardSnapshot>,
}

impl Transition {
    fn progress(&self) -> f32 {
        let elapsed = (self.started.elapsed().as_secs_f32() / DURATION.as_secs_f32()).clamp(0., 1.);
        let t = motion::ease_standard(elapsed);
        if self.exiting.is_some() {
            self.from * (1. - t)
        } else {
            self.from + (1. - self.from) * t
        }
    }
}

#[derive(Default)]
pub(super) struct SessionMotion {
    workspace: Option<u64>,
    known: HashSet<SessionId>,
    pending_entries: HashSet<SessionId>,
    cards: HashMap<SessionId, CardSnapshot>,
    heights: HashMap<SessionId, Rc<Cell<f32>>>,
    transitions: HashMap<SessionId, Transition>,
}

impl Sidebar {
    /// Observe inventory, not status or grouping changes. Exiting rows are a
    /// passive snapshot: the underlying session closes and releases input now.
    pub(super) fn sync_session_motion(
        &mut self,
        groups: &[(String, Vec<SessionId>)],
        visible: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let registry = self.runtime.read(cx).registry.read(cx);
        let known: HashSet<_> = registry.all().iter().map(|meta| meta.id).collect();
        let reset = self.session_motion.workspace != Some(self.active_workspace)
            || !visible
            || cx.reduce_motion();
        if reset {
            self.session_motion.transitions.clear();
            self.session_motion.pending_entries.clear();
        } else {
            self.session_motion
                .pending_entries
                .extend(known.difference(&self.session_motion.known).copied());
            self.session_motion
                .pending_entries
                .retain(|id| known.contains(id));
            for (&id, card) in &self.session_motion.cards {
                if !known.contains(&id) && self.session_motion.known.contains(&id) {
                    let from = self
                        .session_motion
                        .transitions
                        .get(&id)
                        .map_or(1., Transition::progress);
                    self.session_motion.transitions.insert(
                        id,
                        Transition {
                            started: Instant::now(),
                            height: self.session_motion.heights.entry(id).or_default().clone(),
                            from,
                            exiting: Some(card.clone()),
                        },
                    );
                }
            }
            for (_, ids) in groups {
                for &id in ids {
                    if self.session_motion.pending_entries.remove(&id) {
                        self.session_motion.transitions.insert(
                            id,
                            Transition {
                                started: Instant::now(),
                                height: self.session_motion.heights.entry(id).or_default().clone(),
                                from: 0.,
                                exiting: None,
                            },
                        );
                    }
                }
            }
        }
        self.session_motion.workspace = Some(self.active_workspace);
        self.session_motion.known = known;
        self.session_motion.cards.clear();
        if visible {
            for (group_position, (group, ids)) in groups.iter().enumerate() {
                let group_occurrence = groups[..group_position]
                    .iter()
                    .filter(|(label, _)| label == group)
                    .count();
                for (position, id) in ids.iter().enumerate() {
                    let Some(meta) = registry.meta(*id).cloned() else {
                        continue;
                    };
                    let title = session_display_title(
                        &meta,
                        registry
                            .codex(*id)
                            .and_then(|session| session.read(cx).state.saved.title.clone())
                            .or_else(|| {
                                registry
                                    .session(*id)
                                    .and_then(|session| session.read(cx).title())
                            }),
                    );
                    let worktree = self
                        .snapshot
                        .projects
                        .iter()
                        .flat_map(|p| &p.worktrees)
                        .find(|wt| wt.path == meta.wt_path)
                        .map_or_else(
                            || {
                                std::path::Path::new(&meta.wt_path).file_name().map_or_else(
                                    || meta.wt_path.clone(),
                                    |name| name.to_string_lossy().into_owned(),
                                )
                            },
                            |wt| wt.name.clone(),
                        );
                    self.session_motion.heights.entry(*id).or_default();
                    self.session_motion.cards.insert(
                        *id,
                        CardSnapshot {
                            status: self.status(&meta, cx).0,
                            tasks: self.task_glance(*id).cloned(),
                            roots: session_root_inventory(&meta, &self.snapshot),
                            selected: self.selection == Some(Selection::Session(*id))
                                && !self.dashboard_open,
                            meta,
                            title,
                            worktree,
                            group: group.clone(),
                            group_position,
                            group_occurrence,
                            position,
                        },
                    );
                }
            }
        }
        self.session_motion
            .transitions
            .retain(|_, state| state.started.elapsed() < DURATION);
        self.session_motion.heights.retain(|id, _| {
            self.session_motion.cards.contains_key(id)
                || self.session_motion.transitions.contains_key(id)
        });
        if !self.session_motion.transitions.is_empty() {
            window.request_animation_frame();
        }
    }

    pub(super) fn session_motion_groups(&self, groups: &mut Vec<(String, Vec<SessionId>)>) {
        let mut ghosts: Vec<_> = self
            .session_motion
            .transitions
            .iter()
            .filter_map(|(&id, state)| state.exiting.as_ref().map(|card| (id, card)))
            .collect();
        ghosts.sort_by_key(|(id, card)| (card.group_position, card.position, id.raw()));
        for (id, card) in ghosts {
            if let Some((_, ids)) = groups
                .iter_mut()
                .filter(|(label, _)| label == &card.group)
                .nth(card.group_occurrence)
            {
                ids.insert(card.position.min(ids.len()), id);
            } else {
                groups.insert(
                    card.group_position.min(groups.len()),
                    (card.group.clone(), vec![id]),
                );
            }
        }
    }

    pub(super) fn session_motion_row(&self, id: SessionId, row: AnyElement) -> AnyElement {
        let Some(height) = self.session_motion.heights.get(&id).cloned() else {
            return row;
        };
        let inner = div()
            .w_full()
            .flex_shrink_0()
            .child(row)
            .on_prepaint(move |bounds, _, _| {
                let value = f32::from(bounds.size.height);
                if value > 0. {
                    height.set(value);
                }
            });
        let Some(state) = self.session_motion.transitions.get(&id) else {
            return inner.into_any_element();
        };
        let t = state.progress();
        let measured = state.height.get();
        div()
            .id(("sidebar-session-motion", id.raw()))
            .debug_selector(move || format!("sidebar-session-motion-{}", id.raw()))
            .w_full()
            .flex()
            .flex_col()
            .min_h_0()
            .overflow_hidden()
            .max_h(gpui::px(measured * t))
            .opacity(INITIAL_OPACITY + (1. - INITIAL_OPACITY) * t)
            .relative()
            .top(rpx(-SPACE_MD * (1. - t)))
            .child(inner)
            .into_any_element()
    }

    pub(super) fn exiting_session_row(&self, id: SessionId, cx: &App) -> Option<AnyElement> {
        let card = self.session_motion.transitions.get(&id)?.exiting.as_ref()?;
        let meta = &card.meta;
        let content = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .w_full()
                    .min_w_0()
                    .h(rpx(SESSION_LIST_HEADER_H))
                    .gap(rpx(SPACE_SM))
                    .child(div().flex_shrink_0().size(rpx(TEXT_MICRO)).child(icon(
                        meta.agent.icon_name(),
                        TEXT_MICRO,
                        c::FG_DIM(),
                    )))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(rpx(TEXT_MICRO))
                            .line_height(rpx(SESSION_LIST_FOOTER_H))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(if card.selected { c::FG() } else { c::FG_DIM() })
                            .child(meta.project.clone()),
                    )
                    .child(div().flex_shrink_0().ml(rpx(SPACE_SM)).child(icon(
                        "git-branch",
                        ICON_SM,
                        c::FG_DIM(),
                    )))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(rpx(TEXT_MICRO))
                            .line_height(rpx(SESSION_LIST_FOOTER_H))
                            .text_color(c::FG_DIM())
                            .child(card.worktree.clone()),
                    ),
            )
            .child(
                div()
                    .min_w_0()
                    .w_full()
                    .truncate()
                    .mt(rpx(SPACE_SM))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_size(rpx(TEXT_TITLE))
                    .line_height(rpx(SESSION_LIST_TITLE_H))
                    .child(single_line_session_title(&card.title)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .w_full()
                    .min_w_0()
                    .mt(rpx(SPACE_XS))
                    .h(rpx(CHROME_CONTROL_H))
                    .gap(rpx(SPACE_SM))
                    .when_some(card.tasks.as_ref(), |line, tasks| {
                        line.child(task_glance::row_progress(id, tasks))
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .min_w_0()
                            .gap(rpx(SPACE_SM))
                            .text_size(rpx(TEXT_MICRO))
                            .line_height(rpx(SESSION_LIST_FOOTER_H))
                            .text_color(super::super::status::session_kind(card.status).color())
                            .child(div().flex_shrink_0().child(session_status_icon_sized(
                                id,
                                card.status,
                                ICON_MD,
                                cx,
                            )))
                            .child(div().min_w_0().truncate().child(card.status)),
                    ),
            )
            .children(card.roots.iter().cloned().map(|root| {
                div()
                    .truncate()
                    .text_size(rpx(TEXT_MICRO))
                    .line_height(rpx(SESSION_META_LINE_H))
                    .font_family(crate::fonts::UI_FAMILY)
                    .text_color(c::FG_DIM())
                    .child(root)
            }));
        Some(
            div()
                .id(("sidebar-session-exit", id.raw()))
                .debug_selector(move || format!("sidebar-session-exit-{}", id.raw()))
                .flex()
                .items_start()
                .h_auto()
                .min_h(rpx(SESSION_LIST_CARD_H))
                .px(rpx(SPACE_XL))
                .py(rpx(SPACE_LG))
                .rounded(rpx(RADIUS_GROUP))
                .bg(if card.selected {
                    c::alpha(c::FG(), 0.14)
                } else {
                    c::alpha(c::BG(), 0.0)
                })
                .child(content)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MotionProbe {
        sidebar: Entity<Sidebar>,
        groups: Vec<(String, Vec<SessionId>)>,
    }
    impl Render for MotionProbe {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let groups = self.groups.clone();
            self.sidebar.update(cx, |sidebar, cx| {
                sidebar.sync_session_motion(&groups, true, window, cx);
                let mut displayed = groups;
                sidebar.session_motion_groups(&mut displayed);
                let mut list = div().w_full().flex().flex_col().child(
                    div()
                        .debug_selector(|| "motion-anchor".into())
                        .h(gpui::px(20.)),
                );
                for (_, ids) in displayed {
                    for id in ids {
                        let row = sidebar.exiting_session_row(id, cx).unwrap_or_else(|| {
                            div()
                                .debug_selector(move || format!("motion-live-{}", id.raw()))
                                .w_full()
                                .h(gpui::px(80.))
                                .into_any_element()
                        });
                        list = list.child(sidebar.session_motion_row(id, row));
                    }
                }
                list.child(
                    div()
                        .debug_selector(|| "motion-following".into())
                        .h(gpui::px(20.)),
                )
            })
        }
    }

    #[gpui::test]
    fn session_presence_tracks_lifecycle_and_reduced_motion(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_reduce_motion(false);
        });
        let (probe, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            let first = runtime.read(cx).registry.clone().update(cx, |registry, _| {
                registry.insert_meta("demo".into(), "/demo".into(), Agent::Codex)
            });
            let sidebar = cx.new(|cx| Sidebar::new(runtime, window, cx));
            MotionProbe {
                sidebar,
                groups: vec![("Idle".into(), vec![first])],
            }
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let anchor = cx.debug_bounds("motion-anchor").unwrap();
        let initial = cx.debug_bounds("motion-following").unwrap();
        assert_eq!(initial.top(), anchor.bottom() + gpui::px(80.));
        let second = cx.update(|_, cx| {
            let sidebar = probe.read(cx).sidebar.clone();
            let registry = sidebar.read(cx).runtime.read(cx).registry.clone();
            let id = registry.update(cx, |registry, _| {
                registry.insert_meta("demo".into(), "/demo".into(), Agent::Claude)
            });
            probe.update(cx, |probe, cx| {
                probe.groups[0].1.push(id);
                cx.notify();
            });
            id
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let opening = cx.debug_bounds("motion-following").unwrap().top();
        assert!(opening >= initial.top() && opening < initial.top() + gpui::px(80.));
        cx.update(|window, cx| {
            let sidebar = probe.read(cx).sidebar.clone();
            sidebar.update(cx, |sidebar, _| {
                let transition = sidebar.session_motion.transitions.get_mut(&second).unwrap();
                assert!(transition.exiting.is_none());
                transition.started = Instant::now() - DURATION / 2;
            });
            probe.update(cx, |_, cx| cx.notify());
            let _ = window.draw(cx);
        });
        let midway = cx.debug_bounds("motion-following").unwrap().top();
        assert!(midway > opening && midway < initial.top() + gpui::px(80.));
        cx.update(|window, cx| {
            let sidebar = probe.read(cx).sidebar.clone();
            sidebar.update(cx, |sidebar, _| {
                sidebar
                    .session_motion
                    .transitions
                    .get_mut(&second)
                    .unwrap()
                    .started = Instant::now() - DURATION
            });
            probe.update(cx, |_, cx| cx.notify());
            let _ = window.draw(cx);
        });
        assert_eq!(
            cx.debug_bounds("motion-following").unwrap().top(),
            initial.top() + gpui::px(80.)
        );
        cx.update(|window, cx| {
            let sidebar = probe.read(cx).sidebar.clone();
            // A change of group or title with unchanged inventory never restarts motion.
            probe.update(cx, |probe, cx| {
                probe.groups[0].0 = "Working".into();
                cx.notify();
            });
            let _ = window.draw(cx);
            assert!(sidebar.read(cx).session_motion.transitions.is_empty());
            let registry = sidebar.read(cx).runtime.read(cx).registry.clone();
            registry.update(cx, |registry, _| {
                registry.remove(second);
            });
            probe.update(cx, |probe, cx| {
                probe.groups[0].1.retain(|id| *id != second);
                cx.notify();
            });
            let _ = window.draw(cx);
            let transition = sidebar
                .read(cx)
                .session_motion
                .transitions
                .get(&second)
                .unwrap();
            assert!(transition.exiting.is_some());
            assert!(transition.height.get() > 0.);
            assert!(registry.read(cx).meta(second).is_none());
        });
        assert!(cx.debug_bounds("sidebar-session-exit-2").is_some());
        assert!(cx.debug_bounds("motion-live-2").is_none());
        cx.update(|window, cx| {
            let sidebar = probe.read(cx).sidebar.clone();
            sidebar.update(cx, |sidebar, _| {
                sidebar
                    .session_motion
                    .transitions
                    .get_mut(&second)
                    .unwrap()
                    .started = Instant::now() - DURATION / 2
            });
            probe.update(cx, |_, cx| cx.notify());
            let _ = window.draw(cx);
        });
        let closing = cx.debug_bounds("motion-following").unwrap().top();
        let ghost_height = cx.update(|_, cx| {
            let sidebar = probe.read(cx).sidebar.read(cx);
            sidebar.session_motion.transitions[&second].height.get()
        });
        assert!(closing > initial.top() && closing < initial.top() + gpui::px(ghost_height));
        cx.update(|window, cx| {
            cx.set_reduce_motion(true);
            probe.update(cx, |_, cx| cx.notify());
            let _ = window.draw(cx);
            let sidebar = probe.read(cx).sidebar.read(cx);
            assert!(sidebar.session_motion.transitions.is_empty());
        });
        assert!(cx.debug_bounds("sidebar-session-exit-2").is_none());
        assert_eq!(
            cx.debug_bounds("motion-following").unwrap().top(),
            initial.top()
        );
        cx.update(|window, cx| {
            let sidebar = probe.read(cx).sidebar.clone();
            let registry = sidebar.read(cx).runtime.read(cx).registry.clone();
            let id = registry.update(cx, |registry, _| {
                registry.insert_meta("demo".into(), "/demo".into(), Agent::Codex)
            });
            probe.update(cx, |probe, cx| {
                probe.groups[0].1.push(id);
                cx.notify();
            });
            let _ = window.draw(cx);
            assert!(sidebar.read(cx).session_motion.transitions.is_empty());
        });
        assert_eq!(
            cx.debug_bounds("motion-following").unwrap().top(),
            initial.top() + gpui::px(80.)
        );
    }
    #[gpui::test]
    fn real_sidebar_animates_inventory_and_done_closes_immediately(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![grove_core::storage::Project {
                    name: "Example".into(),
                    path: "/grove-single-sidebar-example".into(),
                    scripts: Default::default(),
                    archived: false,
                    worktree_dir: None,
                }],
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_reduce_motion(false);
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            runtime.read(cx).registry.clone().update(cx, |registry, _| {
                registry.insert_meta(
                    "Example".into(),
                    "/grove-single-sidebar-example".into(),
                    Agent::Codex,
                );
            });
            Sidebar::new(runtime, window, cx)
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("session-1").is_some());
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar
            .session_motion
            .transitions
            .is_empty()));
        let id = cx.update(|_, cx| {
            let registry = sidebar.read(cx).runtime.read(cx).registry.clone();
            registry.update(cx, |registry, cx| {
                let id = registry.insert_meta(
                    "Example".into(),
                    "/grove-single-sidebar-example".into(),
                    Agent::Claude,
                );
                cx.notify();
                id
            })
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar
            .session_motion
            .transitions
            .get(&id)
            .is_some_and(|state| state.exiting.is_none())));
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar
                    .session_motion
                    .transitions
                    .get_mut(&id)
                    .unwrap()
                    .started = Instant::now() - DURATION;
                cx.notify();
            });
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("session-2").is_some());
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.act(Action::FinishSession(id), window, cx)
            });
            let _ = window.draw(cx);
            let sidebar = sidebar.read(cx);
            assert!(sidebar
                .runtime
                .read(cx)
                .registry
                .read(cx)
                .meta(id)
                .is_none());
            assert!(sidebar.pending_close.is_none());
            assert!(sidebar
                .session_motion
                .transitions
                .get(&id)
                .is_some_and(|state| state.exiting.is_some()));
        });
        assert!(cx.debug_bounds("sidebar-session-exit-2").is_some());
        assert!(cx.debug_bounds("close-session-2").is_none());
    }
}
