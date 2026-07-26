use super::shortcuts::{global_mods, Screen};
use crate::gui::metrics::{pty_metrics, TERM_PANEL_PORTION_STEP};
use crate::gui::state::{AbsCell, FocusedPane, Grove, Msg, PtyCell, PtyDrag, PtyPane};
use grove_core::session::Session;
use iced::keyboard::{key::Named, Key, Modifiers};
use iced::Task;
use iced::{event, keyboard, Event};

impl Grove {
    pub(super) fn on_pty_mouse_down(&mut self, pane: PtyPane, x: f32, y: f32) -> Task<Msg> {
        if let PtyPane::Tile(si) = pane {
            // Focus this tile, then anchor a selection the same way
            // the Agent/Panel path below does — `grid_focused` is
            // already updated, so `pixel_to_abs`/`pty_view_geom`
            // (which resolve via `focused_session`) target this
            // tile's session.
            self.pty_press_focused = self.grid_focused != Some(si);
            self.grid_focused = Some(si);
            self.app.active_session = Some(si);
            self.acknowledge_session(si);
            self.pty_selection = None;
            if let (Some(cell), Some((h, _))) = (self.pixel_to_abs(x, y), self.pty_view_geom()) {
                self.pty_selection = Some((cell, cell));
                self.pty_drag = Some(PtyDrag {
                    last_x: x,
                    last_y: y,
                    view_h_px: h as f32 * pty_metrics(1.0).cell_h,
                });
            }
            return Task::none();
        }
        self.pending_kill = None;
        self.pending_kill_terminal = None;
        // Clicking a PTY focuses its pane (so subsequent keystrokes,
        // scroll, and this very selection route there). Honored only
        // while the panel is open; otherwise the agent always owns input.
        let pane_before = self.focused_pane;
        self.focus_pane(pane);
        self.pty_press_focused = self.focused_pane != pane_before;
        // A focus switch invalidates any in-progress selection on the
        // previously focused PTY — it was anchored to a different grid.
        self.pty_selection = None;
        if let (Some(cell), Some((h, _))) = (self.pixel_to_abs(x, y), self.pty_view_geom()) {
            self.pty_selection = Some((cell, cell));
            self.pty_drag = Some(PtyDrag {
                last_x: x,
                last_y: y,
                view_h_px: h as f32 * pty_metrics(1.0).cell_h,
            });
        }
        Task::none()
    }

    pub(super) fn on_pty_mouse_drag(&mut self, pane: PtyPane, x: f32, y: f32) -> Task<Msg> {
        // Ignore drags from the pane that doesn't own the active
        // selection (the canvas captures the drag, but focus — and thus
        // the geometry helpers — belong to the pane the press landed in).
        // `selection_pane` covers grid tiles too: it resolves to the
        // focused tile while in grid view.
        if self.selection_pane() != pane {
            return Task::none();
        }
        if let Some(d) = self.pty_drag.as_mut() {
            d.last_x = x;
            d.last_y = y;
        }
        if let (Some(cell), Some((a, _))) = (self.pixel_to_abs(x, y), self.pty_selection) {
            self.pty_selection = Some((a, cell));
        }
        Task::none()
    }

    pub(super) fn on_pty_scroll(&mut self, pane: PtyPane, up: bool, x: f32, y: f32) -> Task<Msg> {
        if let PtyPane::Tile(si) = pane {
            // Scroll the specific tile under the cursor, not just the focused one.
            if let Some(s) = self.app.sessions.get_mut(si) {
                let cell = pixel_to_cell(x, y);
                s.scroll(up, cell.col as u16, cell.row as u16);
            }
            return Task::none();
        }
        // Scrolling over a PTY focuses it too, so the wheel always
        // drives the terminal under the cursor — but don't hand focus
        // to a panel with no shell: input routed there would fall back
        // to the agent while keystrokes stayed stuck on the panel.
        let panel_has_shell = self
            .active_wt_path()
            .is_some_and(|wt| self.app.active_wt_terminal(&wt).is_some());
        if !matches!(pane, PtyPane::Panel) || panel_has_shell {
            self.focus_pane(pane);
        }
        let cell = pixel_to_cell(x, y);
        if let Some(s) = self.focused_session_mut() {
            s.scroll(up, cell.col as u16, cell.row as u16);
        }
        Task::none()
    }

