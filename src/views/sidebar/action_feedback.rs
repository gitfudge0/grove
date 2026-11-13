//! Side-panel receipts are owned by a checkout and an action identity.
use super::*;
use gpui::AnimationExt as _;

pub(super) const DRAW: Duration = crate::views::status::CHECK_DRAW_DURATION;
pub(super) const COLLAPSE: Duration = Duration::from_millis(220);
pub(super) const HOLD: Duration = crate::views::status::CHECK_CONFIRMATION_HOLD;
pub(super) const LIFETIME: Duration =
    Duration::from_millis((DRAW.as_millis() + HOLD.as_millis()) as u64);

#[derive(Clone)]
pub(super) struct Feedback {
    pub token: u64,
    pub label: String,
    pub completed: Option<Instant>,
    pub collapse: bool,
}
impl Feedback {
    pub fn visible(&self, now: Instant) -> bool {
        self.completed.is_none_or(|started| {
            now.saturating_duration_since(started)
                < LIFETIME
                    + if self.collapse {
                        COLLAPSE
                    } else {
                        Duration::ZERO
                    }
        })
    }
}

#[derive(Default)]
pub(super) struct Feedbacks {
    next: u64,
    entries: HashMap<(String, String), Feedback>,
}
impl Feedbacks {
    pub fn begin(&mut self, owner: &str, action: &str, label: &str) -> u64 {
        if matches!(action, "local" | "pull" | "main" | "recovery" | "pr-merge") {
            self.entries
                .retain(|(entry_owner, entry_action), feedback| {
                    entry_owner != owner
                        || feedback.completed.is_none()
                        || !matches!(
                            entry_action.as_str(),
                            "local" | "pull" | "main" | "recovery" | "pr-merge"
                        )
                });
        }
        self.next = self.next.wrapping_add(1);
        self.entries.insert(
            (owner.into(), action.into()),
            Feedback {
                token: self.next,
                label: label.into(),
                completed: None,
                collapse: false,
            },
        );
        self.next
    }
    pub fn get(&self, owner: &str, action: &str) -> Option<Feedback> {
        self.entries
            .get(&(owner.into(), action.into()))
            .filter(|f| f.visible(Instant::now()))
            .cloned()
    }
    pub fn progress(&mut self, owner: &str, action: &str, token: u64, label: &str) {
        if let Some(entry) = self
            .entries
            .get_mut(&(owner.into(), action.into()))
            .filter(|f| f.token == token && f.completed.is_none())
        {
            entry.label = label.into();
        }
    }
    pub fn finish(
        &mut self,
        owner: &str,
        action: &str,
        token: u64,
        label: Option<&str>,
        now: Instant,
    ) {
        let key = (owner.into(), action.into());
        if let Some(entry) = self
            .entries
            .get_mut(&key)
            .filter(|f| f.token == token && f.completed.is_none())
        {
            if let Some(label) = label {
                entry.label = label.into();
                entry.completed = Some(now);
                entry.collapse =
                    matches!(action, "pull" | "main" | "recovery") || label == "Run stopped";
            } else {
                self.entries.remove(&key);
            }
        }
    }
    pub fn expire(&mut self, owner: &str, action: &str, token: u64) {
        let key = (owner.into(), action.into());
        if self.entries.get(&key).is_some_and(|f| f.token == token) {
            self.entries.remove(&key);
        }
    }
}

impl Sidebar {
    pub(super) fn begin_panel_feedback(
        &mut self,
        owner: &str,
        action: &str,
        label: &str,
        cx: &mut Context<Self>,
    ) -> u64 {
        let token = self.checkout_panel.feedback.begin(owner, action, label);
        cx.notify();
        token
    }
    pub(super) fn finish_panel_feedback(
        &mut self,
        owner: &str,
        action: &str,
        token: u64,
        label: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        self.checkout_panel
            .feedback
            .finish(owner, action, token, label, Instant::now());
        cx.notify();
        if label.is_some() {
            let owner = owner.to_owned();
            let action = action.to_owned();
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(LIFETIME).await;
                let collapsing = this
                    .update(cx, |this, cx| {
                        let collapsing = !cx.reduce_motion()
                            && this
                                .checkout_panel
                                .feedback
                                .get(&owner, &action)
                                .is_some_and(|f| f.token == token && f.collapse);
                        if !collapsing {
                            this.checkout_panel.feedback.expire(&owner, &action, token);
                        }
                        cx.notify();
                        collapsing
                    })
                    .unwrap_or(false);
                if collapsing {
                    cx.background_executor().timer(COLLAPSE).await;
                    let _ = this.update(cx, |this, cx| {
                        this.checkout_panel.feedback.expire(&owner, &action, token);
                        cx.notify();
                    });
                }
            })
            .detach();
        }
    }
}

