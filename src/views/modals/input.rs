//! `ModalInput` — the one place both S2 workarounds live (carried decision 2).
//!
//! A thin wrapper around a `gpui_component::input::InputState` plus a policy
//! saying which keys the *hosting modal* claims from the field.
//!
//! # How the interception actually works at this gpui rev
//!
//! The plan called for **capture-phase interception**. That is not available
//! at ZED_REV `1a246ef`: `Window::dispatch_key_event`
//! (`gpui/src/window.rs:4948-4988`) matches and dispatches **key bindings
//! first**, and only calls `finish_dispatch_key_event` →
//! `dispatch_key_down_up_event` — which is where `capture_key_down` listeners
//! run — if propagation survived. A capture-phase listener therefore fires
//! *after* `MoveLeft` has already moved the caret, so it cannot pre-empt it.
//!
//! The mechanism that does work is gpui's own binding resolution, and it is
//! the idiomatic one. `Keymap::bindings_for_input`
//! (`gpui/src/keymap.rs:164-189`) sorts matched bindings by
//! `KeyBindingContextPredicate::depth_of` descending, then by registration
//! index descending. A **descendant predicate** — `"ModalSessionLauncher >
//! Input"` — matches at the very same node as gpui-component's plain
//! `"Input"` binding (`depth_of` returns the deepest matching depth for both,
//! `keymap/context.rs:260-268`), so the tie is broken by registration order
//! and the **later-registered binding wins**. Grove binds its modal keys after
//! `gpui_component::init(cx)`, so Grove wins, the action fires, and
//! `dispatch_action_on_node` stops propagation before `MoveLeft` is reached.
//!
//! The vendored `movement.rs` is **not patched** — a vendored patch is a fork
//! with extra steps and would silently diverge from the recorded rev.
//!
//! # The three contracts
//!
//! - `wants_arrows` — Left/Right belong to the modal, not the caret (the
//!   palette's `PALETTE_OPEN` carve-out, `pty_input.rs:353-356`). When clear,
//!   the caret gets them, exactly as iced's non-palette fields do.
//! - `wants_tab` — same mechanism for Tab, used by Onboarding's single-line
//!   field alternation. Multiline buffers leave it clear so Tab indents
//!   (`indent.rs:219-232`; `is_indentable()` is true for multiline).
//! - **Never** `clean_on_escape`: `InputState::escape()` calls `cx.propagate()`
//!   (`input/state.rs:1685`) unless that flag is set, which is the whole
//!   reason Escape reaches the modal layer from inside a focused field.

// The chrome, the input wrapper and the archive/teardown helpers are built
// once here and consumed by Tasks 4-6 of gpui rewrite plan 08.
#![allow(dead_code)]

use gpui::{App, AppContext as _, Context, Entity, Window};
use gpui_component::input::InputState;

use crate::modal::ModalKind;

/// Which keys the hosting modal claims from its field.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputPolicy {
    /// ←/→ go to the modal, not the caret.
    pub wants_arrows: bool,
    /// Tab goes to the modal, not the field's indent/tab-stop handling.
    pub wants_tab: bool,
    /// A multiline buffer. Tab **indents** here and is never claimed —
    /// traversal is click plus `ctrl-tab` at the modal level.
    pub multi_line: bool,
}

impl InputPolicy {
    /// The policy a given modal's field runs under, derived from the pure
    /// state machine so the two can never disagree.
    pub fn for_modal(kind: ModalKind) -> Self {
        // `ScriptsEditor`'s name field and its three lifecycle buffers are
        // all genuinely single-line since the "Variant D" redesign
        // (`views/modals/mod.rs`'s `ScriptsEditor` field-construction arm) —
        // `ThemeManager`'s editor buffer is the only survivor here.
        let multi_line = matches!(kind, ModalKind::ThemeManager);
        Self {
            wants_arrows: kind.wants_arrows(),
            // A multiline buffer never claims Tab — see the module doc.
            wants_tab: kind.wants_tab() && !multi_line,
            multi_line,
        }
    }
}

/// A modal text field: the `InputState` plus the policy that decides which
/// keystrokes the modal steals from it.
pub struct ModalInput {
    state: Entity<InputState>,
    policy: InputPolicy,
}