    pub(super) fn on_pty_mouse_up(&mut self) -> Task<Msg> {
        self.pty_drag = None;
        // The press that switched focus is focus-only: swallow its
        // release so refocusing a pane never moves the caret (a second
        // click does).
        let press_focused = std::mem::take(&mut self.pty_press_focused);
        if let Some((a, h)) = self.pty_selection {
            if a == h {
                self.pty_selection = None;
                if press_focused {
                    return Task::none();
                }
                // No drag happened — treat it as a click-to-move-caret.
                // `pixel_to_abs` only clamps into the visible window
                // when scrollback is 0, so bail if the view has been
                // scrolled: clicking history must be inert.
                if let Some((h_rows, sb)) = self.pty_view_geom() {
                    if sb == 0 && h_rows > 0 {
                        let row = (h_rows - 1).saturating_sub(a.a_row) as u16;
                        if let Some(s) = self.focused_session_mut() {
                            s.click(a.col as u16, row);
                        }
                    }
                }
            }
        }
        Task::none()
    }

    /// Reset the input-focus target after the active session (and hence the
    /// panel's worktree) changes: focus the panel when it's open (the just
    /// re-anchored terminal), otherwise the agent.
    pub(super) fn reset_focused_pane(&mut self) {
        self.focused_pane = if self.term_panel_open {
            FocusedPane::Panel
        } else {
            FocusedPane::Agent
        };
    }

    /// Whether input currently routes to the panel PTY. Only true while the
    /// panel is open *and* the panel pane holds focus.
    pub(super) fn panel_focused(&self) -> bool {
        matches!(self.focused_input_pane(), PtyPane::Panel)
    }

    /// Apply a click/scroll's origin pane to the input-focus target. A `Panel`
    /// click only takes effect while the panel is open; an `Agent` click always
    /// returns focus to the agent (it's only reachable as a click target when
    /// the split is showing both PTYs).
    pub(super) fn focus_pane(&mut self, pane: PtyPane) {
        if !self.term_panel_open {
            return;
        }
        self.focused_pane = match pane {
            PtyPane::Agent => FocusedPane::Agent,
            PtyPane::Panel => FocusedPane::Panel,
            PtyPane::Tile(_) => return, // tile focus handled via grid_focused
        };
    }

    /// The session the workspace PTY is currently showing — and that keystrokes,
    /// scrolling, and selection target. The home terminal when the terminal tab
    /// is active, otherwise the active worktree session.
    pub(super) fn focused_session(&self) -> Option<&Session> {
        if self.grid_view {
            return self.grid_focused.and_then(|si| self.app.sessions.get(si));
        }
        if self.terminal_tab() {
            self.app.active_home_terminal()
        } else if self.panel_focused() {
            // Panel terminal when this worktree has one; otherwise fall back to
            // the agent so a worktree with no shell doesn't silently swallow
            // keystrokes.
            self.active_wt_path()
                .and_then(|wt| self.app.active_wt_terminal(&wt))
                .or_else(|| {
                    self.app
                        .active_session
                        .and_then(|i| self.app.sessions.get(i))
                })
        } else {
            self.app
                .active_session
                .and_then(|i| self.app.sessions.get(i))
        }
    }

