//! Keyboard policy retained separately from the removed modal text fields.
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

pub fn override_context(host: ModalKind) -> String {
    format!("{} > Input", host.key_context())
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
            override_context(ModalKind::SessionLauncher),
            "ModalSessionLauncher > Input"
        );
    }
}
