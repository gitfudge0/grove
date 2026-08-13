//! The root view: the sidebar rail, the divider, and the body showing whatever
//! `WorkspaceState` says is active.
//!
//! Plans 06-07 replace the remaining placeholders (appbar, statusbar, grid,
//! zen). Every dimension comes from a named constant carrying its
//! `src/gui/metrics.rs` line.

use crate::views::rpx;
use std::collections::HashMap;

use gpui::{div, prelude::*, px, App, Context, Entity, FocusHandle, Focusable, Window};

use crate::activity::ActivityState;
use crate::entities::activity_store::ActivityStore;
use crate::entities::animation_clock::AnimationClock;
use crate::entities::project_tree::ProjectTree;
use crate::entities::session_registry::{SessionId, SessionRegistry};
use crate::entities::terminal_session::TerminalSession;
use crate::entities::toast::ToastState;
use crate::entities::upgrade::Upgrade;
use crate::entities::upgrade_state::upgrade_available;
use crate::entities::workspace_state::{
    clamp_sidebar_width, term_portion_for_cursor, LiveTile, PtyPane, RailMode, WorkspaceState,
    RAIL_W,
};
use crate::fonts::{MONO_FAMILY, UI_FAMILY};
use crate::keymap;
use crate::settings::SettingsState;
use crate::theme as c;
use crate::views::appbar::{self, AppbarCtx, ChromeAction, WaitingRow};
use crate::views::grid::{self, GridAction, GridCtx, TileData, PTY_PAD_H, PTY_PAD_W};
use crate::views::modals::{ModalEvent, ModalLayer};
use crate::views::rows;
use crate::views::session_header::{self, SessionHeaderData, ToolAction, ToolCluster};
use crate::views::sidebar::{self, Sidebar};
use crate::views::statusbar::{self, StatusbarCtx};
use crate::views::term_panel::{self, PanelAction, PanelCtx, ShellTab};
use crate::views::terminal_tab::{self, TerminalTabAction, TerminalTabCtx};
use crate::views::terminal_view::TerminalView;
use crate::views::tokens::{ICON_SM, SPACE_MD, SPACE_SM, TEXT_MICRO};
use crate::zoom::{self, ZoomState};

pub struct Workspace {
    focus: FocusHandle,
    /// Kept alive here: dropping the clock entity would stop every animation
    /// in the window, including the terminal cursor blink.
    clock: Entity<AnimationClock>,
    state: Entity<WorkspaceState>,
    registry: Entity<SessionRegistry>,
    tree: Entity<ProjectTree>,
    activity: Entity<ActivityStore>,
    toast: Entity<ToastState>,
    sidebar: Entity<Sidebar>,
    /// The single modal slot, rendered above everything (Plan 08 Task 2).
    modals: Entity<ModalLayer>,
    /// The self-update flow: the three check triggers, the changelog and the
    /// apply. Kept alive here; dropping it would cancel every timer.
    upgrade: Entity<Upgrade>,
    /// One view per session, cached by id so switching does not respawn
    /// anything (Task 6 Step 2).
    views: HashMap<SessionId, Entity<TerminalView>>,
    home_views: HashMap<SessionId, Entity<TerminalView>>,
    /// One view per **panel shell**, same memoization as the agent views.
    panel_views: HashMap<SessionId, Entity<TerminalView>>,
    /// The split divider's drag state, and the previous press for the 350ms
    /// double-click reset (`layout.rs:162-197`).
    term_panel_dragging: bool,
    last_term_divider_press: Option<std::time::Instant>,
    /// The window's logical width, refreshed each frame — the divider drag maps
    /// a cursor x against it.
    logical_win_w: f32,
    /// The terminal takes focus on the first frame so keystrokes land without
    /// a click; `window.focus` needs a `&mut Window`, which `new` has not got.
    /// Cleared again whenever a modal closes: the layer's focus handle leaves
    /// the element tree with it, and a `Window::focus` pointing at a handle
    /// that is no longer painted dispatches to the *dispatch-tree root*,
    /// which is above this view's div — so every binding and `on_key_down`
    /// below it would go dead until the next click.
    focused_once: bool,
    /// The body view that currently holds keyboard focus. Switching sessions
    /// swaps the body entity without any focus call of its own, so `render`
    /// compares this against the live body's id and refocuses when it changes
    /// — never every frame, or the terminal would fight for focus.
    last_body_focused: Option<gpui::EntityId>,
    /// `observe_window_activation` needs a `&mut Window`, which `new` has not
    /// got — registered on the first frame instead.
    activation_observed: bool,
    /// The agent pane's PTY dims as of the last frame. A reattached session is
    /// resized to these the moment it attaches, so tmux does not report a
    /// stale geometry on the first frame (`src/gui/update/mod.rs:135-139`).
    last_pty_dims: (u16, u16),
    /// `last_pty_dims` from the previous frame, so the startup discovery gate
    /// can tell a settled viewport (two consecutive equal frames) from a
    /// still-resizing window (Wayland configures geometry over several
    /// frames, so frame 1's size is transient, not final).
    prev_pty_dims: Option<(u16, u16)>,
    /// Frames rendered while waiting for `last_pty_dims` to settle, so
    /// discovery is forced after `TMUX_DISCOVERY_SETTLE_FRAMES` even if the
    /// viewport never stops changing.
    tmux_discovery_frames: u32,
    /// The startup tmux scan runs once, after the viewport has settled.
    tmux_discovered: bool,
    /// Frames rendered since discovery, so a reattached session that no tile
    /// ever paints still gets its deferred tmux attach after
    /// `TMUX_ATTACH_FALLBACK_FRAMES`. Stops counting once nothing is pending.
    tmux_attach_frames: u32,
    /// The sidebar width is seeded from the real, zoom-corrected window width
    /// on the first frame rather than the 1280px placeholder `new` had to use
    /// before any `Window` existed (`update/mod.rs:124-127`); every later
    /// frame just re-clamps the live width.
    sidebar_seeded: bool,
    /// `Window::on_window_should_close` is registered on the first frame for
    /// the same reason, and additionally needs *this* entity (it counts the
    /// running native sessions and runs [`Workspace::shutdown`]).
    close_hook_registered: bool,
    /// The first-run check runs on the first frame and never again — a latch
    /// in the same family as `close_hook_registered` above. Without it every
    /// later render would re-open the wizard the moment the user skipped it,
    /// because `store.onboarded` only flips on the *last* step.
    first_run_checked: bool,
    /// Last frame's header-segment decision per session, so
    /// [`grid::fit_segments`] has a `prev` to apply hysteresis against. A
    /// render-time memo, not app state: it is deliberately *not* on
    /// `WorkspaceState` and is never persisted. Evicted down to `tile_order`
    /// each frame so it cannot outgrow the live sessions.
    header_fits: HashMap<SessionId, grid::HeaderFit>,
    observers: Vec<gpui::Subscription>,
}

impl Focusable for Workspace {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

// ── tile-header measurement ──────────────────────────────────────────────
//
// `grid::tile_header` decides which identity segments it can afford from
// widths measured here, because this is where a `&Window` — and therefore a
// text system — exists. Everything below returns **device pixels**; the
// decision itself is `grid::fit_segments`, which is pure.

/// Token space → **device pixels**. Every `SPACE_*`/`TEXT_*`/`ICON_*` token is
/// authored against [`zoom::REM_BASE`] through [`rpx`], and `render` sets
/// `rem_size = px(REM_BASE * zoom)` (`:1780`), so a token of `v` paints at
/// `rem_size * (v / REM_BASE)` — i.e. `v * zoom`. Logical px (viewport over
/// zoom, `sidebar_width`) convert by the same factor. Mixing the two spaces
/// would leave the budget correct at 100% zoom only.
pub(crate) fn token_px(v: f32, window: &Window) -> f32 {
    f32::from(window.rem_size()) * (v / zoom::REM_BASE)
}

/// The advance width of `text` as `components::ui`/`mono` would paint it.
/// `WindowTextSystem::layout_line` is cached per frame, so one call per
/// segment per tile per frame costs a hash lookup after the first.
pub(crate) fn text_px(
    window: &Window,
    text: &str,
    family: &'static str,
    size: f32,
    weight: gpui::FontWeight,
) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    let mut font = gpui::font(family);
    font.weight = weight;
    let run = gpui::TextRun {
        len: text.len(),
        font,
        color: gpui::transparent_black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let layout = window
        .text_system()
        .layout_line(text, px(token_px(size, window)), &[run], None);
    f32::from(layout.width)
}

/// What one optional identity segment costs the row: `grid::tile_header`
/// renders the `·` and the name as two children of a `gap(rpx(SPACE_SM))`
/// flex row, so the two gaps are part of the segment, not of the frame.
/// Blank text costs nothing and is reported as absent, which is what stops a
/// branchless tile from painting an orphan dot.
fn segment_px(window: &Window, text: &str) -> f32 {
    if text.trim().is_empty() {
        return 0.0;
    }
    let normal = gpui::FontWeight::default();
    2.0f32.mul_add(
        token_px(SPACE_SM, window),
        text_px(window, "·", UI_FAMILY, TEXT_MICRO, normal)
            + text_px(window, text, UI_FAMILY, TEXT_MICRO, normal),
    )
}

/// `keycap_filled`'s shell (`components.rs:102-109`): `SPACE_MD` of padding
/// each side, plus the 1px stroke both the number hint and the respond chip
/// add themselves with `.border_1()`.
fn keycap_shell_px(window: &Window) -> f32 {
    2.0f32.mul_add(token_px(SPACE_MD, window), 2.0)
}

/// `grid::chord`'s inner width: on macOS a square `TEXT_MICRO` command glyph,
/// a 1px gap and the mono digit; elsewhere the mono `"{mod}+{n}"` the
/// registry spells.
fn chord_px(window: &Window, tile_idx: usize) -> f32 {
    let n = tile_idx + 1;
    let normal = gpui::FontWeight::default();
    if cfg!(target_os = "macos") {
        token_px(TEXT_MICRO, window)
            + 1.0
            + text_px(window, &n.to_string(), MONO_FAMILY, TEXT_MICRO, normal)
    } else {
        let label = format!("{}+{n}", keymap::platform_mod_label());
        text_px(window, &label, MONO_FAMILY, TEXT_MICRO, normal)
    }
}

/// The controls cluster's width. It is `flex_shrink_0`, so this is space the
/// header can never reclaim however long the title gets.
fn controls_px(window: &Window, tile_idx: usize, waiting: bool) -> f32 {
    // zen + kill: `icon_btn` is exactly a `TILE_BTN_BOX` square, and
    // `grid::tile_btn` passes `hover_ring: false`, so it carries no stroke
    // (`components.rs:607-617`).
    let mut w = 2.0 * token_px(grid::TILE_BTN_BOX, window);
    let mut children = 2u16;
    if tile_idx < 9 {
        w += keycap_shell_px(window) + chord_px(window, tile_idx);
        children += 1;
    }
    if waiting {
        w += keycap_shell_px(window)
            + text_px(
                window,
                grid::respond_label(tile_idx),
                MONO_FAMILY,
                TEXT_MICRO,
                gpui::FontWeight::default(),
            );
        if tile_idx < 9 {
            // `respond_chip`'s own 1px gap before the chord.
            w += 1.0 + chord_px(window, tile_idx);
        }
        children += 1;
    }
    token_px(SPACE_SM, window).mul_add(f32::from(children - 1), w)
}

/// How much of `tile_w_px` is left for the optional segments and the title.
fn header_budget_px(
    window: &Window,
    tile_w_px: f32,
    tile_idx: usize,
    waiting: bool,
    agent_label: &str,
) -> f32 {
    // A waiting tile spends 1px per side on its amber hairline (`grid::tile`).
    let border = if waiting { 2.0 } else { 0.0 };
    // `tile_header`'s own `.px(rpx(SPACE_MD))`, and the two `SPACE_SM` gaps
    // between its three always-present zones.
    let frame = 2.0f32.mul_add(token_px(SPACE_MD, window), 2.0 * token_px(SPACE_SM, window));
    // Always rendered, so never negotiable: the agent icon box, its gap, and
    // the agent label — measured SEMIBOLD because that is how it paints, and
    // measuring it at normal weight under-counts every tile.
    let identity_base = token_px(ICON_SM, window)
        + token_px(SPACE_SM, window)
        + text_px(
            window,
            agent_label,
            UI_FAMILY,
            TEXT_MICRO,
            gpui::FontWeight::SEMIBOLD,
        );
    (tile_w_px - border - frame - controls_px(window, tile_idx, waiting) - identity_base).max(0.0)
}

impl Workspace {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let clock = cx.new(AnimationClock::new);
        let state = cx.new(|cx| WorkspaceState::new(&cx.global::<SettingsState>().store, 1280.0));
        let registry = cx.new(|_| SessionRegistry::new());
        let tree = cx.new(|_| ProjectTree::new());
        let activity = cx.new({
            let (state, registry) = (state.clone(), registry.clone());
            |cx| ActivityStore::start(state, registry, cx)
        });

        // Seed the active project's worktrees so the rail has something to draw
        // on the first frame (`App::refresh_worktrees`).
        let active_project = cx
            .global::<SettingsState>()
            .store
            .active_projects()
            .next()
            .map(|(i, p)| (i, p.path.clone()));
        if let Some((idx, path)) = active_project {
            tree.update(cx, |t, _| {
                t.set_active_worktrees(idx, grove_core::git::list_worktrees(&path));
            });
        }

        let toast = cx.new(|_| ToastState::new());

        let sidebar = cx.new({
            let (state, tree, registry, activity, clock) = (
                state.clone(),
                tree.clone(),
                registry.clone(),
                activity.clone(),
                clock.clone(),
            );
            |cx| Sidebar::new(state, tree, registry, activity, clock, cx)
        });

        let upgrade = cx.new(Upgrade::new);

        let modals = cx.new({
            let (state, registry, tree, toast, activity, clock, upgrade) = (
                state.clone(),
                registry.clone(),
                tree.clone(),
                toast.clone(),
                activity.clone(),
                clock.clone(),
                upgrade.clone(),
            );
            |cx| ModalLayer::new(state, registry, tree, toast, activity, clock, upgrade, cx)
        });
        sidebar.update(cx, |s, _| s.set_modals(modals.clone(), toast.clone()));