    pub(super) fn focused_session_mut(&mut self) -> Option<&mut Session> {
        if self.grid_view {
            return self
                .grid_focused
                .and_then(move |si| self.app.sessions.get_mut(si));
        }
        if self.terminal_tab() {
            self.app
                .active_terminal
                .and_then(move |i| self.app.home_terminals.get_mut(i))
        } else if self.panel_focused() {
            if let Some(wt) = self.active_wt_path() {
                if let Some(idx) = self.app.active_wt_terminal_idx(&wt) {
                    return self
                        .app
                        .wt_terminals
                        .get_mut(&wt)
                        .and_then(|v| v.get_mut(idx));
                }
            }
            // No panel shell for this worktree — route to the agent instead.
            self.app
                .active_session
                .and_then(move |i| self.app.sessions.get_mut(i))
        } else {
            self.app
                .active_session
                .and_then(move |i| self.app.sessions.get_mut(i))
        }
    }

    /// Absolute worktree path of the active session — the scope of the terminal
    /// slide-over panel. `None` when no session is active.
    pub(in crate::gui) fn active_wt_path(&self) -> Option<String> {
        self.app
            .active_session
            .and_then(|i| self.app.sessions.get(i))
            .map(|s| s.wt_path.clone())
    }

    pub(super) fn selection_text(&self) -> Option<String> {
        let (a, h) = self.pty_selection?;
        let s = self.focused_session()?;
        s.selection_text_abs((a.a_row, a.col), (h.a_row, h.col))
    }

    /// Visible grid height (rows) and current scrollback offset of the focused
    /// session, used to convert between viewport and absolute selection rows.
    pub(super) fn pty_view_geom(&self) -> Option<(usize, usize)> {
        let s = self.focused_session()?;
        let p = s.parser.lock().ok()?;
        let (h, _) = p.screen().size();
        Some((h as usize, p.screen().scrollback()))
    }

    /// Convert unzoomed canvas pixels to an absolute selection cell, clamping
    /// the row into the currently-visible window `[S, S + h - 1]`.
    pub(super) fn pixel_to_abs(&self, x: f32, y: f32) -> Option<AbsCell> {
        let (h, sb) = self.pty_view_geom()?;
        if h == 0 {
            return None;
        }
        let m = pty_metrics(1.0);
        let r = ((y / m.cell_h).max(0.0) as usize).min(h - 1);
        let col = (x / m.cell_w).max(0.0) as usize;
        Some(AbsCell {
            a_row: sb + (h - 1 - r),
            col,
        })
    }

    /// Called each `Msg::Tick`. While a selection drag is held with the cursor
    /// in the top/bottom edge zone, scroll grove's scrollback one step in that
    /// direction and extend the selection head over the revealed line.
    pub(super) fn tick_drag_autoscroll(&mut self) {
        let Some(d) = self.pty_drag else { return };
        let margin = pty_metrics(1.0).cell_h;
        let up = if d.last_y <= margin {
            true
        } else if d.last_y >= d.view_h_px - margin {
            false
        } else {
            return;
        };
        // Drive grove's own scrollback (no-op if the inner app grabs the mouse).
        let before = self.pty_view_geom().map(|(_, s)| s);
        if let Some(s) = self.focused_session_mut() {
            s.scroll(up, 0, 0);
        }
        // Only extend if the scroll actually moved the view.
        if self.pty_view_geom().map(|(_, s)| s) == before {
            return;
        }
        if let (Some(cell), Some((anchor, _))) =
            (self.pixel_to_abs(d.last_x, d.last_y), self.pty_selection)
        {
            self.pty_selection = Some((anchor, cell));
        }
    }
}

pub(super) fn pixel_to_cell(x: f32, y: f32) -> PtyCell {
    let metrics = pty_metrics(1.0);
    PtyCell {
        row: (y / metrics.cell_h).max(0.0) as usize,
        col: (x / metrics.cell_w).max(0.0) as usize,
    }
}

