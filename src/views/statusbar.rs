//! Passive, workspace-scoped status strip. Repaints follow existing entities.
use std::collections::{HashMap, HashSet};

use gpui::{div, prelude::*, Context, Entity, Render, Subscription, Window};

use super::{rpx, sidebar::Sidebar, tokens::*};
use crate::{
    activity::ActivityState,
    entities::{session_registry::SessionId, terminal_session::Backend, toast::ToastKind},
    runtime::Runtime,
    settings::SettingsState,
    theme as c,
};

pub(super) const STATUS_H: f32 = 26.0;
const BACKEND_MIN_W: f32 = 620.0;
const VERSION_MIN_W: f32 = 840.0;
const THEME_MIN_W: f32 = 1080.0;
const WORKSPACE_MAX_W: f32 = 160.0;
const COMPACT_WORKSPACE_MAX_W: f32 = 88.0;

#[derive(Default, Debug, PartialEq, Eq)]
struct Summary {
    active: usize,
    working: usize,
    waiting: usize,
    local: usize,
    tmux: usize,
}

impl Summary {
    fn record(&mut self, alive: bool, state: ActivityState, backend: &Backend) {
        if !alive || state == ActivityState::Exited {
            return;
        }
        self.active += 1;
        self.working += usize::from(state == ActivityState::Working);
        self.waiting += usize::from(state == ActivityState::WaitingForInput);
        match backend {
            Backend::Native => self.local += 1,
            Backend::Tmux { .. } => self.tmux += 1,
        }
    }

    fn backend(&self) -> &'static str {
        match (self.local > 0, self.tmux > 0) {
            (true, true) => "Mixed backend",
            (true, false) => "Local backend",
            (false, true) => "tmux backend",
            (false, false) => "No active backend",
        }
    }

    fn label(&self, compact: bool) -> String {
        if self.active == 0 {
            "No active sessions".into()
        } else if compact {
            if self.waiting > 0 {
                format!("{} active · {} needs you", self.active, self.waiting)
            } else {
                format!("{} active · {} working", self.active, self.working)
            }
        } else {
            format!(
                "{} active · {} working · {} needs you",
                self.active, self.working, self.waiting
            )
        }
    }
}

pub struct Statusbar {
    runtime: Entity<Runtime>,
    sidebar: Entity<Sidebar>,
    _observers: Vec<Subscription>,
    session_observers: HashMap<SessionId, (gpui::EntityId, Subscription)>,
}

impl Statusbar {
    pub fn new(runtime: Entity<Runtime>, sidebar: Entity<Sidebar>, cx: &mut Context<Self>) -> Self {
        let rt = runtime.read(cx);
        let (registry, activity, toast) =
            (rt.registry.clone(), rt.activity.clone(), rt.toast.clone());
        let observers = vec![
            cx.observe(&runtime, |_, _, cx| cx.notify()),
            cx.observe(&sidebar, |_, _, cx| cx.notify()),
            cx.observe(&registry, |_, _, cx| cx.notify()),
            cx.observe(&activity, |_, _, cx| cx.notify()),
            cx.observe(&toast, |_, _, cx| cx.notify()),
            cx.observe_global::<SettingsState>(|_, cx| cx.notify()),
        ];
        Self {
            runtime,
            sidebar,
            _observers: observers,
            session_observers: HashMap::new(),
        }
    }

    fn summary(&mut self, cx: &mut Context<Self>) -> Summary {
        let ids = self.sidebar.read(cx).active_canvas_sessions(cx);
        let wanted: HashSet<_> = ids.iter().map(|(id, _)| *id).collect();
        self.session_observers.retain(|id, _| wanted.contains(id));
        let runtime = self.runtime.read(cx);
        let (registry, activity) = (runtime.registry.clone(), runtime.activity.clone());
        let mut summary = Summary::default();
        for (id, home) in ids {
            let registry = registry.read(cx);
            let session = if home {
                registry
                    .home_terminals()
                    .iter()
                    .position(|meta| meta.id == id)
                    .and_then(|index| registry.home_terminal(index))
                    .cloned()
            } else {
                registry.session(id).cloned()
            };
            let Some(session) = session else { continue };
            if self
                .session_observers
                .get(&id)
                .is_some_and(|(entity, _)| *entity != session.entity_id())
            {
                self.session_observers.remove(&id);
            }
            self.session_observers.entry(id).or_insert_with(|| {
                // Repaint only on lifecycle changes, never for every PTY output chunk.
                let mut previous = {
                    let term = session.read(cx);
                    (
                        term.has_exited(),
                        term.is_pending_attach(),
                        term.spawn_error().is_some(),
                    )
                };
                let observer = cx.observe(&session, move |_, session, cx| {
                    let term = session.read(cx);
                    let next = (
                        term.has_exited(),
                        term.is_pending_attach(),
                        term.spawn_error().is_some(),
                    );
                    if next != previous {
                        previous = next;
                        cx.notify();
                    }
                });
                (session.entity_id(), observer)
            });
            let term = session.read(cx);
            summary.record(
                !term.has_exited() && !term.is_pending_attach() && term.spawn_error().is_none(),
                if home {
                    ActivityState::Idle
                } else {
                    activity.read(cx).state_of(id)
                },
                term.backend(),
            );
        }
        summary
    }
}