pub(super) fn mark(
    id: impl Into<SharedString>,
    feedback: &Feedback,
    size: f32,
    cx: &App,
) -> AnyElement {
    if let Some(started) = feedback.completed {
        crate::views::status::drawn_check(id, feedback.token, started, size, cx)
    } else {
        crate::views::status::mark(id, crate::views::status::StatusKind::Working, size, cx)
    }
}

/// Retain the disappearing action through its receipt, then move neighbors together.
pub(super) fn retained_row<E: IntoElement + 'static>(
    element: E,
    feedback: Option<Feedback>,
    cx: &App,
) -> AnyElement {
    let Some(feedback) = feedback.filter(|f| f.collapse) else {
        return element.into_any_element();
    };
    let Some(started) = feedback.completed else {
        return element.into_any_element();
    };
    let elapsed = started.elapsed().saturating_sub(LIFETIME);
    if started.elapsed() < LIFETIME {
        return element.into_any_element();
    }
    if cx.reduce_motion() {
        return div().h_0().overflow_hidden().into_any_element();
    }
    let from = (1. - elapsed.as_secs_f32() / COLLAPSE.as_secs_f32()).clamp(0., 1.);
    div()
        .overflow_hidden()
        .child(element)
        .with_animation(
            SharedString::from(format!("panel-feedback-exit-{}", feedback.token)),
            gpui::Animation::new(COLLAPSE.saturating_sub(elapsed)),
            move |el, t| el.h(rpx(ROW_H * from * (1. - t))).opacity(from * (1. - t)),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn receipts_are_owner_scoped_and_stale_completion_and_expiry_are_inert() {
        let mut feedback = Feedbacks::default();
        let old = feedback.begin("/one", "pull", "Pulling…");
        let new = feedback.begin("/one", "pull", "Pulling…");
        feedback.finish("/one", "pull", old, Some("Pulled"), Instant::now());
        assert!(feedback.get("/one", "pull").unwrap().completed.is_none());
        feedback.expire("/one", "pull", old);
        assert_eq!(feedback.get("/one", "pull").unwrap().token, new);
        assert!(feedback.get("/two", "pull").is_none());
        feedback.finish("/one", "pull", new, Some("Pulled"), Instant::now());
        assert_eq!(feedback.get("/one", "pull").unwrap().label, "Pulled");
        let started = feedback.get("/one", "pull").unwrap().completed;
        feedback.finish("/one", "pull", new, Some("Duplicate"), Instant::now());
        assert_eq!(feedback.get("/one", "pull").unwrap().completed, started);
        let failed = feedback.begin("/one", "pull", "Pulling…");
        feedback.finish("/one", "pull", failed, None, Instant::now());
        assert!(feedback.get("/one", "pull").is_none());
    }
    #[test]
    fn new_git_work_supersedes_git_receipts_but_preserves_independent_scripts() {
        let mut feedback = Feedbacks::default();
        let pull = feedback.begin("/one", "pull", "Pulling…");
        feedback.finish("/one", "pull", pull, Some("Pulled"), Instant::now());
        let script = feedback.begin("/one", "script-tests", "Running…");
        feedback.begin("/one", "local", "Committing…");
        assert!(feedback.get("/one", "pull").is_none());
        assert_eq!(feedback.get("/one", "script-tests").unwrap().token, script);
    }

    #[test]
    fn disappearing_receipt_retains_content_only_through_collapse() {
        let now = Instant::now();
        let feedback = Feedback {
            token: 1,
            label: "Pulled".into(),
            completed: Some(now),
            collapse: true,
        };
        assert!(feedback.visible(now + LIFETIME));
        assert!(feedback.visible(
            (now + LIFETIME + COLLAPSE)
                .checked_sub(Duration::from_millis(1))
                .unwrap()
        ));
        assert!(!feedback.visible(now + LIFETIME + COLLAPSE));
    }

    #[test]
    fn receipt_holds_for_three_seconds_after_the_draw() {
        let now = Instant::now();
        let feedback = Feedback {
            token: 1,
            label: "Done".into(),
            completed: Some(now),
            collapse: false,
        };
        assert!(feedback.visible(
            (now + DRAW + HOLD)
                .checked_sub(Duration::from_millis(1))
                .unwrap()
        ));
        assert!(!feedback.visible(now + DRAW + HOLD));
    }
}