        let observers = vec![
            // The clock drives the cursor blink inside the terminal; the
            // workspace repaints with it so chrome animations stay in phase.
            cx.observe(&clock, |_, _, cx| cx.notify()),
            // Selection changes repaint the body (Task 6 Step 2).
            cx.observe(&state, |_, _, cx| cx.notify()),
            cx.observe(&registry, |this, _, cx| {
                this.sync_grid_tiles(cx);
                cx.notify();
            }),
            // The 480ms pass repaints the chrome that reads it.
            cx.observe(&activity, |_, _, cx| cx.notify()),
            // The toast's own TTL task clears it; the statusbar repaints with it.
            cx.observe(&toast, |_, _, cx| cx.notify()),
            // The modal layer repaints the window and hands back the effects
            // it cannot perform itself.
            cx.observe(&modals, |_, _, cx| cx.notify()),
            cx.subscribe(&modals, Self::on_modal_event),
            // The cog's dot and the Settings modal both read this entity.
            cx.observe(&upgrade, |_, _, cx| cx.notify()),
        ];

        Self {
            focus: cx.focus_handle(),
            clock,
            state,
            registry,
            tree,
            activity,
            toast,
            sidebar,
            modals,
            upgrade,
            views: HashMap::new(),
            home_views: HashMap::new(),
            panel_views: HashMap::new(),
            term_panel_dragging: false,
            last_term_divider_press: None,
            logical_win_w: 1280.0,
            focused_once: false,
            last_body_focused: None,
            activation_observed: false,
            close_hook_registered: false,
            first_run_checked: false,
            last_pty_dims: (24, 80),
            prev_pty_dims: None,
            tmux_discovery_frames: 0,
            tmux_discovered: false,
            tmux_attach_frames: 0,
            sidebar_seeded: false,
            header_fits: HashMap::new(),
            observers,
        }
    }

    /// Applies a new zoom level: state, the debounced persist, repaint.
    pub(crate) fn set_zoom(zoom_value: f32, cx: &mut App) {
        let snapped = zoom::snap(zoom_value);
        if cx.global::<ZoomState>().zoom == snapped {
            return;
        }
        cx.global_mut::<ZoomState>().zoom = snapped;
        SettingsState::update(cx, |s| s.ui_zoom = Some(snapped));
        cx.refresh_windows();
    }

    fn zoom_in(_: &keymap::ZoomIn, _: &mut Window, cx: &mut App) {
        crate::telemetry::track("zoom_changed", vec![]);
        Self::set_zoom(cx.global::<ZoomState>().zoom + zoom::ZOOM_STEP, cx);
    }

    fn zoom_out(_: &keymap::ZoomOut, _: &mut Window, cx: &mut App) {
        crate::telemetry::track("zoom_changed", vec![]);
        Self::set_zoom(cx.global::<ZoomState>().zoom - zoom::ZOOM_STEP, cx);
    }

    fn zoom_reset(_: &keymap::ZoomReset, _: &mut Window, cx: &mut App) {
        crate::telemetry::track("zoom_changed", vec![]);
        Self::set_zoom(zoom::ZOOM_DEFAULT, cx);
    }

    // ── data-carrying actions (Task 6 Step 1) ───────────────────────────

    fn snapshot(&self, cx: &mut App) -> crate::entities::workspace_state::TreeSnapshot {
        let active_proj = self.state.read(cx).proj_idx();
        let registry = self.registry.clone();
        self.tree.clone().update(cx, |tree, cx| {
            let store = &cx.global::<SettingsState>().store;
            tree.snapshot(store, registry.read(cx), active_proj)
        })
    }

    /// The sidebar's own row list is the index space, so keyboard selection and
    /// what is on screen cannot disagree.
    fn visible_order(&self, cx: &App) -> Vec<SessionId> {
        self.sidebar.read(cx).visible_session_order()
    }

    // ── the grid's world (Plan 07 Task 3 Step 4) ────────────────────────

    /// Every live agent session with its stable cross-restart key — the input
    /// `WorkspaceState`'s grid transitions reconcile against.
    fn live_tiles(&self, cx: &App) -> Vec<LiveTile> {
        self.registry
            .read(cx)
            .all()
            .iter()
            .map(|m| LiveTile {
                id: m.id,
                key: crate::grid::session_grid_key(&m.project, &m.wt_path),
            })
            .collect()
    }

    /// The persisted arrangement (`Store::grid_order`).
    fn saved_grid_order(cx: &App) -> Vec<String> {
        cx.global::<SettingsState>().store.grid_order.clone()
    }

    /// Re-derive the grid's tile list from the live registry. Every registry
    /// change funnels through here — spawn, kill, tmux discovery — because
    /// `tile_order` was otherwise only built on grid entry and teardown, so a
    /// session spawned while the grid was up never got a tile.
    fn sync_grid_tiles(&mut self, cx: &mut Context<Self>) {
        let (grid, before_zen, known) = {
            let ws = self.state.read(cx);
            (
                ws.grid_view(),
                ws.grid_view_before_zen(),
                ws.tile_order().to_vec(),
            )
        };
        if !grid && !before_zen {
            return;
        }
        let live = self.live_tiles(cx);
        if live.len() == known.len() && live.iter().all(|t| known.contains(&t.id)) {
            return;
        }
        let saved = Self::saved_grid_order(cx);
        self.state
            .update(cx, |s, _| s.reconcile_after_teardown(&live, &saved));
    }

    /// Drains whatever the last transition staged and writes it to
    /// `Store::grid_order`, mapped back through each tile's stable key
    /// (`persist_grid_order`, `layout.rs:481-489`).
    fn persist_grid_order(&mut self, cx: &mut Context<Self>) {
        let Some(order) = self.state.update(cx, |s, _| s.take_grid_order_to_persist()) else {
            return;
        };
        let registry = self.registry.read(cx);
        let keys: Vec<String> = order
            .iter()
            .filter_map(|&id| registry.meta(id))
            .map(|m| crate::grid::session_grid_key(&m.project, &m.wt_path))
            .collect();
        SettingsState::update(cx, |s| s.grid_order = keys);
    }

    // ── the exit paths ──────────────────────────────────────────────────

    /// **The** flush. Every process-terminating path calls this and nothing
    /// else, so "did we persist?" is a structural property rather than a
    /// discipline (carried decision 7; iced's authoritative list is
    /// `src/gui/update/layout.rs:518-522`): the close request, the quit
    /// confirm, and the post-update restart.
    ///
    /// Idempotent by construction — the staged grid order is `take`n, and
    /// `flush_now` no-ops unless something is dirty — so calling it twice
    /// writes once.
    pub(crate) fn shutdown(&mut self, cx: &mut Context<Self>) {
        self.persist_grid_order(cx);
        SettingsState::flush_now(cx);
    }

    /// Exit path 3 of 3: the post-update restart, in `on_restart_app`'s exact
    /// order (`src/gui/update/upgrade.rs:115-125`) — relaunch first (the
    /// process exits either way, so a failed relaunch is the one chance to say
    /// anything about it), then the shared flush, then exit.
    fn restart_after_update(&mut self, cx: &mut Context<Self>) {
        if let Ok(exe) = std::env::current_exe() {
            if let Err(e) = std::process::Command::new(&exe).spawn() {
                tracing::error!(exe = %exe.display(), error = %e, "failed to relaunch after update");
            }
        }
        self.shutdown(cx);
        std::process::exit(0);
    }

    /// Running **native** sessions: the ones that die with the window.
    /// tmux-backed sessions survive grove and must never block a quit
    /// (`src/gui/update/modals.rs:339-341`, `src/app/mod.rs:273-279`).
    fn native_sessions_running(&self, cx: &mut App) -> usize {
        let ids: Vec<SessionId> = self.registry.read(cx).all().iter().map(|m| m.id).collect();
        ids.into_iter()
            .filter(|&id| {
                let Some(term) = self.registry.read(cx).session(id).cloned() else {
                    return false;
                };
                term.update(cx, |t, _| {
                    matches!(
                        t.backend(),
                        crate::entities::terminal_session::Backend::Native
                    ) && t.alive()
                })
            })
            .count()
    }

