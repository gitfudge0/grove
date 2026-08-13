//! The diff viewer's gpui-side state: which worktree is open, its file list,
//! the selected path, and a per-path patch cache — all loaded off the main
//! thread through [`gpui::Context::background_spawn`], following the same
//! idiom [`crate::entities::upgrade::Upgrade`] and
//! [`crate::entities::session_registry`] use for their own I/O.
//!
//! Everything that can be computed without gpui — parsing, patch shape, the
//! oversize/binary guards, live-update reconciliation, tree flattening —
//! already lives in `grove_core::diff`. This entity's only job is *when*
//! that runs and *where* the result lands: it owns no diff arithmetic of its
//! own.

use std::collections::HashMap;
use std::time::{Instant, SystemTime};

use gpui::{AppContext as _, Context};
use grove_core::diff::{self, FileChange, Patch};
use grove_core::highlight;
use grove_core::render_rows::{self, SplitRenderRow, UnifiedRenderRow};
use grove_core::storage::DiffMode;

/// The live-update poll's own cadence, matched to `ProjectTree`'s
/// `GIT_POLL_INTERVAL` (`src/entities/project_tree.rs`) so the file list
/// refreshes on the same rhythm as the git-state chips it's opened from,
/// without this entity depending on that private constant.
const LIVE_REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// Which list presentation the file list uses. The tree toggle is
/// session-only (never persisted) and scoped per worktree, per the brief —
/// switching worktrees does not carry an expansion set over.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FileListStyle {
    #[default]
    Flat,
    Tree,
}

/// One cached patch, keyed by the file's mtime at load time so a later poll
/// can tell a stale entry from a fresh one without re-diffing eagerly.
#[derive(Clone, Debug)]
pub struct CachedPatch {
    pub patch: Patch,
    /// Compared against a fresh `stat` on every live-update tick for the
    /// *selected* file: unchanged mtime skips the reload, a changed one
    /// re-triggers it (`Self::maybe_refresh_live`).
    pub mtime: Option<SystemTime>,
    /// The fully baked render rows for both modes, built once per load in
    /// [`Self`]'s construction site (`DiffViewerState::load_patch`, on the
    /// main thread) so the render path never derives rows again.
    ///
    /// `Rc` rather than `Arc` or a plain `Vec`: the row vectors never cross
    /// a thread boundary (they are built after the background work lands),
    /// and the render path clones the handle cheaply instead of the rows.
    pub unified: std::rc::Rc<Vec<UnifiedRenderRow>>,
    pub split: std::rc::Rc<Vec<SplitRenderRow>>,
    /// Index of the widest unified row — the horizontal scroll extent's
    /// reference row (see [`grove_core::render_rows::widest_unified_row`]).
    /// The split body has no counterpart: its `uniform_list` sizes columns
    /// from `split_col_w`'s measured text widths (below), not from a
    /// precomputed widest-row index — every `SplitRenderRow::Lines` row now
    /// has a definite pixel width, so there is no "widest" row to pick.
    pub unified_widest: usize,
    /// The widest line of text on each side of the split body (see
    /// [`grove_core::render_rows::widest_split_side_text`]), baked here so
    /// the view measures its real pixel width (needs a `Window`, so that
    /// step can't happen off-thread) at most twice per render frame instead
    /// of rescanning every row.
    pub split_old_widest_text: String,
    pub split_new_widest_text: String,
}