/// Whether the event subscription forwards this event to `update()`.
///
/// Captured events belong to the widget that consumed them — with two
/// carve-outs:
/// - Escape: a focused `text_input` captures it only to blur itself and
///   never tells the app, so without this cancelling a modal would take two
///   presses.
/// - A `global_mods` chord (Cmd on mac, Ctrl+Shift elsewhere): a focused
///   `text_input` captures *every* `KeyPressed`, chord or not, since as far
///   as the widget is concerned a character key just gets typed. But a
///   chord is a command, never text — without this carve-out, e.g. ⌘D in
///   the Theme sub-pane never reached `handle_modal_key`'s Theme-pane arm at
///   all; the user just saw "d" typed into the focused search field.
///   This carve-out is gated on `modal_open`: `handle_key` only reaches the
///   `handle_modal_key` chord-handling path (which needs it) while a modal
///   is open; with no modal open, the same forwarded chord would instead
///   fall through to `handle_key`'s PTY copy/paste shortcuts (⌘C/⌘V) —
///   double-handling a chord the focused (non-modal) text widget already
///   consumed itself (e.g. copying/pasting into that widget, then again into
///   the PTY). Escape's carve-out stays unconditional: `escape_should_dismiss`
///   is meant to reach `handle_key` with no modal open too.
///   Backing store for `should_forward`'s `modal_open` check — see
///   `Grove::subscription`'s doc comment for why a static is needed here
///   instead of a captured closure variable.
pub(super) static MODAL_OPEN: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub(super) fn should_forward(ev: &Event, status: event::Status, modal_open: bool) -> bool {
    if status != event::Status::Captured {
        return true;
    }
    match ev {
        Event::Keyboard(keyboard::Event::KeyPressed {
            key: Key::Named(Named::Escape),
            ..
        }) => true,
        Event::Keyboard(keyboard::Event::KeyPressed { modifiers, .. }) => {
            modal_open && global_mods(*modifiers)
        }
        _ => false,
    }
}

/// Whether Escape has something to dismiss when no modal is open. `false`
/// means Escape must reach the PTY — many TUI programs need it, and
/// swallowing it unconditionally would regress that. The caller clears both
/// states, so which one is armed doesn't matter.
pub(super) fn escape_should_dismiss(
    pending_kill: Option<usize>,
    pending_kill_terminal: Option<usize>,
    open_agent_menu: Option<(usize, usize)>,
    attention_open: bool,
) -> bool {
    pending_kill.is_some()
        || pending_kill_terminal.is_some()
        || open_agent_menu.is_some()
        || attention_open
}

/// Chord + scope check for the terminal-panel resize (see the registry's
/// display-only "resize terminal panel" row): Ctrl+Shift+Left/Right, Workspace
/// only, on every platform (unlike `global_mods`, this isn't Cmd on macOS).
/// Doesn't know about `term_panel_open` — that's runtime state the caller
/// gates separately so a closed panel falls through to the PTY.
/// How far a keyboard scroll chord should move the view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ScrollAmount {
    /// One page (the session's viewport height, minus a line of overlap).
    Page,
    /// The full scrollback, i.e. jump to the top or back to the bottom.
    All,
}

/// Maps a key event to a keyboard-scroll intent: `Some((up, amount))` when
/// the classic terminal scroll chords are pressed — Shift+PageUp/PageDown
/// (page) or Shift+Home/End (top/bottom) — and `None` otherwise, including
/// when Ctrl/Logo/Alt are also held (so readline/TUI chords like
/// Ctrl+Shift+PageUp aren't stolen) or when Shift isn't held at all (so plain
/// PageUp/PageDown/Home/End keep reaching the PTY).
pub(super) fn keyboard_scroll_intent(key: &Key, mods: Modifiers) -> Option<(bool, ScrollAmount)> {
    if !mods.shift() || mods.control() || mods.logo() || mods.alt() {
        return None;
    }
    match key {
        Key::Named(Named::PageUp) => Some((true, ScrollAmount::Page)),
        Key::Named(Named::PageDown) => Some((false, ScrollAmount::Page)),
        Key::Named(Named::Home) => Some((true, ScrollAmount::All)),
        Key::Named(Named::End) => Some((false, ScrollAmount::All)),
        _ => None,
    }
}