    /// Exit path 1 of 3: the window's close button / `mod+q`. Returning
    /// `false` vetoes the close, which is gpui's analogue of iced's
    /// `exit_on_close_request(false)` + `close_requests()` subscription
    /// (findings §S4; `src/gui/update/modals.rs:338-362`).
    fn register_close_hook(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |_window, cx| {
            this.update(cx, |this, cx| {
                let running = this.native_sessions_running(cx);
                if running == 0 {
                    this.shutdown(cx);
                    return true;
                }
                // Known, preserved gap: grove is one modal deep, so this
                // clobbers whatever was open and cancelling does not restore
                // it (`modal.rs`'s own passing test).
                this.open_modal(crate::modal::Modal::quit_confirm(running), cx);
                false
            })
            .unwrap_or(true)
        });
    }

    /// First run: a config that has never completed the wizard opens it, on
    /// the very first frame, before anything else can take the keyboard.
    ///
    /// `store.onboarded` is the whole condition — not "has no projects". A
    /// user who finished setup and later removed every project is *not* a
    /// fresh install, which is exactly what the field's own contract says
    /// (`grove-core/src/storage.rs:105-110`). The flag is written by the
    /// wizard's two exits — "Skip setup" and the final step — so a skipped
    /// wizard stays skipped across launches.
    ///
    /// The modal is a **screen replacement**, not a panel over the workspace
    /// (DESIGN.md §9.1.1's first exception), so opening it from the first
    /// frame — ahead of `render`'s `focused_once` block, which is gated off
    /// while a modal is open — is what keeps the terminal from stealing focus
    /// out from under it on the same frame. Mirrors iced starting the flow
    /// from app construction (`src/app/onboarding.rs`, whose step sequence
    /// begins at `Welcome`).
    ///
    /// The latch is set on the first *call*, not on the open: the check is
    /// what must happen once, so a user who dismisses the wizard by any route
    /// other than its own two exits does not get it back on the next repaint.
    fn first_run_check(&mut self, cx: &mut Context<Self>) {
        if self.first_run_checked {
            return;
        }
        self.first_run_checked = true;
        if cx.global::<SettingsState>().store.onboarded {
            return;
        }
        self.open_modal(crate::modal::Modal::onboarding(), cx);
    }

    // ── tmux sidecar discovery and reattach (Task 5) ────────────────────

    /// Attach every tmux session a previous grove run left behind, in the
    /// order [`crate::reattach::plan`] computed.
    ///
    /// Startup and the Settings tmux-toggle re-scan are the **same** call
    /// (`src/app/mod.rs:219-224` and `:347-366`); the plan's dedupe on tmux
    /// name is what makes running it twice safe. Reattached sessions keep
    /// their tmux backend even when the saved preference now says native —
    /// the oracle says so explicitly (`:217-219`).
    ///
    /// Failures are **silent** (`src/app/util.rs:12-13`): a tmux server that
    /// died between `list-sessions` and the attach must not stop grove from
    /// starting, so a failed attach is logged at `warn` and the rest continue.
    fn discover_tmux_sessions(&mut self, cx: &mut Context<Self>) {
        if !grove_core::tmux::available() {
            return;
        }
        let discovered = grove_core::tmux::list_grove_sessions();
        if discovered.is_empty() {
            return;
        }
        let plan = {
            let existing = self.registry.read(cx).all().to_vec();
            let store = &cx.global::<SettingsState>().store;
            let paths: HashMap<String, String> = store
                .active_projects()
                .map(|(_, p)| (p.name.clone(), p.path.clone()))
                .collect();
            let wt_order = |project: &str| -> Vec<String> {
                paths.get(project).map_or_else(Vec::new, |path| {
                    grove_core::git::list_worktrees(path)
                        .into_iter()
                        .map(|w| w.path)
                        .collect()
                })
            };
            crate::reattach::plan(&discovered, &existing, &wt_order)
        };
        let dims = self.last_pty_dims;
        // Re-arm the never-painted fallback: these sessions attach lazily, and
        // this is the only place new pending ones appear.
        self.tmux_attach_frames = 0;
        for entry in plan {
            let name = entry.session.name.clone();
            // `dims` only seeds the emulator — the tmux client is spawned
            // later, by `TerminalSession::attach_now`, at the dims of the tile
            // that actually paints this session. One pair of dims cannot be
            // right for every tile in grid view, and attaching at the wrong
            // size costs the agent a spurious SIGWINCH one frame later.
            let session = cx.new(|cx| TerminalSession::attach_existing(&name, dims.0, dims.1, cx));
            if let Some(err) = session.read(cx).spawn_error().map(str::to_string) {
                tracing::warn!(session = %name, error = %err, "reattach failed; skipping");
                continue;
            }
            let id = self
                .registry
                .update(cx, |r, _| r.insert_reattached(entry.at, &entry.session));
            self.registry.update(cx, |r, cx| {
                r.attach(id, session, Some(name.clone()));
                cx.notify();
            });
        }
        cx.notify();
    }

    /// Force the deferred tmux attach on every reattached session no tile has
    /// painted yet, at the focused pane's dims. The painted path
    /// (`TerminalElement::prepaint`) is the one that gets the dims right, so
    /// this runs only as a backstop — see `TMUX_ATTACH_FALLBACK_FRAMES`.
    fn attach_pending_tmux_sessions(&mut self, cx: &mut Context<Self>) {
        let dims = self.last_pty_dims;
        let pending: Vec<_> = {
            let registry = self.registry.read(cx);
            registry
                .all()
                .iter()
                .filter_map(|meta| registry.session(meta.id))
                .filter(|session| session.read(cx).is_pending_attach())
                .cloned()
                .collect()
        };
        for session in pending {
            session.update(cx, |session, cx| {
                session.resize(dims.0, dims.1);
                session.attach_now(cx);
            });
        }
    }

    /// `mod+g` (`layout.rs:199-216`).
    fn toggle_grid(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (live, saved) = (self.live_tiles(cx), Self::saved_grid_order(cx));
        self.state.update(cx, |s, cx| {
            s.toggle_grid(&live, &saved);
            cx.notify();
        });
        self.persist_grid_order(cx);
        // Entering the grid must move keyboard focus onto the focused tile —
        // without this, nothing is focused until a click, and the grid
        // move/swap chords (scoped to the Grid context) never fire.
        self.focus_grid_tile(window, cx);
    }

    /// `mod+enter` (`layout.rs:63-103`).
    fn toggle_zen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (live, saved) = (self.live_tiles(cx), Self::saved_grid_order(cx));
        self.state.update(cx, |s, cx| {
            s.toggle_zen(&live, &saved);
            cx.notify();
        });
        self.persist_grid_order(cx);
        // Zen exit can land back in the grid; same focus re-home as
        // `toggle_grid` (a no-op on every other screen).
        self.focus_grid_tile(window, cx);
    }

    /// `mod+t` (`update/mod.rs:472-500`). The transition reports the spawn; the
    /// spawn itself is the view's, exactly as `on_new_home_terminal` is there.
    fn toggle_terminal_tab(&mut self, cx: &mut Context<Self>) {
        let (live, saved) = (self.live_tiles(cx), Self::saved_grid_order(cx));
        let has_home = self.registry.read(cx).home_terminal_count() > 0;
        let outcome = self.state.update(cx, |s, cx| {
            let outcome = s.toggle_terminal_tab(has_home, &live, &saved);
            cx.notify();
            outcome
        });
        self.persist_grid_order(cx);
        if outcome.spawn_home_terminal {
            self.sidebar
                .clone()
                .update(cx, Sidebar::spawn_home_terminal);
        }
    }

    /// The directional grid chords (`update/mod.rs:1071-1116`).
    fn grid_move(
        &mut self,
        dx: i32,
        dy: i32,
        swap: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !swap {
            // `GlobalShortcut::GridMove` only — the swap chord is untracked in
            // iced too (`src/gui/update/mod.rs:1029`).
            crate::telemetry::track("tile_moved", vec![]);
        }
        self.state.update(cx, |s, cx| {
            if swap {
                s.grid_swap(dx, dy);
            } else {
                s.grid_move(dx, dy);
            }
            cx.notify();
        });
        self.persist_grid_order(cx);
        // Mirrors `GridAction::Press`: `grid_focused` and the focused handle
        // must never disagree (carried amendment 7), or the chord that just
        // moved focus silently leaves the keyboard on the old tile.
        let id = self.state.read(cx).grid_focused();
        if let Some(view) = id.and_then(|id| self.views.get(&id)).cloned() {
            let handle = view.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
    }

    /// `mod+N` selects the Nth **visible** session; out of range is a no-op,
    /// not a clamp (`src/gui/update/sessions.rs:394-407`). Inside the grid the
    /// index space is `tile_order`, so the number the user sees on the tile is
    /// the tile they get (`sessions.rs:396-405`).
    fn select_session(
        &mut self,
        action: &keymap::SelectSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let n = action.index.saturating_sub(1);
        if self.state.read(cx).grid_view() {
            self.state.update(cx, |s, cx| {
                s.select_tile_by_index(n);
                cx.notify();
            });
            // Keeps `grid_focused` and gpui's keyboard focus from
            // disagreeing (carried amendment 7) — without this, mod+N
            // updates the highlighted tile but leaves focus on the old one.
            self.focus_grid_tile(window, cx);
            return;
        }
        let Some(&id) = self.visible_order(cx).get(n) else {
            return;
        };
        let snap = self.snapshot(cx);
        // The sidebar's flattened order spans every project, so `mod+N` can
        // cross a project boundary just like a click can.
        let old = self.state.read(cx).proj_idx();
        ProjectTree::adopt_session_project(&self.tree.clone(), &snap, id, old, cx);
        self.state.update(cx, |s, cx| {
            s.select_session(id, &snap);
            cx.notify();
        });
        self.reanchor_panel(window, cx);
    }

    /// `sessions.rs:365-405`.
    fn cycle(&mut self, next: bool, window: &mut Window, cx: &mut Context<Self>) {
        // In grid view, ctrl-shift-j/k are shadowed by the global
        // NextSession/PrevSession bindings (gpui treats no-context bindings
        // as deepest-context, so they fire ahead of the Grid-scoped
        // `GridMove` bindings) and `cycle_session` has no grid concept.
        // Redirect to the grid's own move so the chord still does something.
        if self.state.read(cx).grid_view() {
            self.grid_move(0, if next { 1 } else { -1 }, false, window, cx);
            return;
        }
        let order = self.visible_order(cx);
        let snap = self.snapshot(cx);
        self.state.update(cx, |s, cx| {
            s.cycle_session(next, &order, &snap);
            cx.notify();
        });
        self.reanchor_panel(window, cx);
    }

    /// The first waiting session in visible order (`update/mod.rs:728-739`),
    /// snapped to the live screen **before** it is selected — deliberately
    /// unlike a manual `mod+j/k` switch (`sessions.rs:210-223`). Selecting
    /// acknowledges, and Plan 07's dropdown closes off the same transition
    /// (`:229`).
    fn jump_to_waiting(&mut self, cx: &mut Context<Self>) {
        let waiting = self.activity.read(cx).waiting_sessions().first().copied();
        let Some(id) = waiting.or_else(|| {
            let activity = self.activity.read(cx);
            self.visible_order(cx)
                .into_iter()
                .find(|&id| activity.state_of(id) == ActivityState::WaitingForInput)
        }) else {
            return;
        };
        if let Some(session) = self.registry.read(cx).session(id).cloned() {
            session.update(cx, |s, cx| {
                s.snap_to_bottom();
                cx.notify();
            });
        }
        let snap = self.snapshot(cx);
        self.state.update(cx, |s, cx| {
            s.select_session(id, &snap);
            cx.notify();
        });
    }

    // ── window chrome (Tasks 5 & 6) ─────────────────────────────────────

    /// The single place an appbar/statusbar click becomes a state change.
    /// Everything Plan 07/08 owns logs a stub naming its plan.
    fn chrome(&mut self, action: ChromeAction, _window: &mut Window, cx: &mut Context<Self>) {
        match action {
            ChromeAction::ToggleAttentionQueue => self.state.update(cx, |s, cx| {
                s.toggle_attention_queue();
                cx.notify();
            }),
            ChromeAction::CloseAttentionQueue => self.state.update(cx, |s, cx| {
                s.close_attention_queue();
                cx.notify();
            }),
            ChromeAction::SelectWaiting(id) => self.select_waiting(id, cx),
            // The zen pill is not a dropdown: it jumps straight to the first
            // waiting session (`appbar.rs:277`).
            ChromeAction::JumpToWaiting => self.jump_to_waiting(cx),
            ChromeAction::OpenSessionLauncher => self.open_launcher(cx),
            ChromeAction::OpenSettings => self.open_settings(cx),
            ChromeAction::OpenShortcutOverlay => {
                self.open_modal(crate::modal::Modal::ShortcutOverlay, cx);
            }
        }
    }

    // ── the grid's clicks (Task 4 Steps 2-6) ────────────────────────────

    fn grid_action(&mut self, action: GridAction, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            GridAction::Press(idx) => {
                let id = self.state.read(cx).tile_order().get(idx).copied();
                self.state.update(cx, |s, cx| {
                    s.grid_drag_start(idx);
                    cx.notify();
                });
                // `grid_focused` and the focused handle must never disagree
                // (carried amendment 7): focusing a tile focuses its view.
                if let Some(view) = id.and_then(|id| self.views.get(&id)).cloned() {
                    let handle = view.read(cx).focus_handle(cx);
                    window.focus(&handle, cx);
                }
            }
            GridAction::Focus(idx) => self.state.update(cx, |s, cx| {
                s.grid_focus_tile(idx);
                cx.notify();
            }),
            GridAction::Hover(idx) => self.state.update(cx, |s, cx| {
                s.grid_drag_hover(idx);
                cx.notify();
            }),
            GridAction::TileZen(id) => {
                self.state.update(cx, |s, cx| {
                    s.tile_zen(id);
                    cx.notify();
                });
                self.persist_grid_order(cx);
            }
            GridAction::RequestKill(id) => self.state.update(cx, |s, cx| {
                s.arm_kill(id);
                cx.notify();
            }),
            GridAction::Kill(id) => self.kill_session(id, window, cx),
        }
    }

    /// The kill half of the two-step confirm, shared by the tile header and
    /// the session bar. `on_session_removed` + a grid reconcile keep
    /// `tile_order` honest (`layout.rs:276-306`).
    fn kill_session(&mut self, id: SessionId, window: &mut Window, cx: &mut Context<Self>) {
        self.registry.update(cx, |r, cx| {
            r.remove(id);
            cx.notify();
        });
        self.views.remove(&id);
        let (live, saved) = (self.live_tiles(cx), Self::saved_grid_order(cx));
        self.state.update(cx, |s, cx| {
            s.on_session_removed(id);
            s.disarm_kill();
            s.reconcile_after_teardown(&live, &saved);
            cx.notify();
        });
        self.persist_grid_order(cx);
        self.focus_grid_tile(window, cx);
    }

    /// Mirrors `GridAction::Press` / `grid_move`: `grid_focused` and the
    /// focused handle must never disagree (carried amendment 7). Killing the
    /// focused tile updates `grid_focused` via `reconcile_after_teardown`
    /// but leaves gpui's keyboard focus stranded on the dead view unless we
    /// re-home it here. A no-op outside the grid — the render-time re-home
    /// (~workspace.rs:1690-1704) already covers zen/workspace screens.
    fn focus_grid_tile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.state.read(cx).grid_view() {
            return;
        }
        let id = self.state.read(cx).grid_focused();
        let view = id.and_then(|id| self.agent_view(id, cx));
        if let Some(view) = view {
            let handle = view.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
    }

    // ── the session bar's tool cluster (Task 5 Step 4) ──────────────────

    fn tool_action(&mut self, action: ToolAction, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            // `on_run_script` (`src/gui/update/sessions.rs:147-177`): the run
            // script opens the terminal panel (if closed) for the active
            // worktree, then spawns the project's `run` script in it — same
            // as the launcher palette's `ModalEvent::RunScript` (`:1638`).
            ToolAction::RunScript => {
                let Some((wt_path, script)) = self.active_run_script(cx) else {
                    return;
                };
                if !self.state.read(cx).term_panel_open() {
                    self.state.update(cx, |s, cx| {
                        s.toggle_term_panel(true);
                        cx.notify();
                    });
                }
                self.spawn_wt_script(&wt_path, &script, cx);
            }
            ToolAction::ToggleTermPanel => self.toggle_term_panel(window, cx),
            ToolAction::ToggleZen => self.toggle_zen(window, cx),
            ToolAction::RequestKill => {
                let Some(id) = self.state.read(cx).active_session() else {
                    return;
                };
                self.state.update(cx, |s, cx| {
                    s.arm_kill(id);
                    cx.notify();
                });
            }
            ToolAction::Kill => {
                let Some(id) = self.state.read(cx).active_session() else {
                    return;
                };
                self.kill_session(id, window, cx);
            }
            ToolAction::OpenDiff => {
                let Some(wt_path) = self.active_wt_path(cx) else {
                    return;
                };
                self.modals.clone().update(cx, |l, cx| {
                    l.open(crate::modal::Modal::DiffViewer { wt_path }, cx);
                });
            }
        }
    }

    // ── the home-terminal tab (Task 5 Step 3) ───────────────────────────

    fn tab_action(
        &mut self,
        action: TerminalTabAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            TerminalTabAction::ToggleZen => self.toggle_zen(window, cx),
            TerminalTabAction::Restart => self.restart_home_terminal(cx),
        }
    }

    /// Replace the active home terminal's shell in place, keeping its slot and
    /// label, and **only once the replacement is live** — a failed spawn toasts
    /// and leaves the (usually exited) shell where it was
    /// (`src/app/terminals.rs:38-53,95-108`).
    fn restart_home_terminal(&mut self, cx: &mut Context<Self>) {
        let Some(i) = self.state.read(cx).active_terminal() else {
            return;
        };
        let Some(meta) = self.registry.read(cx).home_terminals().get(i).cloned() else {
            return;
        };
        let target = crate::entities::session_registry::SpawnTarget::home(meta.label.clone());
        let session = cx.new(|cx| TerminalSession::spawn(&target, &[], None, cx));
        if let Some(err) = session.read(cx).spawn_error().map(str::to_string) {
            self.toast.update(cx, |t, cx| {
                t.set_error(format!("terminal failed: {err}"), cx);
            });
            return;
        }
        let old = self.registry.update(cx, |r, cx| {
            let old = r.replace_home(i, session);
            cx.notify();
            old
        });
        if old.is_some() {
            // Dropping the old entity ends its PTY; its cached view must go
            // with it or the tab would keep rendering the dead shell.
            self.home_views.remove(&meta.id);
        }
        drop(old);
    }

    // ── the worktree panel (Task 6) ─────────────────────────────────────

    /// The active session's worktree — the panel's scope
    /// (`pty_input.rs:220-226`).
    fn active_wt_path(&self, cx: &App) -> Option<String> {
        let id = self.state.read(cx).active_session()?;
        self.registry.read(cx).meta(id).map(|m| m.wt_path.clone())
    }

    /// The active session's worktree path paired with its project's non-blank
    /// `run` script — the single source of truth for both the header's ▶
    /// button visibility and what that button executes.
    fn active_run_script(&self, cx: &App) -> Option<(String, String)> {
        let id = self.state.read(cx).active_session()?;
        let meta = self.registry.read(cx).meta(id)?;
        let wt_path = meta.wt_path.clone();
        let store = &cx.global::<SettingsState>().store;
        let script = grove_core::storage::project_for_worktree_path(&store.projects, &wt_path)
            .and_then(|(_, p)| p.scripts.run.clone())
            .filter(|s| !s.trim().is_empty())?;
        Some((wt_path, script))
    }

    fn toggle_term_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let wt = self.active_wt_path(cx);
        let opened = self.state.update(cx, |s, cx| {
            let opened = s.toggle_term_panel(wt.is_some());
            cx.notify();
            opened
        });
        if !opened {
            return;
        }
        // `ensure_wt_terminal` (`src/app/terminals.rs:133-149`): the panel
        // spawns its first shell on demand.
        if let Some(wt) = wt {
            if self.registry.read(cx).wt_shells_need_spawn(&wt) {
                self.spawn_wt_shell(&wt, cx);
            }
        }
        // Focusing the just-opened panel is the natural default — that is why
        // the user opened it (`sessions.rs:80-84`). With no shell to focus,
        // focus stays on the agent, which is the `pty_input.rs:170-178`
        // fallback made literal.
        self.focus_panel(window, cx);
    }

    /// Swap the sidebar rail between the project tree and the session list,
    /// persisting the choice like the sidebar width. The keyboard path for
    /// `keymap::ToggleRailMode`; the rail button reaches the same
    /// `WorkspaceState` toggle through `RowAction::ToggleRailMode` in
    /// `sidebar.rs`, which owns the state entity directly.
    fn toggle_rail_mode(&mut self, cx: &mut Context<Self>) {
        let mode = self.state.update(cx, |s, cx| {
            let mode = s.toggle_rail_mode();
            cx.notify();
            mode
        });
        SettingsState::update(cx, |s| s.rail_sessions = mode == RailMode::Sessions);
    }

    /// Move the gpui focus onto the panel's active shell, if there is one.
    fn focus_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.panel_view(cx) else {
            return;
        };
        let handle = view.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
    }

    /// The active session changed, so the panel re-anchors to the new
    /// session's remembered open/closed state (`term_panel_open` is per
    /// session — see `WorkspaceState::panel_open_sessions`): `reset_focused_pane`
    /// picks the intent (`pty_input.rs:128-137`) and the matching handle
    /// takes the gpui focus. A worktree with no shell falls back to the agent
    /// (`:170-178`), and so does a session whose panel was left closed.
    fn reanchor_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.read(cx).term_panel_open() {
            // The newly anchored worktree spawns its first shell on demand,
            // exactly as opening the panel does (`ensure_wt_terminal`).
            if let Some(wt) = self.active_wt_path(cx) {
                if self.registry.read(cx).wt_shells_need_spawn(&wt) {
                    self.spawn_wt_shell(&wt, cx);
                }
            }
        }
        self.state.update(cx, |s, cx| {
            s.reset_focused_pane();
            cx.notify();
        });
        if self.state.read(cx).term_panel_open() && self.panel_view(cx).is_some() {
            self.focus_panel(window, cx);
        } else {
            self.focus_agent(window, cx);
        }
    }

    /// Move the gpui focus back onto the agent side.
    fn focus_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.body_view(cx) else {
            return;
        };
        let handle = view.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
    }

    fn panel_action(&mut self, action: PanelAction, window: &mut Window, cx: &mut Context<Self>) {
        let Some(wt) = self.active_wt_path(cx) else {
            return;
        };
        match action {
            PanelAction::NewShell => {
                self.spawn_wt_shell(&wt, cx);
                self.focus_panel(window, cx);
            }
            PanelAction::SelectShell(i) => {
                self.registry.update(cx, |r, cx| {
                    r.select_wt_shell(&wt, i);
                    cx.notify();
                });
                self.state.update(cx, |s, cx| {
                    s.focus_pane(PtyPane::Panel);
                    cx.notify();
                });
                self.focus_panel(window, cx);
            }
            PanelAction::CloseShell(i) => self.close_panel_shell(i, window, cx),
            PanelAction::Collapse => self.toggle_term_panel(window, cx),
            PanelAction::DividerPress => self.term_divider_press(cx),
        }
    }

    /// Close the panel shell at `idx` in the active worktree — the ✕ tab
    /// button's handler and the keyboard `mod+w` path both funnel through
    /// here, so they never diverge. When that was the last shell in the
    /// worktree, the panel collapses (`toggle_term_panel`, which also moves
    /// the focus intent back to the agent) instead of sitting open on the
    /// empty-panel fallback.
    fn close_panel_shell(&mut self, idx: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(wt) = self.active_wt_path(cx) else {
            return;
        };
        let removed = self.registry.update(cx, |r, cx| {
            let removed = r.close_wt_shell(&wt, idx);
            cx.notify();
            removed
        });
        // Dropping the entity ends its PTY: the reader task and the
        // `PtyHandle` both die with it.
        drop(removed);
        if self.registry.read(cx).wt_shells(&wt).is_empty() {
            self.toggle_term_panel(window, cx);
            return;
        }
        // Whatever filled the closed slot — or the agent, if nothing did
        // (`pty_input.rs:170-178`) — takes the input.
        if self.panel_view(cx).is_some() {
            self.focus_panel(window, cx);
        } else {
            self.focus_agent(window, cx);
        }
    }

    /// A press on the split divider arms a drag — or, within 350ms of the
    /// previous press, resets the portion to its 40% default instead, and does
    /// **not** also start a drag (`layout.rs:162-176`, the same double-click
    /// idiom the sidebar divider uses).
    fn term_divider_press(&mut self, cx: &mut Context<Self>) {
        let now = std::time::Instant::now();
        let double = self
            .last_term_divider_press
            .is_some_and(|t| now.duration_since(t) < std::time::Duration::from_millis(350));
        if double {
            self.term_panel_dragging = false;
            self.last_term_divider_press = None;
            self.state.update(cx, |s, cx| {
                s.set_term_panel_portion(crate::entities::workspace_state::TERM_PANEL_PORTION);
                cx.notify();
            });
        } else {
            self.last_term_divider_press = Some(now);
            self.term_panel_dragging = true;
        }
    }

    /// The divider drag's move/release, listened for at the root so the pointer
    /// can leave the 6px zone (`layout.rs:178-197`).
    fn on_root_mouse_move(&mut self, x: f32, cx: &mut Context<Self>) {
        if !self.term_panel_dragging {
            return;
        }
        let (win_w, sidebar_w) = (self.logical_win_w, self.state.read(cx).sidebar_width());
        self.state.update(cx, |s, cx| {
            s.set_term_panel_portion(term_portion_for_cursor(x, win_w, sidebar_w));
            cx.notify();
        });
    }

    fn on_root_mouse_up(&mut self, cx: &mut Context<Self>) {
        self.term_panel_dragging = false;
        self.state.update(cx, |s, cx| {
            s.grid_drag_end();
            cx.notify();
        });
        self.persist_grid_order(cx);
    }

    /// Spawn a panel shell rooted at the worktree and focus it. Native, not
    /// tmux-pinned: these are convenience shells (`Agent::Terminal`), so
    /// `attention::prepare` returns `None` and there is nothing to thread down.
    fn spawn_wt_shell(&mut self, wt_path: &str, cx: &mut Context<Self>) {
        let (id, label) = self
            .registry
            .update(cx, |r, _| (r.next_home_id(), r.next_wt_label()));
        let target = crate::entities::session_registry::SpawnTarget {
            cwd: wt_path.to_string(),
            agent: grove_core::agent::Agent::Terminal,
            project: String::new(),
            label: label.clone(),
            args: Vec::new(),
            use_tmux: false,
        };
        let session = cx.new(|cx| TerminalSession::spawn(&target, &[], None, cx));
        if let Some(err) = session.read(cx).spawn_error().map(str::to_string) {
            self.toast.update(cx, |t, cx| {
                t.set_error(format!("terminal failed: {err}"), cx);
            });
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
        self.registry.update(cx, |r, cx| {
            r.push_wt_shell(wt_path, meta, Some(session));
            cx.notify();
        });
        self.state.update(cx, |s, cx| {
            s.focus_pane(PtyPane::Panel);
            cx.notify();
        });
    }

    /// Spawn a one-shot script as a panel shell rooted at the worktree, and
    /// focus it. Identical to [`Self::spawn_wt_shell`] except the PTY runs
    /// `script` instead of an interactive login shell — the palette strip's
    /// lifecycle-script rows (`src/views/modals/launcher.rs`, `row_actions`)
    /// route through here since gpui has no `spawn_script_session`
    /// equivalent to the iced original.
    fn spawn_wt_script(&mut self, wt_path: &str, script: &str, cx: &mut Context<Self>) {
        crate::views::scripts::spawn_wt_script(
            &self.registry,
            &self.state,
            Some(&self.toast),
            wt_path,
            script,
            cx,
        );
    }

    /// The panel's active shell view, memoized per shell id exactly as the
    /// agent views are.
    fn panel_view(&mut self, cx: &mut Context<Self>) -> Option<Entity<TerminalView>> {
        let wt = self.active_wt_path(cx)?;
        let registry = self.registry.read(cx);
        let idx = registry.active_wt_shell_idx(&wt)?;
        let id = registry.wt_shells(&wt).get(idx)?.id;
        let session = registry.wt_shell(&wt, idx)?.clone();
        if let Some(view) = self.panel_views.get(&id) {
            return Some(view.clone());
        }
        let clock = self.clock.clone();
        let chrome = self.state.clone();
        let view = cx.new(|cx| TerminalView::new(session, None, clock, cx).with_chrome(chrome));
        self.panel_views.insert(id, view.clone());
        Some(view)
    }

    /// A dropdown row: snap to the live screen first, then select — which
    /// acknowledges and closes the dropdown (`sessions.rs:210-223,229`).
    fn select_waiting(&mut self, id: SessionId, cx: &mut Context<Self>) {
        if let Some(session) = self.registry.read(cx).session(id).cloned() {
            session.update(cx, |s, cx| {
                s.snap_to_bottom();
                cx.notify();
            });
        }
        let snap = self.snapshot(cx);
        self.state.update(cx, |s, cx| {
            s.select_session(id, &snap);
            cx.notify();
        });
    }

    /// The attention queue, resolved **once** per frame and shared by the pill
    /// and the dropdown (Task 4 Step 5).
    fn waiting_rows(&self, cx: &App) -> Vec<WaitingRow> {
        let activity = self.activity.read(cx);
        let registry = self.registry.read(cx);
        activity
            .waiting_sessions()
            .iter()
            .filter_map(|&id| {
                let meta = registry.meta(id)?;
                Some(WaitingRow {
                    id,
                    agent_label: meta.agent.label(),
                    project: meta.project.clone(),
                    wt_path: meta.wt_path.clone(),
                    state: activity.state_of(id),
                })
            })
            .collect()
    }

    /// The header for whatever the body is showing. Parameterized by session so
    /// Plan 07 reuses it per grid tile.
    fn header_data(
        &self,
        snap: &crate::entities::workspace_state::TreeSnapshot,
        cx: &App,
    ) -> Option<SessionHeaderData> {
        let ws = self.state.read(cx);
        let (terminal_focused, active_terminal, active_session) = (
            ws.terminal_focused(),
            ws.active_terminal(),
            ws.active_session(),
        );
        let registry = self.registry.read(cx);
        let (meta, entity, header_label) = if terminal_focused {
            let i = active_terminal?;
            let meta = registry.home_terminals().get(i)?.clone();
            let entity = registry.home_terminal(i)?.clone();
            let label = meta.label.clone();
            (meta, entity, label)
        } else {
            let id = active_session?;
            let meta = registry.meta(id)?.clone();
            let entity = registry.session(id)?.clone();
            let label = meta.agent.label().to_string();
            (meta, entity, label)
        };
        let title = entity.read(cx).title();
        let context = title.as_deref().and_then(|raw| {
            if terminal_focused {
                rows::terminal_context(raw, &meta.label)
            } else {
                rows::session_context(
                    raw,
                    &rows::path_basename(&meta.wt_path),
                    &meta.label,
                    meta.agent.label(),
                )
            }
        });
        // Branchless sessions (home terminals) find no worktree and skip the
        // segment entirely (`terminal.rs:530-535`).
        let branch = snap
            .projects
            .iter()
            .flat_map(|p| p.worktrees.iter())
            .find(|w| w.path == meta.wt_path)
            .map_or_else(String::new, |w| w.branch.clone());
        let state = self.activity.read(cx).state_of(meta.id);
        // Same join `rows::flatten_sessions` does: both sides normalized so a
        // trailing slash cannot manufacture a miss (`rows.rs:~577`).
        let git = self.tree.read(cx).git_states();
        let diff = git
            .get(rows::normalize_wt_path(&meta.wt_path))
            .map(|g| (g.added, g.removed));
        Some(SessionHeaderData {
            label: header_label,
            branch,
            context,
            icon_name: meta.agent.icon_name(),
            running: state != ActivityState::Exited,
            diff,
        })
    }

    /// Arms the two-step confirm on whatever is focused, or confirms it on a
    /// second press on the same target (`shortcuts.rs:501-527`'s
    /// `close_focused_session_decision`). A different target re-arms; no
    /// target is a no-op — this never quits the app.
    fn close_focused(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ws = self.state.read(cx);
        let (terminal_focused, active_terminal) = (ws.terminal_focused(), ws.active_terminal());
        if terminal_focused {
            let Some(i) = active_terminal else { return };
            let kill = self.state.update(cx, |s, cx| {
                let kill = s.close_focused_terminal(i);
                cx.notify();
                kill
            });
            if kill {
                self.sidebar
                    .clone()
                    .update(cx, |sb, cx| sb.close_home_terminal(i, cx));
            }
            return;
        }
        let grid_view = ws.grid_view();
        // A panel shell — focused via `FocusSidePanel` or a mouse click —
        // takes `mod+w` too, the same target the ✕ tab button closes and with
        // the same no-confirmation behavior. Agent sessions keep their
        // two-step confirm below, untouched.
        if !grid_view {
            let has_panel_shell = self.panel_view(cx).is_some();
            if self.state.read(cx).input_target(has_panel_shell) == PtyPane::Panel {
                if let Some(wt) = self.active_wt_path(cx) {
                    if let Some(idx) = self.registry.read(cx).active_wt_shell_idx(&wt) {
                        self.close_panel_shell(idx, window, cx);
                    }
                }
                return;
            }
        }
        let ws = self.state.read(cx);
        let target = if grid_view {
            ws.grid_focused().or_else(|| ws.active_session())
        } else {
            ws.active_session()
        };
        let Some(id) = target else { return };
        let kill = self.state.update(cx, |s, cx| {
            let kill = s.close_focused_session(id);
            cx.notify();
            kill
        });
        if kill {
            self.kill_session(id, window, cx);
        }
    }

    fn scroll_half_page(&mut self, up: bool, cx: &mut Context<Self>) {
        let Some(session) = self.active_session_entity(cx) else {
            return;
        };
        session.update(cx, |s, cx| {
            let lines = s.scroll_page_lines() / 2;
            s.scroll_lines(up, lines);
            cx.notify();
        });
    }

    fn active_session_entity(&self, cx: &App) -> Option<Entity<TerminalSession>> {
        let ws = self.state.read(cx);
        let registry = self.registry.read(cx);
        if ws.terminal_focused() {
            return ws
                .active_terminal()
                .and_then(|i| registry.home_terminal(i))
                .cloned();
        }
        ws.active_session()
            .and_then(|id| registry.session(id))
            .cloned()
    }

    // ── the four screens' bodies (Tasks 4-6) ────────────────────────────

    // ── the modal layer (Plan 08) ───────────────────────────────────────

    /// The single entry point for opening a modal from the workspace. Opening
    /// replaces whatever was open; there is no stack.
    pub(crate) fn open_modal(&mut self, modal: crate::modal::Modal, cx: &mut Context<Self>) {
        self.modals.clone().update(cx, |l, cx| l.open(modal, cx));
    }

    /// `Msg::Open` — mod+p's plain palette (`src/gui/update/palette.rs:15-19`).
    fn open_launcher(&mut self, cx: &mut Context<Self>) {
        crate::telemetry::track("launcher_opened", vec![]);
        self.open_modal(crate::modal::Modal::SessionLauncher(Box::default()), cx);
    }

    /// mod+s (`GlobalShortcut::SwitchSession`, `update/mod.rs:909-922`): opens
    /// the palette straight into its switch drill-in, cleared query and row 0
    /// (`launcher_enter_switch`, `session_launcher/palette.rs:356-361`).
    ///
    /// Zen-only, and only with something to switch *to* — `switch_to_session_
    /// active` (`palette.rs:687-689`): outside zen the workspace/grid already
    /// shows every session, so the drill-in would be redundant. Otherwise a
    /// runtime no-op.
    fn open_switch_drill_in(&mut self, cx: &mut Context<Self>) {
        let ws = self.state.read(cx);
        let (zen, active) = (ws.zen(), ws.active_session());
        let registry = self.registry.read(cx);
        // `switch_to_session_row_visible` (`palette.rs:670-680`): any session
        // other than the active one, or any home terminal at all.
        let row_visible = registry.all().iter().any(|m| Some(m.id) != active)
            || !registry.home_terminals().is_empty();
        if !zen || !row_visible {
            return;
        }
        crate::telemetry::track("launcher_opened", vec![]);
        self.open_modal(
            crate::modal::Modal::SessionLauncher(Box::new(crate::modal::LauncherSlotState {
                view: crate::modal::LauncherView::Switch,
                ..Default::default()
            })),
            cx,
        );
    }

    /// alt-chord+n (`GlobalShortcut::NewSessionInWorktree`,
    /// `update/mod.rs:923-943`). Not a palette entry point at all: it launches
    /// straight into the focused session's own project/worktree via
    /// `launch_or_pick` (`src/app/spawn.rs:18-48`) — the configured default
    /// agent if it is still on PATH, otherwise the agent picker (with a toast
    /// when a saved default has gone missing).
    fn new_session_in_worktree(&mut self, cx: &mut Context<Self>) {
        let ws = self.state.read(cx);
        let (terminal_focused, active_terminal, active_session) = (
            ws.terminal_focused(),
            ws.active_terminal(),
            ws.active_session(),
        );
        let registry = self.registry.read(cx);
        let meta = if terminal_focused {
            active_terminal.and_then(|i| registry.home_terminals().get(i))
        } else {
            active_session.and_then(|id| registry.meta(id))
        };
        let Some((project, wt_path)) = meta.map(|m| (m.project.clone(), m.wt_path.clone())) else {
            return;
        };
        let available = crate::views::modals::confirm::available_agents();
        let saved = cx.global::<SettingsState>().store.default_agent;
        if let Some(agent) = saved.filter(|a| available.contains(a)) {
            let snap = self.snapshot(cx);
            let Some(p) = snap.projects.iter().find(|p| p.name == project) else {
                return;
            };
            let Some(wt) = p.worktrees.iter().position(|w| w.path == wt_path) else {
                return;
            };
            let proj = p.idx;
            self.sidebar
                .clone()
                .update(cx, |s, cx| s.spawn_session(proj, wt, agent, cx));
            return;
        }
        if let Some(saved) = saved {
            let msg = format!("{} not found; pick an agent", saved.label());
            self.toast.update(cx, |t, cx| t.set_error(msg, cx));
        }
        let sel = saved
            .and_then(|a| available.iter().position(|&x| x == a))
            .unwrap_or(0);
        self.open_modal(
            crate::modal::Modal::AgentPicker {
                project,
                wt_path,
                sel,
            },
            cx,
        );
    }

    /// `on_open_settings` (`src/gui/update/mod.rs:551-555`).
    fn open_settings(&mut self, cx: &mut Context<Self>) {
        crate::telemetry::track("settings_opened", vec![]);
        self.open_modal(crate::modal::Modal::Settings, cx);
    }

    /// Effects the layer cannot perform for itself.
    fn on_modal_event(
        &mut self,
        _layer: Entity<ModalLayer>,
        event: &ModalEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            ModalEvent::Quit => {
                // Exit path 2 of 3 (`submit_modal_confirm` →
                // `iced::exit()`, `src/gui/update/modals.rs:558-576`).
                self.shutdown(cx);
                cx.quit();
            }
            // The slot is empty and the layer is about to be unmounted, taking
            // its focus handle out of the element tree. Hand the keyboard back
            // to the body on the next frame (see `focused_once`).
            ModalEvent::Closed => {
                self.focused_once = false;
                self.last_body_focused = None;
                cx.notify();
            }
            // The Settings tmux row (or the first-run choice) switched the
            // backend on: re-scan, exactly as `discover_tmux_sessions` does
            // in iced (`src/app/mod.rs:288-292,347-366`).
            ModalEvent::TmuxEnabled => self.discover_tmux_sessions(cx),
            ModalEvent::RestartApp => self.restart_after_update(cx),
            ModalEvent::SpawnAgent {
                project,
                wt_path,
                agent,
            } => {
                // The palette can launch into a project whose worktree cache is
                // cold, so resolving through the tree snapshot dropped the launch
                // on the floor (`ProjectTree::snapshot` only carries the active
                // project's worktrees). Take the emitted path at face value; warm
                // the tree so the new session's row is visible, exactly as
                // `RowAction::SelectProject` does.
                let (project, wt_path, agent) = (project.clone(), wt_path.clone(), *agent);
                let old = self.state.read(cx).proj_idx();
                let target = {
                    let store = &cx.global::<SettingsState>().store;
                    store
                        .projects
                        .iter()
                        .position(|p| p.name == project)
                        .map(|i| (i, store.projects[i].path.clone()))
                };
                if let Some((proj, path)) = target {
                    if proj != old {
                        self.tree.update(cx, |t, cx| {
                            t.switch_active_project(old, proj, &path);
                            cx.notify();
                        });
                    }
                }
                self.sidebar
                    .clone()
                    .update(cx, |s, cx| s.spawn_session_in(project, wt_path, agent, cx));
            }
            ModalEvent::NewHomeTerminal => {
                self.sidebar.clone().update(cx, Sidebar::new_home_terminal);
            }
            ModalEvent::SelectSession(id) => {
                let snap = self.snapshot(cx);
                let id = *id;
                self.state.update(cx, |s, cx| {
                    s.select_session(id, &snap);
                    cx.notify();
                });
            }
            ModalEvent::SelectTerminal(i) => {
                let count = self.registry.read(cx).home_terminal_count();
                let i = *i;
                self.state.update(cx, |s, cx| {
                    s.select_home_terminal(i, count);
                    cx.notify();
                });
            }
            // The palette strip's lifecycle-script rows (`launcher.rs`, the
            // strip): open the worktree's terminal panel if it is closed,
            // then run the script as a shell in it.
            ModalEvent::RunScript { wt_path, script } => {
                if !self.state.read(cx).term_panel_open() {
                    self.state.update(cx, |s, cx| {
                        s.toggle_term_panel(true);
                        cx.notify();
                    });
                }
                self.spawn_wt_script(wt_path, script, cx);
            }
            ModalEvent::WorktreeAdded | ModalEvent::TreeInvalidated => {
                let idx = self.state.read(cx).proj_idx();
                let active = cx
                    .global::<SettingsState>()
                    .store
                    .projects
                    .get(idx)
                    .map(|p| p.path.clone());
                self.tree.clone().update(cx, |t, cx| {
                    t.rebuild_wt_cache();
                    if let Some(path) = active {
                        t.set_active_worktrees(idx, grove_core::git::list_worktrees(&path));
                    }
                    cx.notify();
                });
                cx.notify();
            }
        }
    }

    /// A dispatch closure of any action kind, routed back into `self`.
    #[allow(clippy::type_complexity)]
    fn dispatcher<A: 'static>(
        &self,
        cx: &mut Context<Self>,
        f: impl Fn(&mut Self, A, &mut Window, &mut Context<Self>) + 'static,
    ) -> std::rc::Rc<dyn Fn(A, &mut Window, &mut App)> {
        let weak = cx.entity().downgrade();
        std::rc::Rc::new(move |action, window, cx: &mut App| {
            let _ = weak.update(cx, |this: &mut Self, cx| f(this, action, window, cx));
        })
    }

    /// The tiles, resolved once per frame. Each hosts the **same**
    /// `TerminalView` entity the single-session body would (amendment 7).
    fn tile_data(
        &mut self,
        snap: &crate::entities::workspace_state::TreeSnapshot,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<TileData> {
        let (order, focused, pending_kill, sidebar_w) = {
            let ws = self.state.read(cx);
            (
                ws.tile_order().to_vec(),
                ws.grid_focused(),
                ws.pending_kill(),
                ws.sidebar_width(),
            )
        };
        // The grid's *real* width per tile, in **device px**. Unlike
        // `GridCtx::tile_size` (which feeds the slide's draw offset and must
        // keep `grid_tile_size`'s sidebar-blind geometry), the header budget
        // has to reflect the width a tile is actually given: sidebar
        // subtracted, and `grid`'s inter-column `gap(px(1.0))` — a true device
        // pixel, not a token — taken off before the divide.
        let tile_w_px = {
            let (cols, _) = crate::grid::grid_layout(order.len());
            let cols_f = f32::from(u16::try_from(cols).unwrap_or(u16::MAX));
            let body_w = f32::from(window.viewport_size().width) - token_px(sidebar_w, window);
            ((body_w - (cols_f - 1.0)) / cols_f).max(0.0)
        };
        // A render-time memo, so it must not accumulate dead sessions.
        self.header_fits.retain(|id, _| order.contains(id));
        // Read once per frame, not once per tile — same join `header_data`
        // does, hoisted out of the loop below (`rows.rs:~577`).
        let git = self.tree.read(cx).git_states();
        let mut out = Vec::with_capacity(order.len());
        for (tile_idx, id) in order.into_iter().enumerate() {
            let Some(meta) = self.registry.read(cx).meta(id).cloned() else {
                continue;
            };
            let Some(view) = self.agent_view(id, cx) else {
                continue;
            };
            let branch = snap
                .projects
                .iter()
                .flat_map(|p| p.worktrees.iter())
                .find(|w| w.path == meta.wt_path)
                .map_or_else(String::new, |w| w.branch.clone());
            // Same derivation as `header_data` — a tile and the session bar
            // must never disagree about what a session is doing.
            let context = self.registry.read(cx).session(id).and_then(|entity| {
                let title = entity.read(cx).title();
                title.as_deref().and_then(|raw| {
                    rows::session_context(
                        raw,
                        &rows::path_basename(&meta.wt_path),
                        &meta.label,
                        meta.agent.label(),
                    )
                })
            });
            let waiting = self.activity.read(cx).state_of(id) == ActivityState::WaitingForInput;
            let agent_label = meta.agent.label();
            let seg = grid::SegmentWidths {
                project: segment_px(window, &meta.project),
                branch: segment_px(window, &branch),
                title: segment_px(window, context.as_deref().unwrap_or_default()),
            };
            let budget = header_budget_px(window, tile_w_px, tile_idx, waiting, agent_label);
            let fit = grid::fit_segments(budget, &seg, self.header_fits.get(&id).copied());
            self.header_fits.insert(id, fit);
            let diff = git
                .get(rows::normalize_wt_path(&meta.wt_path))
                .map(|g| (g.added, g.removed));
            out.push(TileData {
                id,
                agent_label,
                icon_name: meta.agent.icon_name(),
                project: meta.project.clone(),
                branch,
                waiting,
                focused: focused == Some(id),
                confirming_kill: pending_kill == Some(id),
                context,
                running: self.activity.read(cx).state_of(id) != ActivityState::Exited,
                diff,
                view,
                fit,
            });
        }
        out
    }

    /// The session column: its bar atop its PTY, split with the worktree panel
    /// when that is open (`terminal.rs:181-229`). **Never** reached in grid
    /// view — `workspace()` returns `grid_workspace()` first (`:182-184`).
    fn session_body(
        &mut self,
        header: Option<SessionHeaderData>,
        tick: u64,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let tool_dispatch = self.dispatcher(cx, |this, action: ToolAction, window, cx| {
            this.tool_action(action, window, cx);
        });
        let has_run_script = self.active_run_script(cx).is_some();
        let ws = self.state.read(cx);
        let (term_panel_open, chrome_visible, portion, pending_kill, active) = (
            ws.term_panel_open(),
            ws.chrome_visible(),
            ws.term_panel_portion(),
            ws.pending_kill(),
            ws.active_session(),
        );
        let cluster = ToolCluster {
            has_run_script,
            term_panel_open,
            chrome_visible,
            confirming_kill: active.is_some_and(|id| pending_kill == Some(id)),
            dispatch: tool_dispatch,
        };
        let body = self.body_view(cx);
        if header.is_none() && body.is_none() {
            // No active session: mirror iced's `empty_workspace()` /
            // `empty_no_projects_workspace()` (`src/gui/widgets/primitives.rs:222-276`),
            // reusing the grid's shared "nothing here" panel (`views/grid.rs:117-129`)
            // so the chrome matches the grid's own zero-tile empty state.
            let has_active_projects = cx
                .global::<SettingsState>()
                .store
                .active_projects()
                .next()
                .is_some();
            let empty = if has_active_projects {
                grid::empty_state(
                    "no session selected",
                    "click a worktree's start button to spawn an agent",
                )
            } else {
                grid::empty_state(
                    "No active projects",
                    "Add or restore a project to get started.",
                )
            };
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .h_full()
                .overflow_hidden()
                .bg(c::BG())
                .child(empty)
                .into_any_element();
        }
        let column = div()
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .overflow_hidden()
            .bg(c::BG())
            .when_some(header, |d, h| {
                d.child(session_header::session_header(&h, tick, Some(&cluster)))
            })
            .child(
                // Whatever the chrome costs in height comes out of the
                // terminal's rows for free — the element derives its dims from
                // its own bounds in `prepaint` (Plan 04 amendment 7).
                div()
                    .flex()
                    .flex_1()
                    .w_full()
                    .overflow_hidden()
                    // iced's `pty()` container padding, reproduced from the
                    // `compute_pty_dims` fudge constant so the cell grid
                    // matches (`views::grid::PTY_PAD_W` carries the full
                    // derivation and the `src/gui/metrics.rs:21-22` citation).
                    .px(rpx(PTY_PAD_W / 2.0))
                    .py(rpx(PTY_PAD_H / 2.0))
                    // A click on the agent PTY moves the input intent back to
                    // the agent (`focus_pane`, `pty_input.rs:146-158`); the
                    // keystrokes themselves follow gpui focus, which the
                    // child's own press already took.
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _: &gpui::MouseDownEvent, _, cx| {
                            this.state.update(cx, |s, cx| {
                                s.focus_pane(PtyPane::Agent);
                                cx.notify();
                            });
                        }),
                    )
                    .when_some(body, gpui::ParentElement::child),
            );

        let wt = self.active_wt_path(cx);
        if !term_panel_open || wt.is_none() {
            return column.into_any_element();
        }
        let panel_dispatch = self.dispatcher(cx, |this, action: PanelAction, window, cx| {
            this.panel_action(action, window, cx);
        });
        let panel_view = self.panel_view(cx);
        let tabs = wt.map_or_else(Vec::new, |wt| {
            let registry = self.registry.read(cx);
            let active = registry.active_wt_shell_idx(&wt);
            registry
                .wt_shells(&wt)
                .iter()
                .enumerate()
                .map(|(i, meta)| ShellTab {
                    running: self.activity.read(cx).state_of(meta.id) != ActivityState::Exited,
                    active: active == Some(i),
                })
                .collect()
        });
        let panel_ctx = PanelCtx {
            tabs,
            view: panel_view,
            dispatch: std::rc::Rc::clone(&panel_dispatch),
        };
        // Proportional flex weights, so the ratio is the single source of
        // truth exactly as iced's `FillPortion` makes it.
        div()
            .flex()
            .flex_row()
            .flex_1()
            .w_full()
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .flex_basis(px(0.0))
                    .flex_grow(f32::from(100 - portion))
                    .h_full()
                    .overflow_hidden()
                    .child(column),
            )
            .child(term_panel::divider(&panel_dispatch))
            .child(
                div()
                    .flex()
                    .flex_basis(px(0.0))
                    .flex_grow(f32::from(portion))
                    .h_full()
                    .overflow_hidden()
                    // The mirror image: clicking the panel returns input to it.
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _: &gpui::MouseDownEvent, _, cx| {
                            this.state.update(cx, |s, cx| {
                                s.focus_pane(PtyPane::Panel);
                                cx.notify();
                            });
                        }),
                    )
                    .child(term_panel::term_panel(&panel_ctx)),
            )
            .into_any_element()
    }

    // ── the body follows the selection (Task 6 Step 2) ──────────────────

    /// The view for whatever is active, minted once per session and cached, so
    /// switching never respawns a PTY.
    fn body_view(&mut self, cx: &mut Context<Self>) -> Option<Entity<TerminalView>> {
        let ws = self.state.read(cx);
        let (terminal_focused, active_terminal, active_session) = (
            ws.terminal_focused(),
            ws.active_terminal(),
            ws.active_session(),
        );
        let clock = self.clock.clone();
        let chrome = self.state.clone();
        if terminal_focused {
            let i = active_terminal?;
            let registry = self.registry.read(cx);
            let id = registry.home_terminals().get(i)?.id;
            let session = registry.home_terminal(i)?.clone();
            if let Some(view) = self.home_views.get(&id) {
                return Some(view.clone());
            }
            let view = cx.new(|cx| TerminalView::new(session, None, clock, cx).with_chrome(chrome));
            self.home_views.insert(id, view.clone());
            return Some(view);
        }
        self.agent_view(active_session?, cx)
    }

    /// Get-or-create the cached `TerminalView` for an agent session. Shared by
    /// the single-session body, the grid tiles, and `focus_grid_tile` — the
    /// last one is why creation can't live only in render: entering the grid
    /// from the terminal tab must be able to focus a tile whose view has not
    /// rendered yet.
    fn agent_view(
        &mut self,
        id: crate::entities::session_registry::SessionId,
        cx: &mut Context<Self>,
    ) -> Option<Entity<TerminalView>> {
        if let Some(view) = self.views.get(&id) {
            return Some(view.clone());
        }
        let registry = self.registry.read(cx);
        let session = registry.session(id)?.clone();
        let project = registry.meta(id).map(|m| m.project.clone());
        let (clock, chrome) = (self.clock.clone(), self.state.clone());
        let view = cx.new(|cx| TerminalView::new(session, project, clock, cx).with_chrome(chrome));
        self.views.insert(id, view.clone());
        Some(view)
    }
}

