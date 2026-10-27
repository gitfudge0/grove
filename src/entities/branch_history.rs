//! Immutable HEAD snapshots; background results belong to an entity and request generation.
use gpui::{AppContext as _, Context};
use grove_core::{
    diff::{FileChange, Patch},
    history::{self, BranchHistory, CommitDetails},
};

/// Local to this history viewer; independent of the working-tree diff setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HistoryDiffMode {
    #[default]
    Split,
    Unified,
}

#[derive(Clone)]
pub struct BranchHistoryState {
    pub checkout: String,
    pub snapshot: Option<BranchHistory>,
    pub loading: bool,
    pub error: Option<String>,
    pub selected: Option<String>,
    pub details: Option<CommitDetails>,
    pub detail_error: Option<String>,
    pub detail_loading: bool,
    pub selected_file: Option<String>,
    pub patch: Option<Patch>,
    pub patch_error: Option<String>,
    pub patch_loading: bool,
    pub expanded: bool,
    pub diff_mode: HistoryDiffMode,
    pub pan_x: f32,
    /// User-selected file register width in logical pixels for this history viewer.
    pub file_width: Option<f32>,
    /// Pointer origin in physical pixels and the logical width at drag start.
    pub file_resize_drag: Option<(f32, f32)>,
    pub metadata_expanded: bool,
    pub limit: usize,
    generation: u64,
    selection_generation: u64,
    patch_generation: u64,
}
impl BranchHistoryState {
    pub fn new(checkout: String, cx: &mut Context<Self>) -> Self {
        Self::with_limit(checkout, 100, cx)
    }
    pub fn with_limit(checkout: String, limit: usize, cx: &mut Context<Self>) -> Self {
        let mut this = Self::initial(checkout, limit);
        this.reload(cx);
        this
    }
    fn initial(checkout: String, limit: usize) -> Self {
        Self {
            checkout,
            snapshot: None,
            loading: false,
            error: None,
            selected: None,
            details: None,
            detail_error: None,
            detail_loading: false,
            selected_file: None,
            patch: None,
            patch_error: None,
            patch_loading: false,
            expanded: false,
            diff_mode: HistoryDiffMode::default(),
            pan_x: 0.0,
            file_width: None,
            file_resize_drag: None,
            metadata_expanded: false,
            limit,
            generation: 0,
            selection_generation: 0,
            patch_generation: 0,
        }
    }
    pub fn snapshot_generation(&self) -> u64 {
        self.generation
    }
    /// Dismiss only the transient preview and invalidate any in-flight detail read.
    pub fn dismiss_preview(&mut self, cx: &mut Context<Self>) {
        self.selection_generation += 1;
        self.patch_generation += 1;
        self.pan_x = 0.0;
        self.metadata_expanded = false;
        self.selected = None;
        self.details = None;
        self.detail_error = None;
        self.detail_loading = false;
        self.selected_file = None;
        self.patch = None;
        self.patch_error = None;
        self.patch_loading = false;
        cx.notify();
    }
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.generation += 1;
        self.selection_generation += 1;
        self.patch_generation += 1;
        self.pan_x = 0.0;
        self.metadata_expanded = false;
        let generation = self.generation;
        self.loading = true;
        self.error = None;
        self.detail_error = None;
        self.patch_error = None;
        self.selected = None;
        self.details = None;
        self.expanded = false;
        self.selected_file = None;
        self.patch = None;
        self.detail_loading = false;
        self.patch_loading = false;
        let checkout = self.checkout.clone();
        let limit = self.limit;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { history::load(&checkout, limit) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(snapshot) => this.snapshot = Some(snapshot),
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    pub fn more(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.limit = (self.limit + 100).min(500);
        self.reload(cx);
    }
    pub fn select(&mut self, id: String, cx: &mut Context<Self>) {
        if self.selected.as_ref() == Some(&id) {
            return;
        }
        self.selection_generation += 1;
        self.patch_generation += 1;
        self.pan_x = 0.0;
        self.metadata_expanded = false;
        let generation = self.selection_generation;
        self.selected = Some(id.clone());
        self.details = None;
        self.detail_error = None;
        self.selected_file = None;
        self.patch = None;
        self.patch_error = None;
        self.patch_loading = false;
        self.detail_loading = true;
        self.expanded = false;
        let checkout = self.checkout.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { history::details(&checkout, &id) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.finish_details(generation, result, cx);
            });
        })
        .detach();
        cx.notify();
    }
    /// Enter the commit inspector, selecting a useful initial file as soon as details exist.
    pub fn expand(&mut self, cx: &mut Context<Self>) {
        self.expanded = true;
        self.select_first_file(cx);
        cx.notify();
    }
    /// Keep commit and file selection so returning to the inspector preserves context.
    pub fn collapse(&mut self, cx: &mut Context<Self>) {
        self.expanded = false;
        self.file_resize_drag = None;
        cx.notify();
    }
    pub fn set_diff_mode(&mut self, mode: HistoryDiffMode, cx: &mut Context<Self>) {
        self.diff_mode = mode;
        cx.notify();
    }
    pub fn toggle_metadata(&mut self, cx: &mut Context<Self>) {
        self.metadata_expanded = !self.metadata_expanded;
        cx.notify();
    }
    fn select_first_file(&mut self, cx: &mut Context<Self>) {
        if self.expanded && self.selected_file.is_none() {
            if let Some(file) = self
                .details
                .as_ref()
                .and_then(|details| details.files.first())
                .cloned()
            {
                self.file(file, cx);
            }
        }
    }
    fn finish_details(
        &mut self,
        generation: u64,
        result: Result<CommitDetails, String>,
        cx: &mut Context<Self>,
    ) {
        if self.selection_generation != generation {
            return;
        }
        self.detail_loading = false;
        match result {
            Ok(details) => {
                self.details = Some(details);
                self.select_first_file(cx);
            }
            Err(error) => self.detail_error = Some(error),
        }
        cx.notify();
    }
    fn finish_patch(
        &mut self,
        selection_generation: u64,
        patch_generation: u64,
        path: &str,
        result: Result<Patch, String>,
        cx: &mut Context<Self>,
    ) {
        if self.selection_generation != selection_generation
            || self.patch_generation != patch_generation
            || self.selected_file.as_deref() != Some(path)
        {
            return;
        }
        self.patch_loading = false;
        match result {
            Ok(patch) => self.patch = Some(patch),
            Err(error) => self.patch_error = Some(error),
        }
        cx.notify();
    }
    pub fn file(&mut self, file: FileChange, cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let generation = self.selection_generation;
        self.patch_generation += 1;
        self.pan_x = 0.0;
        let patch_generation = self.patch_generation;
        let checkout = self.checkout.clone();
        let path = file.path.clone();
        self.selected_file = Some(path.clone());
        self.patch = None;
        self.patch_error = None;
        self.patch_loading = true;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { history::file_patch(&checkout, &id, &file) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.finish_patch(generation, patch_generation, &path, result, cx);
            });
        })
        .detach();
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grove_core::diff::Status;
    use std::process::Command;

    fn checkout() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.name", "History Test"]);
        git(&["config", "user.email", "history@example.invalid"]);
        fs_err::write(dir.path().join("alpha.txt"), "alpha\n").unwrap();
        fs_err::write(dir.path().join("beta.txt"), "beta\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "Two changed files"]);
        let id = git(&["rev-parse", "HEAD"]);
        (dir, id)
    }

    #[gpui::test]
    fn expand_while_details_pending_loads_first_file_and_preserves_context(
        cx: &mut gpui::TestAppContext,
    ) {
        let (dir, id) = checkout();
        let state =
            cx.new(|_| BranchHistoryState::initial(dir.path().to_string_lossy().into_owned(), 100));
        state.update(cx, |state, cx| {
            assert_eq!(state.diff_mode, HistoryDiffMode::Split);
            assert!(!state.metadata_expanded);
            state.select(id.clone(), cx);
            state.expand(cx);
            assert!(state.detail_loading);
            assert!(state.selected_file.is_none());
        });
        cx.run_until_parked();
        state.update(cx, |state, cx| {
            assert!(state.expanded);
            assert_eq!(state.selected.as_deref(), Some(id.as_str()));
            assert_eq!(state.selected_file.as_deref(), Some("alpha.txt"));
            assert!(
                matches!(state.patch, Some(Patch::Text { ref hunks, .. }) if !hunks.is_empty())
            );
            let beta = state
                .details
                .as_ref()
                .unwrap()
                .files
                .iter()
                .find(|file| file.path == "beta.txt")
                .unwrap()
                .clone();
            state.file(beta, cx);
            state.collapse(cx);
            assert!(!state.expanded);
            assert_eq!(state.selected_file.as_deref(), Some("beta.txt"));
            state.expand(cx);
            assert_eq!(state.selected_file.as_deref(), Some("beta.txt"));
            state.set_diff_mode(HistoryDiffMode::Unified, cx);
            state.toggle_metadata(cx);
            assert_eq!(state.diff_mode, HistoryDiffMode::Unified);
            assert!(state.metadata_expanded);
            state.toggle_metadata(cx);
            assert!(!state.metadata_expanded);
        });
        cx.run_until_parked();
        state.read_with(cx, |state, _| {
            assert!(matches!(state.patch, Some(Patch::Text {ref hunks, ..}) if !hunks.is_empty()));
        });
    }

    #[gpui::test]
    fn expand_after_details_loaded_selects_first_file_immediately(cx: &mut gpui::TestAppContext) {
        let (dir, id) = checkout();
        let state =
            cx.new(|_| BranchHistoryState::initial(dir.path().to_string_lossy().into_owned(), 100));
        state.update(cx, |state, cx| state.select(id, cx));
        cx.run_until_parked();
        state.update(cx, |state, cx| {
            assert!(state.details.is_some());
            assert!(state.selected_file.is_none());
            state.expand(cx);
            assert_eq!(state.selected_file.as_deref(), Some("alpha.txt"));
            assert!(state.patch_loading);
        });
        cx.run_until_parked();
        state.read_with(cx, |state, _| assert!(state.patch.is_some()));
    }

    #[gpui::test]
    fn stale_a_b_a_patch_cannot_replace_latest_request_or_loading_state(
        cx: &mut gpui::TestAppContext,
    ) {
        let state = cx.new(|_| BranchHistoryState::initial(String::new(), 100));
        state.update(cx, |state, cx| {
            // Drive the same completion boundary used by background Git reads,
            // deliberately finishing the first A only after a newer A exists.
            state.selected = Some("a".repeat(40));
            let selection = state.selection_generation;
            let file = |path: &str| FileChange {
                path: path.into(),
                status: Status::Modified,
                added: 1,
                removed: 0,
            };
            state.file(file("a.txt"), cx);
            let old_a = state.patch_generation;
            state.file(file("b.txt"), cx);
            state.file(file("a.txt"), cx);
            let current_a = state.patch_generation;
            state.finish_patch(selection, old_a, "a.txt", Ok(Patch::Binary), cx);
            assert!(state.patch.is_none());
            assert!(state.patch_loading);
            state.finish_patch(selection, old_a, "a.txt", Err("stale failure".into()), cx);
            assert!(state.patch_error.is_none());
            assert!(state.patch_loading);
            state.finish_patch(
                selection,
                current_a,
                "a.txt",
                Ok(Patch::Text {
                    hunks: vec![],
                    no_newline_at_eof: false,
                }),
                cx,
            );
            assert!(matches!(state.patch, Some(Patch::Text { .. })));
            assert!(!state.patch_loading);
            state.pan_x = 100.0;
            state.select("b".repeat(40), cx);
            assert_eq!(state.pan_x, 0.0);
            state.finish_patch(selection, current_a, "a.txt", Ok(Patch::Binary), cx);
            assert!(state.patch.is_none());
            state.file(file("a.txt"), cx);
            let pending_selection = state.selection_generation;
            let pending_patch = state.patch_generation;
            state.pan_x = 100.0;
            state.reload(cx);
            assert_eq!(state.pan_x, 0.0);
            state.finish_patch(
                pending_selection,
                pending_patch,
                "a.txt",
                Ok(Patch::Binary),
                cx,
            );
            assert!(state.patch.is_none());
            state.dismiss_preview(cx);
            state.finish_patch(selection, current_a, "a.txt", Ok(Patch::Binary), cx);
            assert!(state.patch.is_none());
            assert!(state.selected_file.is_none());
        });
    }
}