/// The diff viewer's live state for one open worktree. Constructed fresh each
/// time the modal opens (`ModalLayer::open`) rather than kept warm across
/// closes, so a stale file list can never leak into a different worktree's
/// session — mirroring why `Modal::AgentPicker` is rebuilt per open rather
/// than reused.
pub struct DiffViewerState {
    pub wt_path: String,
    /// The worktree's current branch, for the header (`None` while unknown or
    /// detached — the header falls back to the path alone).
    pub branch: Option<String>,
    pub files: Vec<FileChange>,
    /// Selection follows the *path*, not an index, so a file that stops
    /// being changed can show "No longer changed" in place instead of
    /// snapping the selection elsewhere (brief decision 6) — see
    /// [`Self::maybe_refresh_live`] for the live-update side of this.
    pub selected_path: Option<String>,
    /// Which list presentation the body renders — read by the split-mode
    /// renderer and flipped by the header's segmented control / Tab.
    pub mode: DiffMode,
    pub list_style: FileListStyle,
    /// Session-only, per-worktree tree-mode disclosure state, keyed by
    /// directory path. Directories default to *expanded* (brief decision
    /// 7), so this holds the directories the user has explicitly
    /// **collapsed** — an empty set means "everything expanded", matching
    /// [`grove_core::diff::flatten_file_tree`]'s `collapsed` parameter
    /// directly.
    pub tree_expanded: std::collections::HashSet<String>,
    pub patch_cache: HashMap<String, CachedPatch>,
    /// True while the file list or a patch is loading on the background
    /// executor. The view shows a plain loading state rather than an empty
    /// list while this is set.
    pub loading: bool,
    /// Last time [`Self::maybe_refresh_live`] actually kicked off a
    /// background refresh — its own throttle, on the same cadence as
    /// [`LIVE_REFRESH_INTERVAL`] rather than firing on every render.
    last_live_refresh: Option<Instant>,
    /// Set while a live-update refresh is in flight, so a second render
    /// frame before it lands can't overlap it — the same discipline
    /// `ProjectTree::git_poll_inflight` uses for its own poll.
    live_refresh_inflight: bool,
    /// True once Enter has moved keyboard focus from the file list to the
    /// scrolling body — ↑/↓/j/k then scroll the body instead of moving the
    /// file selection.
    pub body_focused: bool,
    /// The body's vertical scroll position, moved by one [`crate::views::tokens::DIFF_BODY_LINE_H`]
    /// per Move verdict while [`Self::body_focused`] is set.
    ///
    /// A [`gpui::UniformListScrollHandle`], shared by both modes: unified and
    /// split are both `uniform_list`s now, so there is only one scroll
    /// target — see [`Self::scroll_body`].
    pub body_scroll: gpui::UniformListScrollHandle,
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

    /// The live-update tick: called from the same call site that drives
    /// `ProjectTree::maybe_poll_git_state` (`src/views/workspace.rs`'s
    /// render), so the file list refreshes on the worktree git-state poll's
    /// own cadence rather than a second timer. Self-throttled to
    /// [`LIVE_REFRESH_INTERVAL`] and guarded against overlap, mirroring
    /// `ProjectTree`'s own poll discipline.
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
        // Only the currently-selected file's mtime is worth a fresh `stat`
        // here — every other cached patch is either still valid (its path is
        // still changed and its content is fine to redisplay lazily) or gets
        // evicted outright once it drops out of the fresh file list.
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

    /// Fold a fresh [`diff::changed_files`] result into live state: pure
    /// reconciliation via [`diff::reconcile`] decides the new selection and
    /// which cache entries to evict, then the selected file's patch reloads
    /// only if its mtime actually changed (or it's newly selected and not
    /// yet cached) — never unconditionally, so an open, untouched file isn't
    /// re-diffed and re-highlighted every tick.
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

    /// Select `path` and, if its patch is not already cached, load it.
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

    /// Kick off `file_patch` on the background executor for `path`, caching
    /// the result keyed by the working file's mtime at load time.
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
            // Everything below is `Send`: `Patch`, `Vec<Vec<Span>>`,
            // `Vec<UnifiedRenderRow>` and `Vec<SplitRenderRow>` all hold only
            // owned data (`String`, `Vec`, `Copy` enums) — nothing here is an
            // `Rc`. Only the `Rc::new(...)` wrapping below is main-thread-only,
            // so the actual baking (previously done in `this.update` "because
            // `Rc` is not `Send`") now happens here instead, off the main
            // thread — the split bake alone measured ~2.4ms in
            // `diff_bench`, a real hitch to pay on every render frame's
            // thread if left on `this.update`.
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
                    // Highlight both sides once here, off the main thread,
                    // cached alongside the patch — never recomputed on the
                    // render path. Only a `Text` patch has anything to
                    // highlight; `Binary`/`TooLarge` skip straight to empty
                    // spans, matching their existing no-content stubs.
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
                // Only the O(1) `Rc` wrapping happens on the main thread —
                // `Rc` itself is not `Send`, so it can never cross the
                // `background_spawn` boundary above.
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

    /// The selected file's cached patch, if loaded yet.
    pub fn selected_patch(&self) -> Option<&Patch> {
        let path = self.selected_path.as_deref()?;
        self.patch_cache.get(path).map(|c| &c.patch)
    }

    /// The selected file's baked unified render rows plus the index of its
    /// widest row — one cache lookup, no derivation. `None` until the patch
    /// has loaded.
    pub fn selected_unified(&self) -> Option<(&std::rc::Rc<Vec<UnifiedRenderRow>>, usize)> {
        let path = self.selected_path.as_deref()?;
        self.patch_cache
            .get(path)
            .map(|c| (&c.unified, c.unified_widest))
    }