/// Logs and does nothing. Each stub names the plan that implements it.
impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Startup discovery waits for `last_pty_dims` to repeat across two
        // consecutive frames, but never forever: force it after this many
        // frames in case dims never stop changing.
        const TMUX_DISCOVERY_SETTLE_FRAMES: u32 = 60;
        // A session that is attached but never painted (off the visible grid
        // page) gets no layout pass and so no true dims; attach it at the
        // focused pane's dims rather than leaving it dark forever. Counts
        // only after discovery, so anything still pending after this many
        // frames is off-screen and will not get dims of its own.
        //
        // This is a dims-accuracy grace period, not a timeout: it exists to
        // give the painted path (`TerminalElement::prepaint`) a few frames'
        // head start to claim a session at its tile's real dims before the
        // backstop grabs it at the (possibly wrong) focused-pane dims.
        // Frames are the fast lane of the animation clock, ~60ms each, so 10
        // frames is ~0.6s. Sessions-rail mode routinely relies on this
        // backstop for every non-focused session, since nothing paints their
        // terminals there — keep this short enough that those cards don't
        // sit empty for a perceptible stretch after launch.
        const TMUX_ATTACH_FALLBACK_FRAMES: u32 = 10;

        // The single zoom application point. `WithRemSize` does not exist at
        // this rev; `Window::with_rem_size` is for scoped overrides.
        let zoom_value = cx.global::<ZoomState>().zoom;
        window.set_rem_size(px(zoom::REM_BASE * zoom_value));
        // The split divider maps a cursor x against the logical window width.
        self.logical_win_w = f32::from(window.viewport_size().width) / zoom_value.max(0.1);
        // The first frame is the first point a real window width exists;
        // before that `new` had to seed against a 1280px placeholder
        // (`update/mod.rs:124-127`). Every later frame re-clamps the current
        // width against a resize (`layout.rs:59`).
        let seed_w = if self.sidebar_seeded {
            self.state.read(cx).sidebar_width()
        } else {
            self.sidebar_seeded = true;
            cx.global::<SettingsState>()
                .store
                .sidebar_width
                .unwrap_or(RAIL_W)
        };
        let clamped = clamp_sidebar_width(seed_w, self.logical_win_w);
        if (clamped - self.state.read(cx).sidebar_width()).abs() > f32::EPSILON {
            self.state
                .update(cx, |s, _| s.set_sidebar_width(clamped, self.logical_win_w));
        }

        // Spec's "always >= 1 home terminal": the section lazily spawns its
        // first shell (`src/app/terminals.rs:21-30`).
        if self.registry.read(cx).home_terminals_need_spawn() {
            self.sidebar
                .clone()
                .update(cx, Sidebar::spawn_home_terminal);
        }

        // The 5s git-state poll, kicked from the frame but running off-thread.
        // In sessions mode the rail's cards are what needs git state, and they
        // span every project regardless of collapse — so the poll set has to
        // come from the live sessions, not from the tree's visible rows
        // (`ProjectTree::polled_worktree_paths`).
        let session_wt_paths: Vec<String> = self
            .registry
            .read(cx)
            .all()
            .iter()
            .map(|m| m.wt_path.clone())
            .collect();
        let paths = {
            let ws = self.state.read(cx);
            let store = &cx.global::<SettingsState>().store;
            self.tree
                .read(cx)
                .polled_worktree_paths(store, ws, &session_wt_paths)
        };
        let window_focused = window.is_window_active();
        self.tree.clone().update(cx, |t, cx| {
            t.maybe_poll_git_state(paths, window_focused, cx);
        });

        // The diff viewer's live update rides the same frame this poll is
        // kicked from, rather than a second timer — see
        // `DiffViewerState::maybe_refresh_live`'s own throttle for the
        // "every 5s" cadence.
        if let Some(dv) = self.modals.read(cx).diff_viewer.clone() {
            dv.update(
                cx,
                crate::entities::diff_viewer::DiffViewerState::maybe_refresh_live,
            );
        }

        // Window activation: `window_focused` gates the "focused session is
        // never waiting" rule, and regaining focus acknowledges the visible
        // session (`layout.rs:34-49`).
        // The only close interception in the app (carried decision 6). It is
        // registered here rather than in `main.rs` because it needs this
        // entity, and the first render is the one place with both a
        // `&mut Window` and a `Context<Self>`.
        if !self.close_hook_registered {
            self.close_hook_registered = true;
            self.register_close_hook(window, cx);
        }

        // First run: a config that has never been through the wizard gets it
        // now. Self-latching, so a later frame never re-opens it.
        self.first_run_check(cx);

        // The agent pane's dims, cached for reattach (the element itself
        // resizes what it paints; a session that is attached but not on
        // screen still needs a truthful size).
        {
            let size = window.viewport_size();
            // Every chrome constant now renders through `rpx`, so it costs
            // `const * zoom` real pixels; the subtraction has to scale too.
            let body_w = f32::from(size.width) - self.state.read(cx).sidebar_width() * zoom_value;
            let body_h =
                f32::from(size.height) - (appbar::APPBAR_H + statusbar::STATUS_H) * zoom_value;
            self.last_pty_dims = ZoomState::new(zoom_value).pty_dims(body_w, body_h);
            cx.set_global(crate::zoom::CurrentPtyDims {
                rows: self.last_pty_dims.0,
                cols: self.last_pty_dims.1,
            });
        }

        // Startup discovery, once, after the viewport has settled — not on
        // the first frame, whose geometry is transient (Wayland configures
        // the window over several frames, `src/app/mod.rs:219-224`). Wait
        // for `last_pty_dims` to repeat across two consecutive frames so
        // reattach uses the session's real dims instead of a bogus one, but
        // never wait forever: force it after `TMUX_DISCOVERY_SETTLE_FRAMES`
        // in case dims never stop changing.
        if !self.tmux_discovered {
            self.tmux_discovery_frames += 1;
            let settled = self.prev_pty_dims == Some(self.last_pty_dims);
            let timed_out = self.tmux_discovery_frames > TMUX_DISCOVERY_SETTLE_FRAMES;
            if settled || timed_out {
                self.tmux_discovered = true;
                self.discover_tmux_sessions(cx);
            } else {
                self.prev_pty_dims = Some(self.last_pty_dims);
            }
        }

        // Stops the frame it fires — the painted path has had 90 frames by
        // then.
        if self.tmux_discovered && self.tmux_attach_frames <= TMUX_ATTACH_FALLBACK_FRAMES {
            self.tmux_attach_frames += 1;
            if self.tmux_attach_frames > TMUX_ATTACH_FALLBACK_FRAMES {
                self.attach_pending_tmux_sessions(cx);
            }
        }

        if !self.activation_observed {
            self.activation_observed = true;
            let activity = self.activity.clone();
            let upgrade = self.upgrade.clone();
            let sub = cx.observe_window_activation(window, move |_, window, cx| {
                let active = window.is_window_active();
                activity.update(cx, |a, cx| a.set_window_focused(active, cx));
                // The refocus check rides the observer Plan 06 already
                // registered rather than adding a second one
                // (`src/gui/update/upgrade.rs:193-196`).
                if active {
                    upgrade.update(cx, Upgrade::check_if_due);
                }
            });
            self.observers.push(sub);
        }

        // Carried amendment 5: a waiting session is what feeds the frame
        // clock's `animating` term, or the amber pulse would never animate.
        let waiting = self.activity.read(cx).waiting_count();
        let has_ptys =
            !self.registry.read(cx).is_empty() || self.registry.read(cx).home_terminal_count() > 0;
        let window_active = window.is_window_active();
        self.clock.clone().update(cx, |clock, cx| {
            clock.set_busy_inputs(false, has_ptys, window_active, waiting > 0, false, cx);
        });

        let body = self.body_view(cx);
        let modal_open = self.modals.read(cx).is_open();
        // Focus the body on the first frame, and again whenever the body
        // *entity* changes — selecting, cycling or clicking a session swaps
        // the view without touching focus, which otherwise stays stranded on
        // the old handle. Gated on the id actually changing, so a steady-state
        // frame never re-focuses.
        // Fix 4: gpui dispatches actions along the *focus path*, so a focus
        // handle stranded on a view that fell out of the current element tree
        // (e.g. the grid exited, or a panel shell closed) leaves every root
        // `.on_action` dead even though the id-change check above never
        // fires. Re-home focus whenever the live focus isn't the body or the
        // open panel's handle — gated off while the grid is showing, since
        // per-tile grid focus is `GridAction::Press`'s job, not this one's.
        if !modal_open && self.state.read(cx).grid_view() {
            // `grid_focused` and the keyboard must never disagree (carried
            // amendment 7), and plenty of paths move one without the other: a
            // modal closing strands focus on the unmounted modal handle, and
            // spawning a session moves `grid_focused` onto the new tile. Every
            // other setter (`GridAction::Press`, `grid_move`) leaves them in
            // agreement, so this check is a no-op in steady state.
            let tile = self
                .state
                .read(cx)
                .grid_focused()
                .and_then(|id| self.agent_view(id, cx))
                .map(|v| v.read(cx).focus_handle(cx));
            if tile.is_some() {
                if window.focused(cx) != tile {
                    self.focus_grid_tile(window, cx);
                }
            } else if window.focused(cx) != Some(self.focus.clone()) {
                // An empty grid has no tile to focus, so the re-home above
                // can't run — and every stranding path (a modal closing, the
                // sidebar and body unmounting when the grid takes the row)
                // then leaves focus outside the element tree, which kills
                // *every* binding including the one that exits the grid.
                window.focus(&self.focus, cx);
            }
            self.focused_once = true;
        } else if !modal_open && !self.state.read(cx).grid_view() {
            let panel_handle = if self.state.read(cx).term_panel_open() {
                self.panel_view(cx).map(|v| v.read(cx).focus_handle(cx))
            } else {
                None
            };
            let body_handle = body.as_ref().map(|v| v.read(cx).focus_handle(cx));
            let focused_ok = window.focused(cx).is_some_and(|f| {
                Some(&f) == body_handle.as_ref() || Some(&f) == panel_handle.as_ref()
            });
            self.last_body_focused = body.as_ref().map(gpui::Entity::entity_id);
            if !focused_ok {
                self.focused_once = true;
                if let Some(handle) = body_handle {
                    window.focus(&handle, cx);
                } else {
                    // No sessions at all: keep the root on the dispatch path
                    // rather than leaving focus stranded on nothing.
                    window.focus(&self.focus, cx);
                }
            } else {
                self.focused_once = true;
            }
        }

        // Carried amendment 4: gpui scopes by key context, not by a screen
        // flag consulted at match time. `screen_from_flags` survives purely to
        // *choose* the context string, and `Screen::key_context` already emits
        // exactly what `keymap::contexts_for` binds into. There is no fourth
        // screen: iced's own `Screen` enum has three variants
        // (`shortcuts.rs:87-91`) and the terminal tab is orthogonal to it.
        // While a modal is open the workspace stops declaring its screen
        // context, so no screen-scoped chord can fire from behind the scrim.
        // The modal declares its own context instead (spec §4); that, plus the
        // layer claiming every key its verdict table names, is what replaces
        // iced's `MODAL_OPEN` static (carried decision 3).
        let screen_context = self.state.read(cx).screen().key_context();

        let root = div()
            .track_focus(&self.focus)
            .when(!modal_open, |d| d.key_context(screen_context))
            .on_action(Self::zoom_in)
            .on_action(Self::zoom_out)
            .on_action(Self::zoom_reset)
            .on_action(
                cx.listener(|this, action: &keymap::SelectSession, window, cx| {
                    this.select_session(action, window, cx);
                }),
            )
            .on_action(cx.listener(|this, _: &keymap::NextSession, window, cx| {
                this.cycle(true, window, cx);
            }))
            .on_action(cx.listener(|this, _: &keymap::PrevSession, window, cx| {
                this.cycle(false, window, cx);
            }))
            .on_action(
                cx.listener(|this, _: &keymap::JumpToWaitingSession, _, cx| {
                    this.jump_to_waiting(cx);
                }),
            )
            .on_action(
                cx.listener(|this, _: &keymap::CloseFocusedSession, window, cx| {
                    this.close_focused(window, cx);
                }),
            )
            .on_action(cx.listener(|this, _: &keymap::ScrollHalfPageUp, _, cx| {
                this.scroll_half_page(true, cx);
            }))
            .on_action(cx.listener(|this, _: &keymap::ScrollHalfPageDown, _, cx| {
                this.scroll_half_page(false, cx);
            }))
            .on_action(cx.listener(|this, _: &keymap::ToggleGrid, window, cx| {
                this.toggle_grid(window, cx);
            }))
            .on_action(cx.listener(|this, _: &keymap::ToggleZen, window, cx| {
                this.toggle_zen(window, cx);
            }))
            .on_action(cx.listener(|this, _: &keymap::ToggleTerminal, _, cx| {
                this.toggle_terminal_tab(cx);
            }))
            .on_action(cx.listener(|this, _: &keymap::NewHomeTerminal, _, cx| {
                this.sidebar.clone().update(cx, Sidebar::new_home_terminal);
            }))
            .on_action(cx.listener(|this, a: &keymap::GridMove, window, cx| {
                this.grid_move(a.dx, a.dy, false, window, cx);
            }))
            .on_action(cx.listener(|this, a: &keymap::GridSwap, window, cx| {
                this.grid_move(a.dx, a.dy, true, window, cx);
            }))
            .on_action(cx.listener(|this, a: &keymap::AdjustTermPanel, _, cx| {
                // `term_panel_open` is runtime state, not scope: with the panel
                // closed, Ctrl+Shift+←/→ must fall through to the PTY rather
                // than being swallowed (`update/mod.rs:860-865`). A gpui action
                // handler consumes the key even when it does nothing, so the
                // closed case has to `propagate` explicitly.
                if !this.state.read(cx).term_panel_open() {
                    cx.propagate();
                    return;
                }
                this.state.update(cx, |s, cx| {
                    s.adjust_term_panel_portion(a.delta);
                    cx.notify();
                });
            }))
            .on_action(
                cx.listener(|this, _: &keymap::ToggleTermPanel, window, cx| {
                    // Grid never renders the panel at all, so the chord must fall
                    // through even though the screen still reports `Screen::Zen`
                    // when entered from the grid (`screen_from_flags`).
                    if this.state.read(cx).grid_view() {
                        cx.propagate();
                        return;
                    }
                    this.toggle_term_panel(window, cx);
                }),
            )
            .on_action(cx.listener(|this, _: &keymap::ToggleRailMode, _, cx| {
                this.toggle_rail_mode(cx);
            }))
            .on_action(cx.listener(|this, _: &keymap::FocusSidePanel, window, cx| {
                // Zen-only (Zen key context binds this); the grid never shows
                // the panel at all, so it must fall through even though the
                // screen still reports `Screen::Zen` when entered from the
                // grid (`screen_from_flags`).
                if this.state.read(cx).grid_view() {
                    cx.propagate();
                    return;
                }
                let has_panel_shell = this.panel_view(cx).is_some();
                if !this.state.read(cx).term_panel_open() {
                    // The arrows switch focus only — they no longer open the
                    // panel on demand (`mod+e`/`ToggleTermPanel` does that).
                    // With the panel closed there is nothing to focus, so the
                    // chord falls through to the PTY.
                    cx.propagate();
                    return;
                }
                if this.state.read(cx).input_target(has_panel_shell) == PtyPane::Panel {
                    // Already there — let the chord reach the PTY.
                    cx.propagate();
                    return;
                }
                this.state.update(cx, |s, cx| {
                    s.focus_pane(PtyPane::Panel);
                    cx.notify();
                });
                this.focus_panel(window, cx);
            }))
            .on_action(cx.listener(|this, _: &keymap::FocusAgentPane, window, cx| {
                if this.state.read(cx).grid_view() {
                    cx.propagate();
                    return;
                }
                let has_panel_shell = this.panel_view(cx).is_some();
                if this.state.read(cx).input_target(has_panel_shell) != PtyPane::Panel {
                    // Already on the agent — let the chord reach the PTY.
                    cx.propagate();
                    return;
                }
                this.state.update(cx, |s, cx| {
                    s.focus_pane(PtyPane::Agent);
                    cx.notify();
                });
                this.focus_agent(window, cx);
            }));
        // Plan 08 Task 3/5: the five stub actions open real modals. The three
        // palette entry points differ only in which list state the palette
        // opens into, which Task 5 fills.
        let root = root
            .on_action(cx.listener(|this, _: &keymap::NewSession, _, cx| {
                this.open_launcher(cx);
            }))
            .on_action(
                cx.listener(|this, _: &keymap::NewSessionInWorktree, _, cx| {
                    this.new_session_in_worktree(cx);
                }),
            )
            .on_action(cx.listener(|this, _: &keymap::SwitchSession, _, cx| {
                this.open_switch_drill_in(cx);
            }))
            .on_action(cx.listener(|this, _: &keymap::Settings, _, cx| {
                this.open_settings(cx);
            }))
            .on_action(cx.listener(|this, _: &keymap::ShortcutOverlay, _, cx| {
                this.open_modal(crate::modal::Modal::ShortcutOverlay, cx);
            }));

        // The divider drags (sidebar and split alike) and the tile drag all
        // need pointer events that outlive their hit zones, and the root is the
        // only element wide enough to deliver them.
        let root = sidebar::root_drag_listeners(&self.sidebar, root);
        let root = root
            .on_mouse_move(cx.listener(|this, e: &gpui::MouseMoveEvent, _, cx| {
                // `logical_win_w` and the sidebar width are design-px, so the
                // cursor is divided back out of zoom to match them.
                let zoom = cx.global::<ZoomState>().zoom.max(0.1);
                this.on_root_mouse_move(f32::from(e.position.x) / zoom, cx);
            }))
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, _: &gpui::MouseUpEvent, _, cx| this.on_root_mouse_up(cx)),
            );

        // ── the chrome (Task 6 Step 3) ──────────────────────────────────
        let dispatch: appbar::Dispatch = {
            let weak = cx.entity().downgrade();
            std::rc::Rc::new(move |action, window: &mut Window, cx: &mut App| {
                let _ = weak.update(cx, |this: &mut Self, cx| this.chrome(action, window, cx));
            })
        };
        let snap = self.snapshot(cx);
        // The terminal tab draws its own bar (`home_terminal_bar`), so the
        // session header is resolved for the agent side only.
        let header = if self.state.read(cx).terminal_focused() {
            None
        } else {
            self.header_data(&snap, cx)
        };
        let appbar_ctx = AppbarCtx {
            sidebar_width: self.state.read(cx).sidebar_width(),
            tick: self.clock.read(cx).tick(),
            pulse: self.activity.read(cx).pulse(),
            // Resolved once, handed to the pill and the dropdown alike.
            waiting: self.waiting_rows(cx),
            // `matches!(state, Available(_))` and nothing else
            // (`src/gui/view/appbar.rs:29`).
            upgrade_available: upgrade_available(self.upgrade.read(cx).state()),
            dispatch: std::rc::Rc::clone(&dispatch),
        };
        let statusbar_ctx = {
            let registry = self.registry.read(cx);
            let activity = self.activity.read(cx);
            let running = registry
                .all()
                .iter()
                .filter(|m| activity.state_of(m.id) != ActivityState::Exited)
                .count();
            let store = &cx.global::<SettingsState>().store;
            StatusbarCtx {
                running,
                backend: if grove_core::tmux::available() {
                    "tmux"
                } else {
                    "native"
                },
                theme_name: store
                    .theme
                    .clone()
                    .unwrap_or_else(|| crate::theme::DEFAULT_DARK_THEME.to_string()),
                skip_permissions: store.dangerously_skip_permissions_enabled.unwrap_or(false),
                toast: self.toast.read(cx).current().cloned(),
                dispatch,
            }
        };
        let queue_open =
            self.state.read(cx).attention_queue_open() && !appbar_ctx.waiting.is_empty();

        // ── the four bodies ─────────────────────────────────────────────
        let tick = appbar_ctx.tick;
        let grid_dispatch = self.dispatcher(cx, |this, action: GridAction, window, cx| {
            this.grid_action(action, window, cx);
        });
        let grid_ctx = GridCtx {
            tiles: self.tile_data(&snap, window, cx),
            pulse: appbar_ctx.pulse,
            // The scrim's 40-tick triangle wave —
            // `animation_clock::toast_pulse`'s first and only consumer
            // (Plan 06 recorded ambiguity 3).
            scrim_pulse: {
                let phase = crate::entities::animation_clock::toast_pulse(tick) as f32;
                (phase - 20.0).abs() / 20.0
            },
            tick,
            drag: self.state.read(cx).grid_drag(),
            slide: self.state.read(cx).grid_slide(),
            tile_size: {
                let n = self.state.read(cx).tile_order().len();
                let size = window.viewport_size();
                crate::grid::grid_tile_size(
                    f32::from(size.width),
                    f32::from(size.height),
                    zoom_value,
                    appbar::APPBAR_H + statusbar::STATUS_H,
                    n,
                )
            },
            dispatch: std::rc::Rc::clone(&grid_dispatch),
        };
        let body_el = if self.state.read(cx).terminal_focused() {
            let tab_dispatch =
                self.dispatcher(cx, |this, action: TerminalTabAction, window, cx| {
                    this.tab_action(action, window, cx);
                });
            let (running, context) = {
                let ws = self.state.read(cx);
                let registry = self.registry.read(cx);
                ws.active_terminal()
                    .and_then(|i| {
                        let meta = registry.home_terminals().get(i)?;
                        let entity = registry.home_terminal(i)?;
                        let title = entity.read(cx).title();
                        Some((
                            self.activity.read(cx).state_of(meta.id) != ActivityState::Exited,
                            title
                                .as_deref()
                                .and_then(|raw| rows::terminal_context(raw, &meta.label)),
                        ))
                    })
                    .unwrap_or((false, None))
            };
            terminal_tab::terminal_tab(&TerminalTabCtx {
                view: self.body_view(cx),
                running,
                context,
                chrome_visible: self.state.read(cx).chrome_visible(),
                dispatch: tab_dispatch,
            })
        } else {
            self.session_body(header, tick, cx)
        };

        // Task 5 Step 1: `chrome_visible` is real. Zen hides the appbar, the
        // sidebar and the statusbar; every height they gave up returns to the
        // terminal for free (findings amendment 7).
        let chrome_visible = self.state.read(cx).chrome_visible();
        let grid_view = self.state.read(cx).grid_view();
        let has_waiting = !appbar_ctx.waiting.is_empty();

        // The tile slide samples `Instant::now()` at paint time, so it is only
        // as smooth as the repaint rate — and the 60ms animation clock gives a
        // 150ms slide about two frames. Drive it off the display's frame loop
        // until it settles instead.
        if grid_view && !cx.reduce_motion() {
            if let Some(slide) = grid_ctx.slide {
                if crate::grid::slide_progress(slide.start, std::time::Instant::now()) < 1.0 {
                    window.request_animation_frame();
                }
            }
        }

        // The grid replaces the whole row **including the sidebar**
        // (`view/mod.rs:66-79`); zen shows the body alone, full-bleed.
        let content = if grid_view {
            grid::grid(&grid_ctx)
        } else if chrome_visible {
            div()
                .flex()
                .flex_row()
                .flex_1()
                .w_full()
                .overflow_hidden()
                .child(self.sidebar.clone())
                .child(body_el)
                .into_any_element()
        } else {
            body_el
        };

        root.flex()
            .flex_col()
            .relative()
            .size_full()
            .bg(c::BG())
            .text_color(c::FG())
            .when(chrome_visible, |d| d.child(appbar::appbar(&appbar_ctx)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .w_full()
                    .overflow_hidden()
                    .child(content),
            )
            .when(chrome_visible, |d| {
                d.child(statusbar::statusbar(&statusbar_ctx))
            })
            // The zen pill floats over the terminal, but only while something
            // waits (`view/mod.rs:81-99`).
            .when(!chrome_visible && has_waiting, |d| {
                d.child(appbar::zen_attention_pill(&appbar_ctx))
            })
            // The dropdown layer is gated on the chrome too (`view/mod.rs:101`).
            .when(chrome_visible, |d| d)
            .when(queue_open && chrome_visible, |d| {
                d.child(appbar::attention_dropdown(&appbar_ctx))
            })
            // The modal layer is rendered LAST, so it paints above every other
            // layer including the attention dropdown and the zen pill.
            .when(modal_open, |d| d.child(self.modals.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── the first-run gate (DESIGN.md §9.1.1's screen-replacement case) ──

    /// Points `grove_core::storage` at a private directory for this process so
    /// the first-run test below — which boots a real `SettingsState` and a
    /// real `Workspace` — can never write the developer's real
    /// `projects.json`. Mirrors the helper of the same name in
    /// `views/modals/mod.rs` and `settings.rs`.
    fn isolate_config_dir() {
        use std::sync::Once;
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            let dir =
                std::env::temp_dir().join(format!("grove-gpui-workspace-{}", std::process::id()));
            let _ = fs_err::create_dir_all(&dir);
            std::env::set_var("GROVE_CONFIG_DIR", &dir);
        });
    }

    /// The globals `Workspace::new` and `Workspace::render` read, seeded from a
    /// store whose only interesting field is `onboarded`.
    fn boot_globals(onboarded: bool, cx: &mut App) {
        let store = grove_core::storage::Store {
            onboarded,
            ..grove_core::storage::Store::default()
        };
        cx.set_global(SettingsState::new(store));
        cx.set_global(crate::theme::ThemeState::new(
            false,
            crate::theme::DEFAULT_DARK_THEME.to_string(),
            crate::theme::DEFAULT_LIGHT_THEME.to_string(),
        ));
        cx.set_global(ZoomState::new(1.0));
        // Seeded for the same reason `app.rs:136-140` seeds it: a session can
        // read it before the frame that writes it exists.
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        gpui_component::init(cx);
        cx.bind_keys(crate::keymap::bindings());
    }

    /// The wiring: a config that has never been through the wizard opens it, a
    /// config that has does not — and once it is out of the way it stays out,
    /// even though `onboarded` is still false. That last leg is what
    /// `first_run_checked` buys; the wizard's own exits are what flip the flag.
    ///
    /// Driven through [`Workspace::first_run_check`] on a windowless entity
    /// rather than through a rendered window, because `Workspace::render`'s
    /// first frame also spawns the pinned home terminal — a **real** PTY on a
    /// real bridge thread, which gpui's test scheduler rejects as
    /// non-deterministic. The latch lives inside `first_run_check` precisely so
    /// the whole decision is reachable without a frame; that `render` calls it
    /// is asserted by [`the_first_frame_runs_the_first_run_check`] below.
    #[gpui::test]
    fn a_fresh_config_opens_the_wizard_and_only_once(cx: &mut gpui::TestAppContext) {
        use crate::modal::ModalKind;

        for onboarded in [false, true] {
            isolate_config_dir();
            cx.update(|cx| boot_globals(onboarded, cx));
            let ws = cx.update(|cx| cx.new(Workspace::new));
            let modals = cx.update(|cx| ws.read(cx).modals.clone());

            cx.update(|cx| ws.update(cx, Workspace::first_run_check));
            let opened = cx.update(|cx| modals.read(cx).kind());
            if onboarded {
                assert_eq!(
                    opened, None,
                    "an onboarded config must not be shown the wizard"
                );
                continue;
            }
            assert_eq!(
                opened,
                Some(ModalKind::Onboarding),
                "onboarded=false must open the first-run wizard"
            );

            // Dismiss it *without* going through "Skip setup", so `onboarded`
            // is still false: only the latch can keep it shut.
            cx.update(|cx| modals.update(cx, super::super::modals::ModalLayer::close));
            cx.update(|cx| ws.update(cx, Workspace::first_run_check));
            assert!(
                !cx.update(|cx| modals.read(cx).is_open()),
                "a later frame must not re-open the wizard"
            );
        }
    }

    /// The other half of the wiring, guarded the same way
    /// [`every_exit_path_flushes_first`] is: the first-run check is actually
    /// reached from `render`. Without this, the test above would keep passing
    /// against a `first_run_check` nothing ever calls — which is the exact bug
    /// it exists to close.
    ///
    /// Only the source *above* `mod tests` counts, or the assertion's own
    /// string literal would satisfy it.
    #[test]
    fn the_first_frame_runs_the_first_run_check() {
        let src = include_str!("workspace.rs");
        let marker = "\n#[cfg(test)]\nmod tests {";
        assert!(
            src.contains(marker),
            "the test-module marker moved; this guard would be reading itself"
        );
        let production = src.split(marker).next().unwrap_or(src);
        assert!(
            production
                .lines()
                .any(|l| l.trim() == "self.first_run_check(cx);"),
            "Workspace::render no longer calls first_run_check, so the \
             onboarding wizard is unreachable again"
        );
    }

    /// **The** structural guarantee (carried decision 7): there are exactly
    /// three process-terminating paths — the close request, the quit confirm
    /// and the post-update restart — and each one is immediately preceded by
    /// `shutdown`, which is the only flush. A fourth exit added without one
    /// fails here rather than silently losing the user's zoom and grid order.
    ///
    /// A source-level guard because grove-gpui has no gpui test harness: the
    /// flush's own idempotence and its debounce-defeating write are asserted
    /// directly in `settings::tests`.
    #[test]
    fn every_exit_path_flushes_first() {
        let src = include_str!("workspace.rs");
        let lines: Vec<&str> = src.lines().collect();
        // The primitives that end the process or let the window go.
        let exits: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| {
                let l = l.trim();
                l == "cx.quit();" || l == "std::process::exit(0);" || l == "return true;"
            })
            .map(|(i, _)| i)
            .collect();
        assert_eq!(
            exits.len(),
            3,
            "expected exactly three exit paths, found {}",
            exits.len()
        );
        for i in exits {
            let preceding = lines[i.saturating_sub(3)..i].join("\n");
            assert!(
                preceding.contains("shutdown(cx);"),
                "the exit at line {} does not flush first:\n{preceding}",
                i + 1
            );
        }
    }

    /// The chrome heights are the `src/gui/metrics.rs:15-17` values, and the
    /// three bars agree with the workspace on what they cost vertically.
    #[test]
    fn the_chrome_heights_match_the_iced_metrics() {
        assert!((appbar::APPBAR_H - 44.0).abs() < f32::EPSILON);
        assert!((statusbar::STATUS_H - 26.0).abs() < f32::EPSILON);
        assert!((session_header::SESSBAR_H - 36.0).abs() < f32::EPSILON);
        assert!((crate::views::tokens::DIVIDER_DRAG_HIT_W - 6.0).abs() < f32::EPSILON);
    }

    // ── single-session PTY padding parity (Plan 10 Task 1) ───────────────
    //
    // Row 07 deviation 5: grid tiles and the terminal panel mirror iced's
    // `pty()` padding, but the single-session body was **unpadded**, so it
    // handed the terminal element ~5 more columns than the iced build shows.
    // The fix is `PTY_PAD_W`/`PTY_PAD_H` (`views/grid.rs`) applied half per
    // side to the single-session body and the terminal tab.

    /// `compute_pty_dims` (`src/gui/metrics.rs:265-295`), reimplemented
    /// **here** as the oracle — never exported from production code (carried
    /// amendment 1). Note it keeps `PTY_PAD_*` and `SESSBAR_H` even when the
    /// chrome is hidden; only the appbar/statusbar and the sidebar drop out.
    fn oracle_pty_dims(
        win_w: f32,
        win_h: f32,
        zoom: f32,
        chrome_visible: bool,
        sidebar_w: f32,
    ) -> (u16, u16) {
        const ICED_APPBAR_H: f32 = 44.0;
        const ICED_STATUS_H: f32 = 26.0;
        const ICED_SESSBAR_H: f32 = 36.0;
        const ICED_SIDEBAR_DIVIDER_W: f32 = 6.0;
        const ICED_PTY_PAD_W: f32 = 36.0;
        const ICED_PTY_PAD_H: f32 = 28.0;
        const ICED_CELL_W: f32 = 7.5;
        const ICED_CELL_H: f32 = 17.0;

        let zoom = zoom.max(0.1);
        let logical_w = win_w / zoom;
        let logical_h = win_h / zoom;
        let visible_w = if chrome_visible {
            sidebar_w + ICED_SIDEBAR_DIVIDER_W
        } else {
            0.0
        };
        let visible_h = if chrome_visible {
            ICED_APPBAR_H + ICED_STATUS_H
        } else {
            0.0
        };
        let usable_w = logical_w - (visible_w + ICED_PTY_PAD_W);
        let usable_h = logical_h - (visible_h + ICED_SESSBAR_H + ICED_PTY_PAD_H);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        {
            let cols = (usable_w / ICED_CELL_W).max(10.0) as u16;
            let rows = (usable_h / ICED_CELL_H).max(4.0) as u16;
            (rows, cols)
        }
    }

    /// What gpui's layout actually hands the single-session terminal element:
    /// the root column minus the appbar (+ its 1px hairline) and the
    /// statusbar, the row minus the sidebar and its 6px divider, then the
    /// session header (+ its 1px hairline) and the body's own `PTY_PAD_*`
    /// padding. The element sizes itself from those bounds in `prepaint`.
    fn gpui_session_body_dims(
        win_w: f32,
        win_h: f32,
        zoom: f32,
        chrome_visible: bool,
        sidebar_w: f32,
    ) -> (u16, u16) {
        use crate::views::grid::{PTY_PAD_H, PTY_PAD_W};

        // The chrome is `rpx`-sized, so each constant costs `const * zoom` real
        // pixels; the 1px hairlines stay 1 real pixel by design.
        let z = zoom.max(0.1);
        let chrome_w = if chrome_visible {
            (sidebar_w + crate::views::tokens::DIVIDER_DRAG_HIT_W) * z
        } else {
            0.0
        };
        let chrome_h = if chrome_visible {
            (appbar::APPBAR_H * z + 1.0) + statusbar::STATUS_H * z
        } else {
            0.0
        };
        let body_w = win_w - chrome_w - PTY_PAD_W * z;
        let body_h = win_h - chrome_h - (session_header::SESSBAR_H * z + 1.0) - PTY_PAD_H * z;
        ZoomState::new(zoom).pty_dims(body_w, body_h)
    }

    /// Plan 10 Task 1's matrix. `(win_w, win_h, zoom, sidebar_w, chrome)`.
    const PARITY_MATRIX: [(f32, f32, f32, f32, bool); 5] = [
        (1280.0, 800.0, 1.0, 320.0, true),
        (1280.0, 800.0, 1.0, 220.0, true),
        (1280.0, 800.0, 2.0, 320.0, true),
        (1280.0, 800.0, 0.6, 320.0, true),
        (1280.0, 800.0, 1.0, 320.0, false),
    ];

    /// **Exact, delta 0, at every zoom in the matrix.** The chrome is
    /// authored in [`crate::views::rpx`], so `set_rem_size(REM_BASE * zoom)`
    /// scales it exactly the way iced's application-scale-factor zoom scaled
    /// its own — which is what closes the gap this test used to record as a
    /// divergence (see [`zoom_scales_the_chrome_not_just_the_cells`]).
    #[test]
    fn the_single_session_body_matches_the_iced_oracle_exactly() {
        for (w, h, zoom, sidebar, chrome) in PARITY_MATRIX {
            let got = gpui_session_body_dims(w, h, zoom, chrome, sidebar);
            let want = oracle_pty_dims(w, h, zoom, chrome, sidebar);
            assert_eq!(
                got, want,
                "{w}x{h} zoom={zoom} sidebar={sidebar} chrome={chrome}"
            );
        }
    }

    /// The former `zoom_scales_cells_not_chrome_so_the_oracle_diverges`,
    /// inverted. Before the `rpx` sweep, gpui zoom scaled *only* the terminal
    /// cell grid: the appbar, statusbar, sidebar and session header were
    /// `px()`-sized and stayed put, so a zoomed window handed the terminal
    /// proportionally more cells than iced did (19x61 vs 15x37 at zoom 2.0).
    ///
    /// Pinned here with concrete numbers so a regression back to `px()`
    /// chrome has to come past this test.
    #[test]
    fn zoom_scales_the_chrome_not_just_the_cells() {
        // zoom 2.0, sidebar 320, chrome visible: doubling the zoom roughly
        // halves the grid, because the chrome doubled along with the cells.
        assert_eq!(
            gpui_session_body_dims(1280.0, 800.0, 2.0, true, 320.0),
            (15, 37)
        );
        // zoom 0.6, sidebar 320, chrome visible: the chrome shrinks too, so
        // the grid grows past what unscaled chrome would have allowed.
        assert_eq!(
            gpui_session_body_dims(1280.0, 800.0, 0.6, true, 320.0),
            (70, 236)
        );
    }

    /// The arithmetic tests above model the element tree; this one checks the
    /// tree is actually built that way. grove-gpui has no gpui test harness,
    /// so — like [`every_exit_path_flushes_first`] — the guard is source
    /// level. Both single-session-style bodies must carry the padding, since
    /// iced routes both through the same `pty()` (`terminal.rs:189`, `:401`).
    #[test]
    fn both_single_session_bodies_carry_the_pty_padding() {
        for (name, src) in [
            ("workspace.rs", include_str!("workspace.rs")),
            ("terminal_tab.rs", include_str!("terminal_tab.rs")),
        ] {
            assert!(
                src.contains(".px(rpx(PTY_PAD_W / 2.0))")
                    && src.contains(".py(rpx(PTY_PAD_H / 2.0))"),
                "{name} does not pad its PTY body by PTY_PAD_W/PTY_PAD_H"
            );
        }
    }
}