impl ModalInput {
    /// Build a single-line field. `placeholder` is shown while empty.
    pub fn single_line(
        policy: InputPolicy,
        placeholder: &str,
        initial: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        debug_assert!(!policy.multi_line, "use ModalInput::multi_line");
        let placeholder = placeholder.to_owned();
        let state = cx.new(|cx| {
            let mut st = InputState::new(window, cx).placeholder(placeholder);
            if !initial.is_empty() {
                st.set_value(initial, window, cx);
            }
            st
        });
        Self { state, policy }
    }

    /// Build a multiline buffer of `rows` visible lines (the scripts-editor
    /// and theme-editor shape, `src/gui/scripts_editor.rs:31-33`).
    pub fn multi_line(
        placeholder: &str,
        initial: &str,
        rows: usize,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        let placeholder = placeholder.to_owned();
        let state = cx.new(|cx| {
            let mut st = InputState::new(window, cx)
                .multi_line(true)
                .rows(rows)
                .placeholder(placeholder);
            if !initial.is_empty() {
                st.set_value(initial, window, cx);
            }
            st
        });
        Self {
            state,
            policy: InputPolicy {
                wants_arrows: false,
                wants_tab: false,
                multi_line: true,
            },
        }
    }

    pub fn state(&self) -> &Entity<InputState> {
        &self.state
    }

    pub fn policy(&self) -> InputPolicy {
        self.policy
    }

    pub fn value(&self, cx: &App) -> String {
        self.state.read(cx).value().to_string()
    }

    /// Focus on mount. A field that is never focused silently eats nothing and
    /// looks broken (gpui-development skill pitfall; carried decision 5).
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.state.update(cx, |st, cx| st.focus(window, cx));
    }

    /// Focus and put the caret at the end — the move-cursor-to-end idiom
    /// `focus_add_project_field` performs in iced (`modals.rs:625-644`).
    pub fn focus_at_end(&self, window: &mut Window, cx: &mut App) {
        self.state.update(cx, |st, cx| {
            st.focus(window, cx);
            let len = st.value().len();
            st.set_selected_range(len..len, cx);
        });
    }

    pub fn set_value(&self, value: &str, window: &mut Window, cx: &mut App) {
        self.state
            .update(cx, |st, cx| st.set_value(value, window, cx));
    }

    /// The key-context string the field's *host* declares, so a
    /// `"<host> > Input"` binding can out-rank gpui-component's plain
    /// `"Input"` one. See the module doc.
    pub fn override_context(host: ModalKind) -> String {
        format!("{} > Input", host.key_context())
    }
}

/// `Context`-taking convenience for views that build their fields inside
/// `Entity::new`.
pub fn single_line_in<V: 'static>(
    policy: InputPolicy,
    placeholder: &str,
    initial: &str,
    window: &mut Window,
    cx: &mut Context<V>,
) -> ModalInput {
    ModalInput::single_line(policy, placeholder, initial, window, cx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_palette_and_the_wizard_claim_the_arrows() {
        for kind in ModalKind::ALL {
            let p = InputPolicy::for_modal(kind);
            let expected = matches!(kind, ModalKind::SessionLauncher | ModalKind::AddProject);
            assert_eq!(p.wants_arrows, expected, "{kind:?}");
        }
    }

    #[test]
    fn multiline_modals_never_claim_tab_so_it_indents() {
        let kind = ModalKind::ThemeManager;
        let p = InputPolicy::for_modal(kind);
        assert!(p.multi_line, "{kind:?}");
        assert!(!p.wants_tab, "{kind:?} must let Tab indent");
    }

    /// `ScriptsEditor`'s name field and its three lifecycle buffers are all
    /// genuinely single-line since the "Variant D" redesign, so its fields
    /// get the same `Enter`/`Up`/`Down` bindings any other single-line-only
    /// modal gets (`modal_input_bindings` in `src/keymap.rs` skips a `kind`
    /// entirely while `multi_line` is set).
    #[test]
    fn scripts_editor_is_no_longer_multi_line() {
        assert!(!InputPolicy::for_modal(ModalKind::ScriptsEditor).multi_line);
    }

    #[test]
    fn onboarding_claims_tab_for_its_single_line_field_alternation() {
        let p = InputPolicy::for_modal(ModalKind::Onboarding);
        assert!(p.wants_tab);
        assert!(!p.multi_line);
    }

    #[test]
    fn the_override_context_out_ranks_the_plain_input_context() {
        // A descendant predicate matching at the same node as `"Input"`; the
        // tie is broken by registration order (see the module doc).
        assert_eq!(
            ModalInput::override_context(ModalKind::SessionLauncher),
            "ModalSessionLauncher > Input"
        );
    }
}
