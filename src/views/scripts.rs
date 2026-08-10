//! The shared spawn path for a worktree's `run` lifecycle script.
//!
//! Both the header ▶ / palette (`Workspace::spawn_wt_script`) and the sidebar
//! row's ▶ (`Sidebar::dispatch`, `RowAction::RunScript`) route through here so
//! there is exactly one place that turns a worktree path + script string into
//! a spawned panel shell.

use gpui::AppContext as _;

use crate::entities::session_registry::SessionRegistry;
use crate::entities::terminal_session::TerminalSession;
use crate::entities::toast::ToastState;
use crate::entities::workspace_state::{PtyPane, WorkspaceState};

/// Spawn a one-shot script as a panel shell rooted at the worktree, and
/// focus it. Identical to `Sidebar`/`Workspace`'s `spawn_wt_shell` except the
/// PTY runs `script` instead of an interactive login shell — the palette
/// strip's lifecycle-script rows (`src/views/modals/launcher.rs`,
/// `row_actions`) route through here since gpui has no `spawn_script_session`
/// equivalent to the iced original.
pub(crate) fn spawn_wt_script(
    registry: &gpui::Entity<SessionRegistry>,
    state: &gpui::Entity<WorkspaceState>,
    toast: Option<&gpui::Entity<ToastState>>,
    wt_path: &str,
    script: &str,
    cx: &mut gpui::App,
) {
    let (id, label) = registry.update(cx, |r, _| (r.next_home_id(), r.next_wt_label()));
    let session = cx.new(|cx| TerminalSession::spawn_script(script, wt_path, cx));
    if let Some(err) = session.read(cx).spawn_error().map(str::to_string) {
        if let Some(toast) = toast {
            toast.update(cx, |t, cx| {
                t.set_error(format!("terminal failed: {err}"), cx);
            });
        }
        return;
    }
    let meta = crate::entities::session_registry::SessionMeta {
        id,
        project: String::new(),
        wt_path: wt_path.to_string(),
        agent: grove_core::agent::Agent::Terminal,
        label,
        spawned_at: std::time::Instant::now(),
        attention: None,
        // Home terminals and panel shells are always native.
        tmux: false,
        tmux_name: None,
    };
    registry.update(cx, |r, cx| {
        r.push_wt_shell(wt_path, meta, Some(session));
        cx.notify();
    });
    state.update(cx, |s, cx| {
        s.focus_pane(PtyPane::Panel);
        cx.notify();
    });
}