pub(super) fn term_panel_resize_delta(key: &Key, mods: Modifiers, screen: Screen) -> Option<i16> {
    if screen != Screen::Workspace || !(mods.control() && mods.shift()) {
        return None;
    }
    match key {
        Key::Named(Named::ArrowRight) => Some(TERM_PANEL_PORTION_STEP as i16),
        Key::Named(Named::ArrowLeft) => Some(-(TERM_PANEL_PORTION_STEP as i16)),
        _ => None,
    }
}

/// Returns true when the key event matches the OS copy shortcut.
/// macOS: Cmd+C (logo, no ctrl, no shift)
/// Others: Ctrl+Shift+C
pub(super) fn is_copy_shortcut(mods: Modifiers, s: &str) -> bool {
    if !s.eq_ignore_ascii_case("c") {
        return false;
    }
    #[cfg(target_os = "macos")]
    return mods.logo() && !mods.control();
    #[cfg(not(target_os = "macos"))]
    return mods.control() && mods.shift();
}

/// Returns true when the key event matches the OS paste shortcut.
/// macOS: Cmd+V (logo, no ctrl)
/// Others: Ctrl+Shift+V (mirrors the Ctrl+Shift+C copy shortcut; plain
/// Ctrl+V is left for the PTY, e.g. literal insert in vim/readline).
pub(super) fn is_paste_shortcut(mods: Modifiers, s: &str) -> bool {
    if !s.eq_ignore_ascii_case("v") {
        return false;
    }
    #[cfg(target_os = "macos")]
    return mods.logo() && !mods.control();
    #[cfg(not(target_os = "macos"))]
    return mods.control() && mods.shift();
}

#[cfg(test)]
mod tests {
    /// The terminal-panel resize chord (see the registry's display-only
    /// "resize terminal panel" row) is Ctrl+Shift+Left/Right on every
    /// platform, and scoped to `Screen::Workspace` only — matched by
    /// `term_panel_resize_delta`, not `match_global_shortcut`, because it has
    /// an extra runtime gate (`term_panel_open`) that `handle_key` applies
    /// separately.
    mod term_panel_resize {
        use super::super::{term_panel_resize_delta, Screen, TERM_PANEL_PORTION_STEP};
        use iced::keyboard::{key::Named, Key, Modifiers};

        fn ctrl_shift() -> Modifiers {
            Modifiers::CTRL | Modifiers::SHIFT
        }

        #[test]
        fn matches_only_on_workspace() {
            assert_eq!(
                term_panel_resize_delta(
                    &Key::Named(Named::ArrowRight),
                    ctrl_shift(),
                    Screen::Workspace
                ),
                Some(TERM_PANEL_PORTION_STEP as i16)
            );
            assert_eq!(
                term_panel_resize_delta(
                    &Key::Named(Named::ArrowLeft),
                    ctrl_shift(),
                    Screen::Workspace
                ),
                Some(-(TERM_PANEL_PORTION_STEP as i16))
            );
            assert_eq!(
                term_panel_resize_delta(&Key::Named(Named::ArrowRight), ctrl_shift(), Screen::Grid),
                None
            );
            assert_eq!(
                term_panel_resize_delta(&Key::Named(Named::ArrowRight), ctrl_shift(), Screen::Zen),
                None
            );
        }

        #[test]
        fn requires_the_literal_ctrl_shift_chord() {
            assert_eq!(
                term_panel_resize_delta(
                    &Key::Named(Named::ArrowRight),
                    Modifiers::CTRL,
                    Screen::Workspace
                ),
                None
            );
            assert_eq!(
                term_panel_resize_delta(&Key::Named(Named::Tab), ctrl_shift(), Screen::Workspace),
                None
            );
        }
    }

