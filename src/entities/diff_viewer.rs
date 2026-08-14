//! The diff viewer's gpui-side state, loaded off the main thread. All diff arithmetic lives in `grove_core::diff`.

use std::collections::HashMap;
use std::time::{Instant, SystemTime};

use gpui::{AppContext as _, Context};
use grove_core::diff::{self, FileChange, Patch};
use grove_core::highlight;
use grove_core::render_rows::{self, SplitRenderRow, UnifiedRenderRow};
use grove_core::storage::DiffMode;

/// Matched to `ProjectTree`'s `GIT_POLL_INTERVAL` so the file list refreshes on the same rhythm as the git-state chips it's opened from.
const LIVE_REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// Session-only, scoped per worktree — switching worktrees does not carry an expansion set over.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FileListStyle {
    #[default]
    Flat,
    Tree,
}

/// One cached patch, keyed by the file's mtime at load time so a later poll can tell a stale entry from a fresh one.
#[derive(Clone, Debug)]
pub struct CachedPatch {
    pub patch: Patch,
    pub mtime: Option<SystemTime>,
    /// `Rc`, not `Arc`: the row vectors never cross a thread boundary.
    pub unified: std::rc::Rc<Vec<UnifiedRenderRow>>,
    pub split: std::rc::Rc<Vec<SplitRenderRow>>,
    /// The horizontal scroll extent's reference row; split has no counterpart since every row already has a definite pixel width.
    pub unified_widest: usize,
    /// Baked here since measuring pixel width needs a `Window` and can't happen off-thread.
    pub split_old_widest_text: String,
    pub split_new_widest_text: String,
}

/// Constructed fresh each time the modal opens rather than kept warm, so a stale file list can never leak into a different worktree's session.
pub struct DiffViewerState {
    pub wt_path: String,
    pub branch: Option<String>,
    pub files: Vec<FileChange>,
    /// Follows the *path*, not an index, so a file that stops changing shows "No longer changed" in place instead of the selection snapping elsewhere.
    pub selected_path: Option<String>,
    pub mode: DiffMode,
    pub list_style: FileListStyle,
    /// Directories default to expanded; this holds only the ones explicitly collapsed.
    pub tree_expanded: std::collections::HashSet<String>,
    pub patch_cache: HashMap<String, CachedPatch>,
    pub loading: bool,
    /// Throttle for [`Self::maybe_refresh_live`], on [`LIVE_REFRESH_INTERVAL`]'s cadence rather than firing every render.
    last_live_refresh: Option<Instant>,
    /// Guards against a second render frame overlapping an in-flight refresh.
    live_refresh_inflight: bool,
    /// True once Enter has moved keyboard focus from the file list to the scrolling body.
    pub body_focused: bool,
    /// Shared [`gpui::UniformListScrollHandle`]: unified and split are both `uniform_list`s now, so there is only one scroll target.
    pub body_scroll: gpui::UniformListScrollHandle,
    /// Shared by both halves so a line and its counterpart stay column-aligned; clamped on read, not write, so it survives a file switch.
    pub pan_x: f32,
}

