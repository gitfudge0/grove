//! Deliberately opened project context without changing the selected terminal.
use super::*;

const HOVER_DISMISS_DELAY: Duration = Duration::from_millis(MOTION_SLOW_MS);

impl Sidebar {
    pub(crate) fn set_project_hover_blocked(&mut self, blocked: bool, cx: &mut Context<Self>) {
        if self.project_flyout_hover_blocked != blocked {
            self.project_flyout_hover_blocked = blocked;
            if blocked {
                self.dismiss_project_context(cx);
            } else {
                self.project_flyout_suppressed = None;
            }
        }
    }

    fn project_hover_allowed(&self, cx: &App) -> bool {
        self.is_collapsed(cx)
            && self.mode == ViewMode::Project
            && !self.is_zen()
            && self.navigation_available()
            && !self.project_flyout_hover_blocked
            && !self
                .settings_panel
                .as_ref()
                .is_some_and(|panel| panel.read(cx).is_open())
            && !self
                .workspace_selector
                .as_ref()
                .is_some_and(|selector| selector.read(cx).is_open())
    }

    fn project_group_rect(&self, target: &SidebarContext) -> Option<gpui::Bounds<gpui::Pixels>> {
        match target {
            SidebarContext::MultiProject => Some(self.multi_project_group_bounds.get()),
        }
    }

    fn pointer_in_project_context(&self, point: gpui::Point<gpui::Pixels>) -> bool {
        let Some(group) = self
            .project_flyout
            .as_ref()
            .and_then(|target| self.project_group_rect(target))
        else {
            return false;
        };
        let popup = self.project_flyout_popup_bounds.get();
        group.contains(&point)
            || popup.contains(&point)
            || (point.x >= group.right()
                && point.x <= popup.left()
                && point.y >= group.top().max(popup.top())
                && point.y <= group.bottom().min(popup.bottom()))
    }

    pub(super) fn track_project_flyout_pointer(
        &mut self,
        point: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .project_flyout_suppressed
            .as_ref()
            .is_some_and(|target| {
                self.project_group_rect(target)
                    .is_none_or(|bounds| !bounds.contains(&point))
            })
        {
            self.project_flyout_suppressed = None;
        }
        if self.project_flyout.as_ref() == Some(&SidebarContext::MultiProject)
            && self.confirmation_open()
        {
            // Keep the close dialog's owning context mounted until the decision resolves.
            self.project_flyout_hide_task = None;
            return;
        }
        if !self.project_flyout_hover_open || self.project_flyout.is_none() {
            return;
        }
        if self.pointer_in_project_context(point) {
            self.project_flyout_hide_task = None;
        } else if self.project_flyout_hide_task.is_none() {
            let path = self.project_flyout.clone();
            self.project_flyout_hide_task = Some(cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(HOVER_DISMISS_DELAY).await;
                let _ = this.update_in(cx, |this, window, cx| {
                    if this.project_flyout == path
                        && this.project_flyout_hover_open
                        && (!this.project_hover_allowed(cx)
                            || !this.pointer_in_project_context(window.mouse_position()))
                    {
                        this.dismiss_project_context(cx);
                        this.track_project_flyout_pointer(window.mouse_position(), window, cx);
                    }
                    this.project_flyout_hide_task = None;
                });
            }));
        }
    }

    pub(crate) fn dismiss_project_context(&mut self, cx: &mut Context<Self>) {
        self.project_flyout_hide_task = None;
        self.project_flyout_hover_open = false;
        self.project_flyout_launch_path = None;
        self.project_flyout_launch_error = None;
        if let Some(path) = self.project_flyout.take() {
            self.project_flyout_suppressed = Some(path);
            cx.notify();
        }
    }
    pub(super) fn collapse_project_launch_options(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(path) = self.project_flyout_launch_path.take() else {
            return false;
        };
        self.project_flyout_launch_error = None;
        if let Some(focus) = self.project_flyout_plus_focus.get(&path).cloned() {
            cx.defer_in(window, move |_, window, cx| focus.focus(window, cx));
        }
        cx.notify();
        true
    }

    pub(super) fn close_project_flyout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let return_focus = self
            .project_flyout
            .as_ref()
            .map(|target| match target {
                SidebarContext::MultiProject => &self.multi_project_focus,
            })
            .cloned();
        self.dismiss_project_context(cx);
        self.track_project_flyout_pointer(window.mouse_position(), window, cx);
        if let Some(focus) = return_focus {
            cx.defer_in(window, move |_, window, cx| focus.focus(window, cx));
        }
    }
}