impl Render for Statusbar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let summary = self.summary(cx);
        let store = &cx.global::<SettingsState>().store;
        let workspace = store.workspaces.name(store.workspaces.active).to_string();
        let toast = self.runtime.read(cx).toast.read(cx).current().cloned();
        let width = f32::from(window.viewport_size().width)
            / (f32::from(window.rem_size()) / crate::zoom::REM_BASE);
        let compact = width < BACKEND_MIN_W;
        let backend = summary.backend();
        let label = summary.label(compact);
        let accessible = format!("{workspace}. {}. {backend}.", summary.label(false));
        let mut bar = div()
            .id("statusbar")
            .debug_selector(|| "statusbar".into())
            .role(gpui::Role::Status)
            .aria_label(accessible)
            .h(rpx(STATUS_H))
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(rpx(SPACE_3XL))
            .px(rpx(SPACE_2XL))
            .overflow_hidden()
            .border_t_1()
            .border_color(c::BORDER())
            .bg(c::BG_STRIP())
            .text_color(c::FG_DIM())
            .text_size(rpx(TEXT_MICRO))
            .font_family(crate::fonts::MONO_FAMILY);
        if !compact || toast.is_none() {
            bar = bar.child(
                div()
                    .id("status-workspace")
                    .debug_selector(|| "status-workspace".into())
                    .min_w_0()
                    .max_w(rpx(if compact {
                        COMPACT_WORKSPACE_MAX_W
                    } else {
                        WORKSPACE_MAX_W
                    }))
                    .truncate()
                    .child(workspace),
            );
        }
        bar = bar.child(
            div()
                .id("status-sessions")
                .debug_selector(|| "status-sessions".into())
                .min_w_0()
                .truncate()
                .when(toast.is_none() && compact, gpui::Styled::flex_1)
                .text_color(if summary.waiting > 0 {
                    c::AMBER()
                } else {
                    c::FG_DIM()
                })
                .child(label),
        );
        if !compact {
            bar = bar.child(div().flex_shrink_0().child(backend));
        }
        if let Some(toast) = toast {
            bar = bar.child(
                div()
                    .id("status-toast")
                    .debug_selector(|| "status-toast".into())
                    .role(if toast.kind == ToastKind::Error {
                        gpui::Role::Alert
                    } else {
                        gpui::Role::Status
                    })
                    .aria_label(toast.message.clone())
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(if toast.kind == ToastKind::Error {
                        c::FORM_ERROR()
                    } else {
                        c::FG()
                    })
                    .child(toast.message),
            );
        } else {
            bar = bar.child(div().flex_1().min_w_0());
            if width >= THEME_MIN_W {
                bar = bar.child(
                    div()
                        .text_color(c::FG_MUTE())
                        .child(grove_core::theme::current().name.to_string()),
                );
            }
            if width >= VERSION_MIN_W {
                bar = bar.child(
                    div()
                        .flex_shrink_0()
                        .text_color(c::FG_MUTE())
                        .child(concat!("v", env!("CARGO_PKG_VERSION"))),
                );
            }
        }
        bar
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_and_attention_counts_exclude_stopped_and_failed_sessions() {
        let mut summary = Summary::default();
        summary.record(true, ActivityState::Working, &Backend::Native);
        summary.record(
            true,
            ActivityState::WaitingForInput,
            &Backend::Tmux {
                name: "agent".into(),
            },
        );
        summary.record(false, ActivityState::Working, &Backend::Native);
        summary.record(
            true,
            ActivityState::Exited,
            &Backend::Tmux {
                name: "stopped".into(),
            },
        );
        assert_eq!(
            (summary.active, summary.working, summary.waiting),
            (2, 1, 1)
        );
        assert_eq!(summary.backend(), "Mixed backend");
        assert_eq!(summary.label(true), "2 active · 1 needs you");
    }

    #[test]
    fn empty_workspace_does_not_claim_local_or_running_sessions() {
        let summary = Summary::default();
        assert_eq!(summary.backend(), "No active backend");
        assert_eq!(summary.label(false), "No active sessions");
    }
}
