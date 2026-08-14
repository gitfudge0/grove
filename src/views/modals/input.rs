//! `ModalInput` pairs an `InputState` with a policy for which keys the modal steals. Capture-phase interception doesn't work here; see `override_context`.

use gpui::{App, AppContext as _, Entity, Window};
use gpui_component::input::InputState;

use crate::modal::ModalKind;

/// Which keys the hosting modal claims from its field.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputPolicy {
    /// ←/→ go to the modal, not the caret.
    pub wants_arrows: bool,
    /// Tab goes to the modal, not the field's indent/tab-stop handling.
    pub wants_tab: bool,
    /// A multiline buffer. Tab **indents** here and is never claimed — traversal is click plus `ctrl-tab` at the modal level.
    pub multi_line: bool,
}

impl InputPolicy {
    /// The policy a given modal's field runs under, derived from the pure state machine so the two can never disagree.
    pub fn for_modal(kind: ModalKind) -> Self {
        // ThemeManager's editor buffer is the only multi-line field left; ScriptsEditor's are all single-line now.
        let multi_line = matches!(kind, ModalKind::ThemeManager);
        Self {
            wants_arrows: kind.wants_arrows(),
            // A multiline buffer never claims Tab — see the module doc.
            wants_tab: kind.wants_tab() && !multi_line,
            multi_line,
        }
    }
}

/// A modal text field: the `InputState` plus the policy that decides which keystrokes the modal steals from it.
pub struct ModalInput {
    state: Entity<InputState>,
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
        Self { state }
    }

    /// Build a multiline buffer of `rows` visible lines (the scripts-editor and theme-editor shape, `src/gui/scripts_editor.rs:31-33`).
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
        Self { state }
    }

    pub fn state(&self) -> &Entity<InputState> {
        &self.state
    }

    pub fn value(&self, cx: &App) -> String {
        self.state.read(cx).value().to_string()
    }

    /// Focus and put the caret at the end — the move-cursor-to-end idiom `focus_add_project_field` performs in iced (`modals.rs:625-644`).
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

    /// The key-context string the field's *host* declares, so a `"<host> > Input"` binding can out-rank gpui-component's plain `"Input"` one. See the module doc.
    pub fn override_context(host: ModalKind) -> String {
        format!("{} > Input", host.key_context())
    }
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
        // A descendant predicate matching at the same node as `"Input"`; the tie is broken by registration order (see the module doc).
        assert_eq!(
            ModalInput::override_context(ModalKind::SessionLauncher),
            "ModalSessionLauncher > Input"
        );
    }
}