impl DiffViewerState {
    /// Opens the viewer for `wt_path`: seeds empty state, then kicks off the
    /// background load of the file list (and the branch name alongside it).
    /// `mode` comes from `SettingsState.store.diff_mode` — the caller reads
    /// the global, this entity does not.
    pub fn new(wt_path: String, mode: DiffMode, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            wt_path,
            branch: None,
            files: Vec::new(),
            selected_path: None,
            mode,
            list_style: FileListStyle::Flat,
            tree_expanded: std::collections::HashSet::new(),
            patch_cache: HashMap::new(),
            loading: true,
            last_live_refresh: None,
            live_refresh_inflight: false,
            body_focused: false,
            body_scroll: gpui::UniformListScrollHandle::new(),
            pan_x: 0.0,
        };
        this.load_files(cx);
        this
    }

    /// Kick off `changed_files` on the background executor; on completion,
    /// select the first file (if any) and start loading its patch.
    pub fn load_files(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        cx.notify();
        let wt_path = self.wt_path.clone();
        cx.spawn(async move |this, cx| {
            let (files, branch) = cx
                .background_spawn(async move {
                    let files = diff::changed_files(&wt_path);
                    let branch = grove_core::git::current_branch(&wt_path);
                    (files, branch)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                this.files = files;
                this.branch = Some(branch).filter(|b| !b.is_empty());
                if this.selected_path.is_none() {
                    this.selected_path = this.files.first().map(|f| f.path.clone());
                }
                if let Some(path) = this.selected_path.clone() {
                    this.load_patch(&path, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Rides `ProjectTree`'s own poll cadence rather than a second timer; self-throttled and guarded against overlap.
    pub fn maybe_refresh_live(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        let due = self
            .last_live_refresh
            .is_none_or(|t| now.duration_since(t) >= LIVE_REFRESH_INTERVAL);
        if !due || self.live_refresh_inflight {
            return;
        }
        self.last_live_refresh = Some(now);
        self.live_refresh_inflight = true;

        let wt_path = self.wt_path.clone();
        // Only the selected file's mtime is worth a fresh `stat`; other cached patches stay valid or get evicted.
        let watched_path = self.selected_path.clone();
        cx.spawn(async move |this, cx| {
            let (files, fresh_mtime) = cx
                .background_spawn(async move {
                    let files = diff::changed_files(&wt_path);
                    let fresh_mtime = watched_path.and_then(|path| {
                        fs_err::metadata(std::path::Path::new(&wt_path).join(&path))
                            .ok()
                            .and_then(|m| m.modified().ok())
                    });
                    (files, fresh_mtime)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.live_refresh_inflight = false;
                this.apply_live_update(files, fresh_mtime, cx);
            });
        })
        .detach();
    }

    /// The selected file's patch reloads only if its mtime changed — an open, untouched file is never re-diffed every tick.
    fn apply_live_update(
        &mut self,
        new_files: Vec<FileChange>,
        fresh_mtime: Option<SystemTime>,
        cx: &mut Context<Self>,
    ) {
        let prev_selected = self.selected_path.clone();
        let reconciled = diff::reconcile(&self.files, &new_files, prev_selected.as_deref());
        for path in &reconciled.evicted {
            self.patch_cache.remove(path);
        }
        self.files = new_files;
        self.selected_path.clone_from(&reconciled.selected);
        cx.notify();

        let Some(path) = self.selected_path.clone() else {
            return;
        };
        let selection_changed = prev_selected.as_deref() != Some(path.as_str());
        let stale = self
            .patch_cache
            .get(&path)
            .is_none_or(|cached| !selection_changed && cached.mtime != fresh_mtime);
        if stale {
            self.load_patch(&path, cx);
        }
    }

    pub fn select(&mut self, path: String, cx: &mut Context<Self>) {
        if self.selected_path.as_deref() == Some(path.as_str()) {
            return;
        }
        self.selected_path = Some(path.clone());
        cx.notify();
        if !self.patch_cache.contains_key(&path) {
            self.load_patch(&path, cx);
        }
    }

    fn load_patch(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(file) = self.files.iter().find(|f| f.path == path) else {
            return;
        };
        let wt_path = self.wt_path.clone();
        let path = path.to_string();
        let status = file.status.clone();
        let insert_key = path.clone();
        cx.spawn(async move |this, cx| {
            let path_for_mtime = path.clone();
            let wt_for_mtime = wt_path.clone();
            // Baking happens off the main thread; the split bake alone measured ~2.4ms in `diff_bench`.
            let (
                patch,
                mtime,
                unified,
                split,
                unified_widest,
                split_old_widest_text,
                split_new_widest_text,
            ) = cx
                .background_spawn(async move {
                    let patch = diff::file_patch(&wt_path, &path, &status);
                    let mtime =
                        fs_err::metadata(std::path::Path::new(&wt_for_mtime).join(&path_for_mtime))
                            .ok()
                            .and_then(|m| m.modified().ok());
                    // Only a `Text` patch has anything to highlight; `Binary`/`TooLarge` skip to empty spans.
                    let (old_spans, new_spans) = match &patch {
                        Patch::Text { .. } => {
                            let old_spans = diff::head_blob(&wt_path, &path)
                                .map(|blob| highlight::highlight_file(&blob, &path))
                                .unwrap_or_default();
                            let new_spans = diff::working_file(&wt_path, &path)
                                .map(|content| highlight::highlight_file(&content, &path))
                                .unwrap_or_default();
                            (old_spans, new_spans)
                        }
                        Patch::TooLarge { .. } | Patch::Binary => (Vec::new(), Vec::new()),
                    };
                    let unified = render_rows::unified_render_rows(&patch, &old_spans, &new_spans);
                    let split = render_rows::split_render_rows(&patch, &old_spans, &new_spans);
                    let unified_widest = render_rows::widest_unified_row(&unified);
                    let split_old_widest_text =
                        render_rows::widest_split_side_text(&split, true).to_string();
                    let split_new_widest_text =
                        render_rows::widest_split_side_text(&split, false).to_string();
                    (
                        patch,
                        mtime,
                        unified,
                        split,
                        unified_widest,
                        split_old_widest_text,
                        split_new_widest_text,
                    )
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                // Only the O(1) `Rc` wrapping happens on the main thread — `Rc` is not `Send`.
                this.patch_cache.insert(
                    insert_key,
                    CachedPatch {
                        patch,
                        mtime,
                        unified: std::rc::Rc::new(unified),
                        split: std::rc::Rc::new(split),
                        unified_widest,
                        split_old_widest_text,
                        split_new_widest_text,
                    },
                );
                cx.notify();
            });
        })
        .detach();
    }

    pub fn selected_patch(&self) -> Option<&Patch> {
        let path = self.selected_path.as_deref()?;
        self.patch_cache.get(path).map(|c| &c.patch)
    }

    pub fn selected_unified(&self) -> Option<(&std::rc::Rc<Vec<UnifiedRenderRow>>, usize)> {
        let path = self.selected_path.as_deref()?;
        self.patch_cache
            .get(path)
            .map(|c| (&c.unified, c.unified_widest))
    }

    /// Unlike [`Self::selected_unified`], no widest-row index: split columns size from measured text widths instead.
    pub fn selected_split(&self) -> Option<&std::rc::Rc<Vec<SplitRenderRow>>> {
        let path = self.selected_path.as_deref()?;
        self.patch_cache.get(path).map(|c| &c.split)
    }

    pub fn selected_split_col_texts(&self) -> Option<(&str, &str)> {
        let path = self.selected_path.as_deref()?;
        self.patch_cache.get(path).map(|c| {
            (
                c.split_old_widest_text.as_str(),
                c.split_new_widest_text.as_str(),
            )
        })
    }

    pub fn toggle_list_style(&mut self, cx: &mut Context<Self>) {
        self.list_style = match self.list_style {
            FileListStyle::Flat => FileListStyle::Tree,
            FileListStyle::Tree => FileListStyle::Flat,
        };
        cx.notify();
    }

    pub fn toggle_dir(&mut self, path: String, cx: &mut Context<Self>) {
        if !self.tree_expanded.remove(&path) {
            self.tree_expanded.insert(path);
        }
        cx.notify();
    }

    /// Tree mode skips a collapsed directory's contents, matching what's on-screen.
    fn visible_file_order(&self) -> Vec<String> {
        match self.list_style {
            FileListStyle::Flat => self.files.iter().map(|f| f.path.clone()).collect(),
            FileListStyle::Tree => diff::flatten_file_tree(&self.files, &self.tree_expanded)
                .into_iter()
                .filter_map(|node| match node {
                    diff::TreeNode::File { file, .. } => Some(file.path),
                    diff::TreeNode::Dir { .. } => None,
                })
                .collect(),
        }
    }

    /// Wraps neither end — hitting an edge simply stays put.
    pub fn move_selection(&mut self, delta: i32, cx: &mut Context<Self>) {
        let order = self.visible_file_order();
        if order.is_empty() {
            return;
        }
        let cur = self
            .selected_path
            .as_ref()
            .and_then(|p| order.iter().position(|f| f == p))
            .unwrap_or(0);
        let next = (cur as i32 + delta).clamp(0, order.len() as i32 - 1) as usize;
        self.select(order[next].clone(), cx);
    }

    pub fn focus_body(&mut self, cx: &mut Context<Self>) {
        self.body_focused = true;
        cx.notify();
    }

    /// gpui's scroll offset is negative as content moves up; clamping against `+max_offset.y` would pin the body at zero.
    pub fn scroll_body(&mut self, delta: i32, cx: &mut Context<Self>) {
        let step = crate::views::tokens::DIFF_BODY_LINE_H * delta as f32;
        let base = self.base_scroll();
        let mut offset = base.offset();
        offset.y = (offset.y - gpui::px(step))
            .max(-base.max_offset().y)
            .min(gpui::px(0.0));
        base.set_offset(offset);
        cx.notify();
    }

    /// The only way the view should read [`Self::pan_x`] — clamped to what the layout can show.
    pub fn split_pan(&self, left_content_w: f32, right_content_w: f32, half_w: f32) -> f32 {
        render_rows::clamp_split_pan(self.pan_x, left_content_w, right_content_w, half_w)
    }
    /// Keyboard Left/Right while the body is focused is deliberately left unwired here.
    pub fn pan_split(
        &mut self,
        delta_x: f32,
        left_content_w: f32,
        right_content_w: f32,
        half_w: f32,
        cx: &mut Context<Self>,
    ) {
        let next = render_rows::clamp_split_pan(
            self.split_pan(left_content_w, right_content_w, half_w) + delta_x,
            left_content_w,
            right_content_w,
            half_w,
        );
        if next == self.pan_x {
            return;
        }
        self.pan_x = next;
        cx.notify();
    }

    /// [`Self::scroll_body`] needs the real offset/extent underneath the `UniformListScrollHandle` wrapper.
    pub fn base_scroll(&self) -> gpui::ScrollHandle {
        self.body_scroll.0.borrow().base_handle.clone()
    }

    /// Persistence is the caller's — the click and key paths both write `SettingsState` before reaching here.
    pub fn set_mode(&mut self, mode: DiffMode, cx: &mut Context<Self>) {
        self.mode = mode;
        cx.notify();
    }
}