    /// Keyboard scrollback chords (Shift+PageUp/PageDown/Home/End) are
    /// matched by `keyboard_scroll_intent` ahead of `key_to_bytes` in
    /// `handle_key`, screen-independent (applies on Workspace/Zen/Grid alike
    /// via `focused_session_mut`), and must require Shift alone — no
    /// Ctrl/Logo/Alt — so readline/TUI chords like Ctrl+Shift+PageUp aren't
    /// stolen and plain PageUp/PageDown/Home/End keep reaching the PTY.
    mod keyboard_scroll {
        use super::super::{keyboard_scroll_intent, ScrollAmount};
        use iced::keyboard::{key::Named, Key, Modifiers};

        #[test]
        fn shift_page_up_down_scroll_by_page() {
            assert_eq!(
                keyboard_scroll_intent(&Key::Named(Named::PageUp), Modifiers::SHIFT),
                Some((true, ScrollAmount::Page))
            );
            assert_eq!(
                keyboard_scroll_intent(&Key::Named(Named::PageDown), Modifiers::SHIFT),
                Some((false, ScrollAmount::Page))
            );
        }

        #[test]
        fn shift_home_end_jump_top_and_bottom() {
            assert_eq!(
                keyboard_scroll_intent(&Key::Named(Named::Home), Modifiers::SHIFT),
                Some((true, ScrollAmount::All))
            );
            assert_eq!(
                keyboard_scroll_intent(&Key::Named(Named::End), Modifiers::SHIFT),
                Some((false, ScrollAmount::All))
            );
        }

        #[test]
        fn plain_page_up_down_fall_through_to_the_pty() {
            assert_eq!(
                keyboard_scroll_intent(&Key::Named(Named::PageUp), Modifiers::empty()),
                None
            );
            assert_eq!(
                keyboard_scroll_intent(&Key::Named(Named::PageDown), Modifiers::empty()),
                None
            );
            assert_eq!(
                keyboard_scroll_intent(&Key::Named(Named::Home), Modifiers::empty()),
                None
            );
            assert_eq!(
                keyboard_scroll_intent(&Key::Named(Named::End), Modifiers::empty()),
                None
            );
        }

        #[test]
        fn extra_modifiers_are_not_stolen_from_readline_or_tui_chords() {
            assert_eq!(
                keyboard_scroll_intent(
                    &Key::Named(Named::PageUp),
                    Modifiers::CTRL | Modifiers::SHIFT
                ),
                None
            );
            assert_eq!(
                keyboard_scroll_intent(
                    &Key::Named(Named::PageUp),
                    Modifiers::LOGO | Modifiers::SHIFT
                ),
                None
            );
            assert_eq!(
                keyboard_scroll_intent(
                    &Key::Named(Named::PageUp),
                    Modifiers::ALT | Modifiers::SHIFT
                ),
                None
            );
        }
    }

    /// The subscription's capture filter. Escape must survive capture (a
    /// focused text_input eats it to self-blur); nothing else may (Bug 3).
    mod should_forward {
        use super::super::should_forward;
        use super::gmods;
        use iced::keyboard::{key::Named, Key, Modifiers};
        use iced::{event, keyboard, Event};

        fn press(key: Key) -> Event {
            press_mods(key, Modifiers::empty())
        }

