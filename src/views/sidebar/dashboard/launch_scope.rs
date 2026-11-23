//! Dashboard launch scope validation and launch configuration.

use super::{ScopeTarget, SettingsState};
use gpui::App;

pub(super) fn scope_roots(scope: &[ScopeTarget]) -> Vec<grove_core::session_meta::ContextRoot> {
    scope
        .iter()
        .map(|root| grove_core::session_meta::ContextRoot {
            project: root.project_name.clone(),
            wt_path: root.path.clone(),
        })
        .collect()
}

pub(super) fn validate_scope(
    scope: &[ScopeTarget],
    cwd: &str,
    projects: &[(usize, String, String)],
) -> Result<(), String> {
    if !scope.iter().any(|target| target.path == cwd) {
        return Err("Choose a working directory from the selected worktrees.".into());
    }
    let mut paths = std::collections::HashSet::new();
    for target in scope {
        if !projects
            .iter()
            .any(|(idx, _, path)| *idx == target.project && *path == target.project_path)
            || !std::path::Path::new(&target.path).is_dir()
            || !paths.insert(crate::paths::normalize_wt_path(&target.path))
        {
            return Err(format!(
                "{} is no longer available. Review the selected worktrees.",
                target.path
            ));
        }
    }
    Ok(())
}

pub(super) fn dashboard_launch_args(
    provider: grove_core::codex::Provider,
    scope: &[ScopeTarget],
    cwd: &str,
    cx: &App,
) -> Vec<String> {
    let store = &cx.global::<SettingsState>().store;
    let extras = scope
        .iter()
        .filter(|root| root.path != cwd)
        .map(|root| root.path.clone())
        .collect::<Vec<_>>();
    provider
        .agent()
        .multi_root_launch_args_for_backend(
            true,
            store.yolo_enabled,
            store.chrome_enabled.unwrap_or(false),
            &extras,
        )
        .unwrap_or_default()
}