    /// The selected file's baked split render rows — one cache lookup, no
    /// derivation. `None` until the patch has loaded. Unlike
    /// [`Self::selected_unified`], this has no widest-row index to hand
    /// back: the split body's `uniform_list` sizes its columns from
    /// `selected_split_col_texts`'s measured text widths, since every row
    /// already has a definite pixel width and none is "the widest".
    pub fn selected_split(&self) -> Option<&std::rc::Rc<Vec<SplitRenderRow>>> {
        let path = self.selected_path.as_deref()?;
        self.patch_cache.get(path).map(|c| &c.split)
    }

    /// The widest line of text on each side (old, new) of the selected
    /// file's split body — see [`CachedPatch::split_old_widest_text`].
    pub fn selected_split_col_texts(&self) -> Option<(&str, &str)> {
        let path = self.selected_path.as_deref()?;
        self.patch_cache.get(path).map(|c| {
            (
                c.split_old_widest_text.as_str(),
                c.split_new_widest_text.as_str(),
            )
        })
    }

    /// Flips flat/tree. Wired to the file list's segmented control.
    pub fn toggle_list_style(&mut self, cx: &mut Context<Self>) {
        self.list_style = match self.list_style {
            FileListStyle::Flat => FileListStyle::Tree,
            FileListStyle::Tree => FileListStyle::Flat,
        };
        cx.notify();
    }

    /// Flip one directory's collapsed state (tree mode's disclosure click).
    pub fn toggle_dir(&mut self, path: String, cx: &mut Context<Self>) {
        if !self.tree_expanded.remove(&path) {
            self.tree_expanded.insert(path);
        }
        cx.notify();
    }

    /// The file paths in on-screen order for whichever list style is active
    /// — flat is just `files` in path order; tree mode walks
    /// [`diff::flatten_file_tree`] and keeps only the file rows, so a
    /// collapsed directory's contents are skipped exactly as they are
    /// on-screen. Keyboard navigation (`Self::move_selection`) and mouse
    /// selection both want this same order.
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

    /// Move the selection by `delta` rows (`+1`/`-1`), used by ↑/↓/j/k.
    /// Wraps neither end — hitting an edge simply stays put. Traverses the
    /// *visible* order, so tree mode skips a collapsed directory's contents.
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

    /// Move keyboard focus to the scrolling body (Enter).
    pub fn focus_body(&mut self, cx: &mut Context<Self>) {
        self.body_focused = true;
        cx.notify();
    }

    /// Scroll the body by `delta` lines (`+1`/`-1`), one
    /// [`crate::views::tokens::DIFF_BODY_LINE_H`] each, clamped to the
    /// container's real scroll range.
    ///
    /// gpui's scroll offset is **negative** as content moves up, and
    /// `ScrollHandle::max_offset` is the *positive* overflow, so the valid
    /// range is `-max_offset.y ..= 0` (`div.rs:2271-2276` clamps exactly that
    /// way on prepaint). Clamping against `+max_offset.y` would pin the body
    /// at zero and swallow every downward key.
    pub fn scroll_body(&mut self, delta: i32, cx: &mut Context<Self>) {
        let step = crate::views::tokens::DIFF_BODY_LINE_H * delta as f32;
        // Both modes are `uniform_list`s sharing `body_scroll` now, so there
        // is only one scroll target regardless of `self.mode`. Clone the
        // inner handle out from under the `RefCell` first — `ScrollHandle`
        // shares its state, so the clone drives the same scroll position,
        // and no borrow is held across the reads/write below.
        let base = self.base_scroll();
        let mut offset = base.offset();
        offset.y = (offset.y - gpui::px(step))
            .max(-base.max_offset().y)
            .min(gpui::px(0.0));
        base.set_offset(offset);
        cx.notify();
    }

    /// The plain [`gpui::ScrollHandle`] inside [`Self::body_scroll`], shared
    /// (not copied) with whichever mode's `uniform_list` is on screen —
    /// [`Self::scroll_body`]'s arithmetic needs the real offset and extent
    /// underneath the `UniformListScrollHandle` wrapper.
    pub fn base_scroll(&self) -> gpui::ScrollHandle {
        self.body_scroll.0.borrow().base_handle.clone()
    }

    /// Apply `mode` to the open viewer. Persistence is the *caller's* — the
    /// segment click and `Tab`'s handler both write `SettingsState` and then
    /// reach here, so the click and key paths can never diverge.
    pub fn set_mode(&mut self, mode: DiffMode, cx: &mut Context<Self>) {
        self.mode = mode;
        cx.notify();
    }
}