        fn press_mods(key: Key, modifiers: Modifiers) -> Event {
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: key.clone(),
                modified_key: key,
                physical_key: iced::keyboard::key::Physical::Unidentified(
                    iced::keyboard::key::NativeCode::Unidentified,
                ),
                location: iced::keyboard::Location::Standard,
                modifiers,
                text: None,
                repeat: false,
            })
        }

        #[test]
        fn uncaptured_events_always_forward() {
            assert!(should_forward(
                &press(Key::Character("a".into())),
                event::Status::Ignored,
                false
            ));
        }

        #[test]
        fn captured_escape_still_forwards() {
            // Escape's carve-out is unconditional — it must reach
            // `escape_should_dismiss` whether or not a modal is open.
            assert!(should_forward(
                &press(Key::Named(Named::Escape)),
                event::Status::Captured,
                false
            ));
            assert!(should_forward(
                &press(Key::Named(Named::Escape)),
                event::Status::Captured,
                true
            ));
        }

        #[test]
        fn captured_non_escape_is_dropped() {
            // The load-bearing half: typed characters and Enter belong to the
            // focused field, not to handle_key.
            assert!(!should_forward(
                &press(Key::Character("a".into())),
                event::Status::Captured,
                true
            ));
            assert!(!should_forward(
                &press(Key::Named(Named::Enter)),
                event::Status::Captured,
                true
            ));
        }

        /// The bug this regresses: the palette's focused search input
        /// captures every `KeyPressed` (it's a text field), so without this
        /// carve-out a chord like ⌘D never reached `handle_modal_key`'s
        /// Theme-pane arm at all — the user just saw "d" typed into the
        /// query. Chords are commands, never text, regardless of what
        /// widget currently has focus — but only while a modal is open,
        /// since that's the only time `handle_modal_key` is reached at all.
        #[test]
        fn captured_global_mod_chord_forwards_while_modal_open() {
            assert!(should_forward(
                &press_mods(Key::Character("d".into()), gmods()),
                event::Status::Captured,
                true
            ));
            // Named keys chord too (⌘⌫ delete).
            assert!(should_forward(
                &press_mods(Key::Named(Named::Backspace), gmods()),
                event::Status::Captured,
                true
            ));
        }

        /// With no modal open, the same captured chord must NOT forward:
        /// `handle_key` would otherwise double-handle it via the PTY
        /// copy/paste shortcuts (⌘C/⌘V) on top of whatever the focused
        /// non-modal text widget already did with it itself.
        #[test]
        fn captured_global_mod_chord_dropped_without_modal_open() {
            assert!(!should_forward(
                &press_mods(Key::Character("c".into()), gmods()),
                event::Status::Captured,
                false
            ));
            assert!(!should_forward(
                &press_mods(Key::Character("v".into()), gmods()),
                event::Status::Captured,
                false
            ));
        }

        /// A bare modifier that doesn't add up to the platform's *global*
        /// chord (`global_mods` requires Ctrl+Shift together on non-mac,
        /// Cmd-without-Ctrl on mac) must not forward — otherwise every
        /// Ctrl-chord a text field captures for its own editing (e.g.
        /// word-delete) would leak into `handle_modal_key` too.
        #[test]
        fn captured_partial_or_unrelated_modifier_still_dropped() {
            assert!(!should_forward(
                &press_mods(Key::Character("d".into()), Modifiers::CTRL),
                event::Status::Captured,
                true
            ));
            assert!(!should_forward(
                &press_mods(Key::Character("d".into()), Modifiers::SHIFT),
                event::Status::Captured,
                true
            ));
        }
    }

    /// Escape with no modal open must dismiss an armed kill-confirmation or
    /// open agent menu before it ever reaches the PTY (Bug 9).
    mod escape_should_dismiss {
        use super::super::escape_should_dismiss;

        #[test]
        fn false_when_neither_is_set() {
            assert!(!escape_should_dismiss(None, None, None, false));
        }

        #[test]
        fn true_when_either_is_set() {
            assert!(escape_should_dismiss(Some(2), None, None, false));
            assert!(escape_should_dismiss(None, Some(3), None, false));
            assert!(escape_should_dismiss(None, None, Some((1, 0)), false));
            assert!(escape_should_dismiss(Some(2), None, Some((1, 0)), false));
        }

        #[test]
        fn true_when_attention_queue_is_open() {
            assert!(escape_should_dismiss(None, None, None, true));
        }
    }

    /// The platform's global modifier: Cmd on macOS, Ctrl+Shift elsewhere.
    fn gmods() -> Modifiers {
        #[cfg(target_os = "macos")]
        return Modifiers::LOGO;
        #[cfg(not(target_os = "macos"))]
        return Modifiers::CTRL | Modifiers::SHIFT;
    }

    use iced::keyboard::Modifiers;
}
