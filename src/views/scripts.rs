//! The shared spawn path for a worktree's `run` lifecycle script; both the header/palette and sidebar row ▶ route through here.

use gpui::AppContext as _;

use crate::entities::session_registry::SessionRegistry;
use crate::entities::terminal_session::TerminalSession;
use crate::entities::toast::ToastState;
use crate::entities::workspace_state::{PtyPane, WorkspaceState};

/// Identical to `spawn_wt_shell` except the PTY runs `script` instead of an interactive login shell.
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
        context_roots: Vec::new(),
        label,
        spawned_at: std::time::Instant::now(),
        attention: None,
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
